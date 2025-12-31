# UAEDB

UAEDB uncompresses Unity asset bundles, applies a `.xdelta` patch to the single
unpacked file, then recompresses the bundle.

## Install (zip portable)

1. Download the portable zip from the GitHub release.
2. Extract the zip to any folder.
3. Keep the `runtime/` folder next to `uaedb.exe`.

## Requirements

- Rust toolchain (build)
- Python 3.8+ with UnityPy installed or `UNITYPY_PATH` set to a local UnityPy repo
- `xdelta3` binary in `PATH` (or pass `--xdelta`)

For portable builds, UAEDB looks for:

- `scripts/uaedb_unitypy.py` next to `uaedb.exe`
- `runtime/pydeps` for bundled UnityPy and dependencies
- `runtime/xdelta/xdelta3.exe` for the bundled xdelta

## Build

```bash
cargo build --release
```

## Usage

### One-shot patch

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d
```

`patch.xdelta` must be a file. Directories are rejected with an error.
UAEDB expects the bundle to uncompress into a single file; if more than one
file is found, it exits with an error.

Example patch creation:

```bash
xdelta3 -e -s original_unpacked.unity3d modified_unpacked.unity3d patch.xdelta
```

Tip: run with `--keep-work` to inspect the unpacked file under
`workdir/unpack/files/`.

## Troubleshooting

- If you move `uaedb.exe`, also move the `runtime/` folder with it.
- If `xdelta3` is not found, pass `--xdelta` with the full path to `xdelta3.exe`.

## Portable zip build (Windows)

Use the packaging script to build a portable zip that includes UnityPy
dependencies and xdelta:

```powershell
.\scripts\package_windows.ps1
```

The output folder and zip will be created under `dist/`.

The script also generates `licenses/THIRD_PARTY_NOTICES.md` and includes
`docs/USAGE.md` (English) plus `docs/USAGE_KO.md` (Korean) in the zip.
