use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use tempfile::Builder;

#[derive(Parser)]
#[command(name = "UAEDB", version, about = "Unity asset delta patcher")]
struct Cli {
    /// Input Unity asset bundle or serialized file.
    input: PathBuf,
    /// Patch file (.xdelta) for the unpacked file.
    patch: PathBuf,
    /// Output bundle path.
    output: PathBuf,
    /// Working directory for temporary files (default: current dir).
    #[arg(long)]
    work_dir: Option<PathBuf>,
    /// Keep the working directory after completion.
    #[arg(long)]
    keep_work: bool,
    /// Path to xdelta3 binary (default: runtime/xdelta/xdelta3 or xdelta3).
    #[arg(long)]
    xdelta: Option<PathBuf>,
    /// Unity packer to use for bundles.
    #[arg(long, value_enum, default_value = "original")]
    packer: Packer,
    /// Python interpreter to use (default: runtime/python/python or python).
    #[arg(long)]
    python: Option<PathBuf>,
    /// UnityPy source path (optional). If set, passed as UNITYPY_PATH.
    #[arg(long)]
    unitypy: Option<PathBuf>,
}

#[derive(Clone, Debug, ValueEnum)]
enum Packer {
    None,
    Lz4,
    Lzma,
    Original,
}

impl Packer {
    fn as_str(&self) -> &'static str {
        match self {
            Packer::None => "none",
            Packer::Lz4 => "lz4",
            Packer::Lzma => "lzma",
            Packer::Original => "original",
        }
    }
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    entries: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
struct ManifestEntry {
    disk_path: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let python = cli.python.unwrap_or_else(default_python_path);
    let xdelta = cli.xdelta.unwrap_or_else(default_xdelta_path);

    apply_patch_path(
        &python,
        cli.unitypy.as_ref(),
        &xdelta,
        &cli.input,
        &cli.patch,
        &cli.output,
        cli.work_dir.as_ref(),
        cli.keep_work,
        cli.packer,
    )
}

fn script_path() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(exe_dir) = exe_dir() {
        candidates.push(exe_dir.join("scripts").join("uaedb_unitypy.py"));
        candidates.push(exe_dir.join("uaedb_unitypy.py"));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("uaedb_unitypy.py"),
    );

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.to_path_buf());
        }
    }

    let joined = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if joined.is_empty() {
        bail!("UnityPy helper script not found.");
    }

    bail!("UnityPy helper script not found. Tried: {}", joined);
}

fn exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
}

fn runtime_dir() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("runtime"))
}

fn default_python_path() -> PathBuf {
    if let Some(runtime) = runtime_dir() {
        let exe_name = if cfg!(windows) { "python.exe" } else { "python" };
        let candidate = runtime.join("python").join(exe_name);
        if candidate.exists() {
            return candidate;
        }
    }

    PathBuf::from("python")
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

fn runtime_pythonpath() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(runtime) = runtime_dir() {
        let pydeps = runtime.join("pydeps");
        if pydeps.exists() {
            paths.push(pydeps);
        }
        let unitypy = runtime.join("unitypy");
        if unitypy.exists() {
            paths.push(unitypy);
        }
    }
    paths
}

fn run_python(python: &Path, unitypy: Option<&PathBuf>, args: &[OsString]) -> Result<()> {
    let mut command = Command::new(python);
    if let Some(unitypy) = unitypy {
        command.env("UNITYPY_PATH", unitypy);
    }

    let mut pythonpath_entries = runtime_pythonpath();
    if let Some(existing) = env::var_os("PYTHONPATH") {
        pythonpath_entries.extend(env::split_paths(&existing));
    }
    if !pythonpath_entries.is_empty() {
        let joined = env::join_paths(pythonpath_entries.iter())
            .context("Join PYTHONPATH for UnityPy runtime")?;
        command.env("PYTHONPATH", joined);
        command.env("PYTHONNOUSERSITE", "1");
    }

    let output = command.args(args).output().with_context(|| {
        format!(
            "Failed to run python: {} {}",
            python.display(),
            args.iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "Python helper failed (exit {}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }

    Ok(())
}

fn unpack_bundle(python: &Path, unitypy: Option<&PathBuf>, input: &Path, out: &Path) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("Create output dir: {}", out.display()))?;
    let files_dir = out.join("files");
    fs::create_dir_all(&files_dir)
        .with_context(|| format!("Create files dir: {}", files_dir.display()))?;
    let manifest = out.join("manifest.json");

    let script = script_path()?;
    let args = vec![
        script.into_os_string(),
        OsString::from("unpack"),
        OsString::from("--input"),
        input.as_os_str().to_os_string(),
        OsString::from("--files"),
        files_dir.as_os_str().to_os_string(),
        OsString::from("--manifest"),
        manifest.as_os_str().to_os_string(),
    ];

    run_python(python, unitypy, &args)
}

fn repack_bundle(
    python: &Path,
    unitypy: Option<&PathBuf>,
    input: &Path,
    files: &Path,
    manifest: &Path,
    out: &Path,
    packer: Packer,
) -> Result<()> {
    let script = script_path()?;
    let args = vec![
        script.into_os_string(),
        OsString::from("repack"),
        OsString::from("--input"),
        input.as_os_str().to_os_string(),
        OsString::from("--files"),
        files.as_os_str().to_os_string(),
        OsString::from("--manifest"),
        manifest.as_os_str().to_os_string(),
        OsString::from("--output"),
        out.as_os_str().to_os_string(),
        OsString::from("--packer"),
        OsString::from(packer.as_str()),
    ];

    run_python(python, unitypy, &args)
}

fn apply_patch_path(
    python: &Path,
    unitypy: Option<&PathBuf>,
    xdelta: &Path,
    input: &Path,
    patch_path: &Path,
    out: &Path,
    work_dir: Option<&PathBuf>,
    keep_work: bool,
    packer: Packer,
) -> Result<()> {
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

    let unpack_dir = work_path.join("unpack");
    let files_dir = unpack_dir.join("files");
    let manifest_path = unpack_dir.join("manifest.json");
    unpack_bundle(python, unitypy, input, &unpack_dir)?;

    let manifest = read_manifest(&manifest_path)?;
    if manifest.entries.len() != 1 {
        let preview = manifest
            .entries
            .iter()
            .take(5)
            .map(|entry| entry.disk_path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "Expected exactly 1 unpacked file, found {}. Entries: {}",
            manifest.entries.len(),
            preview
        );
    }

    let entry = &manifest.entries[0];
    let src_path = files_dir.join(&entry.disk_path);
    if !src_path.exists() {
        bail!(
            "Unpacked file not found: {}",
            src_path.display()
        );
    }

    let patched_dir = work_path.join("patched");
    fs::create_dir_all(&patched_dir)
        .with_context(|| format!("Create patched dir: {}", patched_dir.display()))?;

    let dst_path = patched_dir.join(&entry.disk_path);
    if let Some(parent) = dst_path.parent() {
        fs::create_dir_all(parent.to_path_buf())
            .with_context(|| format!("Create dir: {}", parent.display()))?;
    }

    run_xdelta(xdelta, &src_path, patch_path, &dst_path)?;

    repack_bundle(
        python,
        unitypy,
        input,
        &patched_dir,
        &manifest_path,
        out,
        packer,
    )?;

    if keep_work {
        eprintln!("Work directory kept at: {}", work_path.display());
    }

    Ok(())
}

fn read_manifest(path: &Path) -> Result<Manifest> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("Read manifest: {}", path.display()))?;
    let manifest: Manifest = serde_json::from_str(&data)
        .with_context(|| format!("Parse manifest: {}", path.display()))?;
    if manifest.version != 1 {
        bail!("Unsupported manifest version: {}", manifest.version);
    }
    Ok(manifest)
}

fn run_xdelta(xdelta: &Path, source: &Path, patch: &Path, output: &Path) -> Result<()> {
    if output.exists() {
        fs::remove_file(output)
            .with_context(|| format!("Remove existing file: {}", output.display()))?;
    }

    let output = Command::new(xdelta)
        .arg("-d")
        .arg("-s")
        .arg(source)
        .arg(patch)
        .arg(output)
        .output()
        .with_context(|| format!("Run xdelta3 on {}", patch.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "xdelta failed (exit {}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }

    Ok(())
}
