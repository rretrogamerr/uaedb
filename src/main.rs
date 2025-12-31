use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

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
    let decompress_start = log_step_start("Uncompressing bundle");
    bundle.unpack_to_file(input, output)?;
    log_step_done("Uncompress", decompress_start);
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
        let decompress_start = log_step_start("Uncompressing bundle data");
        bundle.decompress_to_file(input, &data_path)?;
        log_step_done("Uncompress", decompress_start);

        let (entry_index, _) = select_entry(bundle.entries(), Some(entry))?;
        let entry_info = &bundle.entries()[entry_index];
        eprintln!(
            "Selected entry: {} ({} bytes)",
            entry_info.path, entry_info.size
        );

        let entry_path = work_path.join("entry.bin");
        let extract_start = log_step_start("Extracting entry");
        bundle.extract_entry(&data_path, entry_index, &entry_path)?;
        log_step_done("Extract", extract_start);

        let patched_path = work_path.join("entry_patched.bin");
        let patch_start = log_step_start("Applying xdelta patch");
        run_xdelta(xdelta, &entry_path, patch_path, &patched_path)?;
        log_step_done("Patch", patch_start);

        let rebuilt_data_path = work_path.join("bundle_patched.data");
        let rebuild_start = log_step_start("Rebuilding bundle");
        let new_entries = bundle.rebuild_data_file(
            &data_path,
            entry_index,
            &patched_path,
            &rebuilt_data_path,
        )?;

        let (data_flags, block_info_flags) = apply_packer(
            bundle.flags(),
            bundle.block_info_flags(),
            packer,
        );

        bundle.write_bundle(
            out,
            &rebuilt_data_path,
            &new_entries,
            data_flags,
            block_info_flags,
        )?;
        log_step_done("Rebuild", rebuild_start);
    } else {
        let uncompressed_path = work_path.join("bundle.uncompressed");
        let unpack_start = log_step_start("Uncompressing bundle");
        bundle.unpack_to_file(input, &uncompressed_path)?;
        log_step_done("Uncompress", unpack_start);

        let uncompressed_bundle =
            UnityFsBundle::read(&uncompressed_path).context("Read uncompressed bundle")?;

        let patched_bundle_path = work_path.join("bundle_patched.uncompressed");
        let patch_start = log_step_start("Applying xdelta patch");
        run_xdelta(xdelta, &uncompressed_path, patch_path, &patched_bundle_path)?;
        log_step_done("Patch", patch_start);

        let patched_bundle =
            UnityFsBundle::read(&patched_bundle_path).context("Read patched bundle")?;
        let data_path = work_path.join("bundle.data");
        let extract_start = log_step_start("Extracting data");
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
            extract_raw_data(&patched_bundle_path, start, len, &data_path)?;
            data_len = Some(len);
        } else {
            patched_bundle.decompress_to_file(&patched_bundle_path, &data_path)?;
        }
        log_step_done("Extract", extract_start);

        let (data_flags, block_info_flags) = apply_packer(
            bundle.flags(),
            bundle.block_info_flags(),
            packer,
        );

        let rebuild_start = log_step_start("Rebuilding bundle");
        if use_raw_data {
            let layout_total: u64 = uncompressed_bundle
                .blocks()
                .iter()
                .map(|block| block.uncompressed_size as u64)
                .sum();
            let data_len = data_len.unwrap_or(layout_total);
            if data_len == layout_total {
                patched_bundle.write_bundle_with_layout(
                    out,
                    &data_path,
                    patched_bundle.entries(),
                    data_flags,
                    block_info_flags,
                    uncompressed_bundle.blocks(),
                )?;
            } else {
                patched_bundle.write_bundle(
                    out,
                    &data_path,
                    patched_bundle.entries(),
                    data_flags,
                    block_info_flags,
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
            )?;
        }
        log_step_done("Rebuild", rebuild_start);
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

fn extract_raw_data(input_path: &Path, start: u64, len: u64, output_path: &Path) -> Result<()> {
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
    io::copy(&mut limited, &mut output)?;
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

    let status = Command::new(xdelta)
        .arg("-d")
        .arg("-s")
        .arg(source)
        .arg(patch)
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("Run xdelta3 on {}", patch.display()))?;

    if !status.success() {
        bail!("xdelta failed (exit {}). See output above.", status);
    }

    Ok(())
}

fn log_step_start(label: &str) -> Instant {
    eprintln!("{label}...");
    Instant::now()
}

fn log_step_done(label: &str, start: Instant) {
    eprintln!("{label} done in {:.1}s", start.elapsed().as_secs_f64());
}
