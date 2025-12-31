use std::env;
use std::fs;
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
    /// Patch file (.xdelta) for the uncompressed entry.
    patch: Option<PathBuf>,
    /// Output bundle path.
    output: Option<PathBuf>,
    /// Write uncompressed bundle data to this path and exit.
    #[arg(long, value_name = "PATH")]
    uncompress: Option<PathBuf>,
    /// Entry path inside bundle to patch when multiple files are present.
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
    bundle.decompress_to_file(input, output)?;
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

    let data_path = work_path.join("bundle.data");
    let decompress_start = log_step_start("Uncompressing bundle");
    bundle.decompress_to_file(input, &data_path)?;
    log_step_done("Uncompress", decompress_start);

    let (entry_index, patched_path_opt) = if entry.is_none() && bundle.entries().len() > 1 {
        auto_detect_entry(&bundle, &data_path, patch_path, &work_path, xdelta, keep_work)?
    } else {
        let (entry_index, _) = select_entry(bundle.entries(), entry)?;
        (entry_index, None)
    };

    let entry_info = &bundle.entries()[entry_index];
    eprintln!(
        "Selected entry: {} ({} bytes)",
        entry_info.path, entry_info.size
    );

    let patched_path = if let Some(path) = patched_path_opt {
        path
    } else {
        let entry_path = work_path.join("entry.bin");
        let extract_start = log_step_start("Extracting entry");
        bundle.extract_entry(&data_path, entry_index, &entry_path)?;
        log_step_done("Extract", extract_start);

        let patched_path = work_path.join("entry_patched.bin");
        let patch_start = log_step_start("Applying xdelta patch");
        run_xdelta(xdelta, &entry_path, patch_path, &patched_path)?;
        log_step_done("Patch", patch_start);
        patched_path
    };

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

    bundle.write_bundle(out, &rebuilt_data_path, &new_entries, data_flags, block_info_flags)?;
    log_step_done("Rebuild", rebuild_start);

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

fn auto_detect_entry(
    bundle: &UnityFsBundle,
    data_path: &Path,
    patch_path: &Path,
    work_path: &Path,
    xdelta: &Path,
    keep_work: bool,
) -> Result<(usize, Option<PathBuf>)> {
    let detect_start = log_step_start("Auto-detecting patch target");
    let entry_path = work_path.join("entry_candidate.bin");
    let mut matches: Vec<(usize, String, PathBuf)> = Vec::new();

    for (idx, entry) in bundle.entries().iter().enumerate() {
        bundle.extract_entry(data_path, idx, &entry_path)?;
        let patched_path = work_path.join(format!("entry_patched_{idx}.bin"));
        let ok = try_xdelta(xdelta, &entry_path, patch_path, &patched_path)?;
        if ok {
            matches.push((idx, entry.path.clone(), patched_path));
        } else {
            fs::remove_file(&patched_path).ok();
        }
    }

    log_step_done("Auto-detect", detect_start);

    match matches.len() {
        0 => bail!("Patch did not apply to any entry. Use --list-entries and --entry."),
        1 => {
            let (idx, path, patched_path) = matches.pop().unwrap();
            eprintln!("Auto-detected entry: {}", path);
            Ok((idx, Some(patched_path)))
        }
        _ => {
            let preview = matches
                .iter()
                .take(5)
                .map(|(_, path, _)| path.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if !keep_work {
                for (_, _, path) in &matches {
                    fs::remove_file(path).ok();
                }
            }
            bail!(
                "Patch applied to multiple entries ({}). Use --entry. Matches: {}",
                matches.len(),
                preview
            );
        }
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

fn try_xdelta(xdelta: &Path, source: &Path, patch: &Path, output: &Path) -> Result<bool> {
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("Run xdelta3 on {}", patch.display()))?;

    if !status.success() {
        fs::remove_file(output).ok();
        return Ok(false);
    }

    Ok(true)
}

fn log_step_start(label: &str) -> Instant {
    eprintln!("{label}...");
    Instant::now()
}

fn log_step_done(label: &str, start: Instant) {
    eprintln!("{label} done in {:.1}s", start.elapsed().as_secs_f64());
}
