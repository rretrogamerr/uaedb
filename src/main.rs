use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use tempfile::Builder;

mod unityfs;

use unityfs::{
    DirectoryEntry, UnityFsBundle, COMP_LZ4, COMP_LZMA, COMP_MASK, COMP_NONE,
};

#[derive(Parser)]
#[command(name = "UAEDB", version, about = "Unity asset delta patcher")]
struct Cli {
    /// Input Unity asset bundle.
    input: PathBuf,
    /// Patch file (.xdelta) for the uncompressed bundle.
    patch: Option<PathBuf>,
    /// Output bundle path.
    output: Option<PathBuf>,
    /// Write an uncompressed UnityFS bundle to this path and exit.
    #[arg(long, value_name = "PATH")]
    uncompress: Option<PathBuf>,
    /// Patch a specific entry instead of the full uncompressed bundle.
    #[arg(long)]
    entry: Option<String>,
    /// List bundle entries and exit.
    #[arg(long)]
    list_entries: bool,
    /// Working directory for temporary files (default: current dir).
    #[arg(long)]
    work_dir: Option<PathBuf>,
    /// Keep the working directory after completion.
    #[arg(long)]
    keep_work: bool,
    /// Path to xdelta3 binary (default: runtime/xdelta/xdelta3 or xdelta3).
    #[arg(long)]
    xdelta: Option<PathBuf>,
    /// Bundle compression to use when writing output.
    #[arg(long, value_enum, default_value = "original")]
    packer: Packer,
}

#[derive(Clone, Debug, ValueEnum)]
enum Packer {
    None,
    Lz4,
    Lzma,
    Original,
}

impl Packer {
    fn override_compression(&self) -> Option<u32> {
        match self {
            Packer::None => Some(COMP_NONE),
            Packer::Lz4 => Some(COMP_LZ4),
            Packer::Lzma => Some(COMP_LZMA),
            Packer::Original => None,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let xdelta = cli.xdelta.unwrap_or_else(default_xdelta_path);

    if let Some(out) = cli.uncompress.as_ref() {
        return uncompress_only(&cli.input, out);
    }

    let patch = cli
        .patch
        .as_ref()
        .context("Missing patch path. Provide PATCH or use --uncompress.")?;
    let output = cli
        .output
        .as_ref()
        .context("Missing output path. Provide OUTPUT or use --uncompress.")?;

    apply_patch_path(
        &xdelta,
        &cli.input,
        patch,
        output,
        cli.work_dir.as_ref(),
        cli.keep_work,
        cli.entry.as_deref(),
        cli.list_entries,
        cli.packer,
    )
}

fn exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
}

fn runtime_dir() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("runtime"))
}

fn default_xdelta_path() -> PathBuf {
    if let Some(runtime) = runtime_dir() {
        let exe_name = if cfg!(windows) {
            "xdelta3.exe"
        } else {
            "xdelta3"
        };
        let candidate = runtime.join("xdelta").join(exe_name);
        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from("xdelta3")
}

fn uncompress_only(input: &Path, output: &Path) -> Result<()> {
    if !input.is_file() {
        bail!("Input bundle not found: {}", input.display());
    }

    let bundle = UnityFsBundle::read(input)?;
    let total = total_uncompressed_bytes(bundle.blocks());
    let mut progress = StepProgress::new("Uncompressing bundle", total);
    {
        let mut callback = |done| progress.update(done);
        bundle.unpack_to_file(input, output, Some(&mut callback))?;
    }
    progress.finish("Uncompress");
    Ok(())
}

fn apply_patch_path(
    xdelta: &Path,
    input: &Path,
    patch_path: &Path,
    out: &Path,
    work_dir: Option<&PathBuf>,
    keep_work: bool,
    entry: Option<&str>,
    list_entries: bool,
    packer: Packer,
) -> Result<()> {
    if !input.is_file() {
        bail!("Input bundle not found: {}", input.display());
    }

    let bundle = UnityFsBundle::read(input)?;

    if list_entries {
        print_entries(bundle.entries());
        return Ok(());
    }

    if !patch_path.is_file() {
        if patch_path.is_dir() {
            bail!(
                "Patch path must be a .xdelta file, not a directory: {}",
                patch_path.display()
            );
        }
        bail!("Patch path not found: {}", patch_path.display());
    }

    let work_root = match work_dir {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("Get current dir")?,
    };
    fs::create_dir_all(&work_root)
        .with_context(|| format!("Create work root: {}", work_root.display()))?;

    let temp = Builder::new()
        .prefix("uaedb-work-")
        .tempdir_in(&work_root)
        .context("Create temp work dir")?;

    let work_path = if keep_work {
        temp.keep()
    } else {
        temp.path().to_path_buf()
    };

    if let Some(entry) = entry {
        let data_path = work_path.join("bundle.data");
        let total = total_uncompressed_bytes(bundle.blocks());
        let mut progress = StepProgress::new("Uncompressing bundle data", total);
        {
            let mut callback = |done| progress.update(done);
            bundle.decompress_to_file(input, &data_path, Some(&mut callback))?;
        }
        progress.finish("Uncompress");

        let (entry_index, _) = select_entry(bundle.entries(), Some(entry))?;
        let entry_info = &bundle.entries()[entry_index];
        eprintln!(
            "Selected entry: {} ({} bytes)",
            entry_info.path, entry_info.size
        );

        let entry_path = work_path.join("entry.bin");
        let mut progress = StepProgress::new("Extracting entry", entry_info.size);
        {
            let mut callback = |done| progress.update(done);
            bundle.extract_entry(&data_path, entry_index, &entry_path, Some(&mut callback))?;
        }
        progress.finish("Extract");

        let patched_path = work_path.join("entry_patched.bin");
        run_xdelta(xdelta, &entry_path, patch_path, &patched_path)?;

        let rebuilt_data_path = work_path.join("bundle_patched.data");
        let patched_size = fs::metadata(&patched_path)
            .with_context(|| format!("Stat patched entry: {}", patched_path.display()))?
            .len();
        let original_total: u64 = bundle.entries().iter().map(|item| item.size).sum();
        let rebuilt_total = original_total
            .checked_sub(entry_info.size)
            .and_then(|size| size.checked_add(patched_size))
            .context("Compute rebuilt data size")?;
        let mut progress = StepProgress::new("Rebuilding data", rebuilt_total);
        let new_entries = {
            let mut callback = |done| progress.update(done);
            bundle.rebuild_data_file(
                &data_path,
                entry_index,
                &patched_path,
                &rebuilt_data_path,
                Some(&mut callback),
            )?
        };
        progress.finish("Rebuild data");

        let (data_flags, block_info_flags) = apply_packer(
            bundle.flags(),
            bundle.block_info_flags(),
            packer,
        );

        let data_len = fs::metadata(&rebuilt_data_path)
            .with_context(|| format!("Stat rebuilt data: {}", rebuilt_data_path.display()))?
            .len();
        let mut progress = StepProgress::new("Rebuilding bundle", data_len);
        {
            let mut callback = |done| progress.update(done);
            bundle.write_bundle(
                out,
                &rebuilt_data_path,
                &new_entries,
                data_flags,
                block_info_flags,
                Some(&mut callback),
            )?;
        }
        progress.finish("Rebuild bundle");
    } else {
        let uncompressed_path = work_path.join("bundle.uncompressed");
        let total = total_uncompressed_bytes(bundle.blocks());
        let mut progress = StepProgress::new("Uncompressing bundle", total);
        {
            let mut callback = |done| progress.update(done);
            bundle.unpack_to_file(input, &uncompressed_path, Some(&mut callback))?;
        }
        progress.finish("Uncompress");

        let uncompressed_bundle =
            UnityFsBundle::read(&uncompressed_path).context("Read uncompressed bundle")?;

        let patched_bundle_path = work_path.join("bundle_patched.uncompressed");
        run_xdelta(xdelta, &uncompressed_path, patch_path, &patched_bundle_path)?;

        let patched_bundle =
            UnityFsBundle::read(&patched_bundle_path).context("Read patched bundle")?;
        let data_path = work_path.join("bundle.data");
        let max_entry_end = patched_bundle
            .entries()
            .iter()
            .map(|entry| entry.offset + entry.size)
            .max()
            .unwrap_or(0);
        let block_total: u64 = patched_bundle
            .blocks()
            .iter()
            .map(|block| block.uncompressed_size as u64)
            .sum();
        let use_raw_data = block_total < max_entry_end;
        let mut data_len = None;
        if use_raw_data {
            eprintln!(
                "Warning: patched block info covers {} bytes but entries require {}. Using raw data region.",
                block_total, max_entry_end
            );
            let file_len = fs::metadata(&patched_bundle_path)
                .with_context(|| format!("Stat bundle: {}", patched_bundle_path.display()))?
                .len();
            let start = patched_bundle.data_start();
            let len = file_len
                .checked_sub(start)
                .context("Patched bundle shorter than data offset")?;
            if len < max_entry_end {
                bail!(
                    "Patched data length {} is smaller than max entry end {}",
                    len,
                    max_entry_end
                );
            }
            let mut progress = StepProgress::new("Extracting data", len);
            {
                let mut callback = |done| progress.update(done);
                extract_raw_data(
                    &patched_bundle_path,
                    start,
                    len,
                    &data_path,
                    Some(&mut callback),
                )?;
            }
            progress.finish("Extract");
            data_len = Some(len);
        } else {
            let total = total_uncompressed_bytes(patched_bundle.blocks());
            let mut progress = StepProgress::new("Extracting data", total);
            {
                let mut callback = |done| progress.update(done);
                patched_bundle.decompress_to_file(
                    &patched_bundle_path,
                    &data_path,
                    Some(&mut callback),
                )?;
            }
            progress.finish("Extract");
        }

        let (data_flags, block_info_flags) = apply_packer(
            bundle.flags(),
            bundle.block_info_flags(),
            packer,
        );

        let data_len = match data_len {
            Some(len) => len,
            None => fs::metadata(&data_path)
                .with_context(|| format!("Stat data: {}", data_path.display()))?
                .len(),
        };
        let mut progress = StepProgress::new("Rebuilding bundle", data_len);
        {
            let mut callback = |done| progress.update(done);
            if use_raw_data {
                let layout_total: u64 = uncompressed_bundle
                    .blocks()
                    .iter()
                    .map(|block| block.uncompressed_size as u64)
                    .sum();
                if data_len == layout_total {
                    patched_bundle.write_bundle_with_layout(
                        out,
                        &data_path,
                        patched_bundle.entries(),
                        data_flags,
                        block_info_flags,
                        uncompressed_bundle.blocks(),
                        Some(&mut callback),
                    )?;
                } else {
                    patched_bundle.write_bundle(
                        out,
                        &data_path,
                        patched_bundle.entries(),
                        data_flags,
                        block_info_flags,
                        Some(&mut callback),
                    )?;
                }
            } else {
                patched_bundle.write_bundle_with_layout(
                    out,
                    &data_path,
                    patched_bundle.entries(),
                    data_flags,
                    block_info_flags,
                    patched_bundle.blocks(),
                    Some(&mut callback),
                )?;
            }
        }
        progress.finish("Rebuild bundle");
    }

    if keep_work {
        eprintln!("Work directory kept at: {}", work_path.display());
    }

    Ok(())
}

fn apply_packer(flags: u32, block_info_flags: u16, packer: Packer) -> (u32, u16) {
    let Some(compression) = packer.override_compression() else {
        return (flags, block_info_flags);
    };

    let new_flags = (flags & !COMP_MASK) | compression;
    let new_block_info_flags = (block_info_flags & !(COMP_MASK as u16)) | (compression as u16);

    (new_flags, new_block_info_flags)
}

const SPINNER_FRAMES: [char; 4] = ['\\', '|', '/', '-'];

struct StepProgress {
    label: String,
    total: u64,
    start: Instant,
    spinner_idx: usize,
    last_render: Instant,
}

impl StepProgress {
    fn new(label: &str, total: u64) -> Self {
        let now = Instant::now();
        let last_render = now
            .checked_sub(Duration::from_millis(200))
            .unwrap_or(now);
        let mut progress = Self {
            label: label.to_string(),
            total: total.max(1),
            start: now,
            spinner_idx: 0,
            last_render,
        };
        progress.render(0, true);
        progress
    }

    fn update(&mut self, done: u64) {
        self.render(done, false);
    }

    fn finish(&mut self, done_label: &str) {
        self.render(self.total, true);
        eprintln!();
        eprintln!("{done_label} done in {:.1}s", self.start.elapsed().as_secs_f64());
    }

    fn render(&mut self, done: u64, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_render) < Duration::from_millis(120) {
            return;
        }
        let pct = (done.min(self.total) * 100) / self.total;
        let spinner = SPINNER_FRAMES[self.spinner_idx];
        self.spinner_idx = (self.spinner_idx + 1) % SPINNER_FRAMES.len();
        eprint!("\r{} {} {:>3}%", spinner, self.label, pct);
        let _ = io::stderr().flush();
        self.last_render = now;
    }
}

fn total_uncompressed_bytes(blocks: &[unityfs::BlockInfo]) -> u64 {
    blocks
        .iter()
        .map(|block| block.uncompressed_size as u64)
        .sum()
}

fn extract_raw_data(
    input_path: &Path,
    start: u64,
    len: u64,
    output_path: &Path,
    mut progress: Option<&mut dyn FnMut(u64)>,
) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Create dir: {}", parent.display()))?;
    }
    let mut input = BufReader::new(
        File::open(input_path).with_context(|| format!("Open bundle: {}", input_path.display()))?,
    );
    input.seek(SeekFrom::Start(start))?;
    let mut output = BufWriter::new(
        File::create(output_path)
            .with_context(|| format!("Create output: {}", output_path.display()))?,
    );
    let mut limited = input.take(len);
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        done = done
            .checked_add(read as u64)
            .context("Data size overflow")?;
        if let Some(callback) = progress.as_deref_mut() {
            callback(done);
        }
    }
    output.flush()?;
    Ok(())
}

fn normalize_entry_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

fn select_entry<'a>(
    entries: &'a [DirectoryEntry],
    entry: Option<&str>,
) -> Result<(usize, &'a DirectoryEntry)> {
    if entries.is_empty() {
        bail!("Bundle contains no entries");
    }

    if let Some(entry) = entry {
        let target = normalize_entry_path(entry);
        let exact_matches: Vec<(usize, &DirectoryEntry)> = entries
            .iter()
            .enumerate()
            .filter(|(_, item)| normalize_entry_path(&item.path) == target)
            .collect();
        if exact_matches.len() == 1 {
            return Ok(exact_matches[0]);
        }
        if exact_matches.len() > 1 {
            bail!(
                "Entry matches multiple files: {} ({} matches). Use --list-entries.",
                entry,
                exact_matches.len()
            );
        }

        let suffix_matches: Vec<(usize, &DirectoryEntry)> = entries
            .iter()
            .enumerate()
            .filter(|(_, item)| normalize_entry_path(&item.path).ends_with(&target))
            .collect();
        if suffix_matches.len() == 1 {
            return Ok(suffix_matches[0]);
        }
        if suffix_matches.len() > 1 {
            bail!(
                "Entry matches multiple files by suffix: {} ({} matches). Use --list-entries.",
                entry,
                suffix_matches.len()
            );
        }

        bail!("Entry not found: {}. Use --list-entries.", entry);
    }

    if entries.len() == 1 {
        return Ok((0, &entries[0]));
    }

    let preview = entries
        .iter()
        .take(5)
        .map(|item| item.path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "Expected exactly 1 bundle entry, found {}. Use --entry or --list-entries. Entries: {}",
        entries.len(),
        preview
    );
}

fn print_entries(entries: &[DirectoryEntry]) {
    for item in entries {
        println!("{}\t{}", item.size, item.path);
    }
}

fn run_xdelta(xdelta: &Path, source: &Path, patch: &Path, output: &Path) -> Result<()> {
    if output.exists() {
        fs::remove_file(output)
            .with_context(|| format!("Remove existing file: {}", output.display()))?;
    }

    let total = fs::metadata(source)
        .with_context(|| format!("Stat source: {}", source.display()))?
        .len()
        .max(1);
    let mut progress = StepProgress::new("Applying xdelta patch", total);
    let mut last_done = 0u64;

    let mut child = Command::new(xdelta)
        .arg("-d")
        .arg("-s")
        .arg(source)
        .arg(patch)
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("Run xdelta3 on {}", patch.display()))?;

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let done = fs::metadata(output).map(|meta| meta.len()).unwrap_or(0);
        last_done = done;
        progress.update(done.min(total));
        thread::sleep(Duration::from_millis(120));
    };

    if !status.success() {
        progress.render(last_done.min(total), true);
        eprintln!();
        bail!("xdelta failed (exit {}). See output above.", status);
    }

    progress.finish("Patch");
    Ok(())
}
