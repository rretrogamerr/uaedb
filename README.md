# UAEDB

UAEDB uncompresses Unity asset bundles, applies a `.xdelta` patch to the target
file inside, then recompresses the bundle.

## Install (zip portable)

1. Download the portable zip from the GitHub release.
2. Extract the zip to any folder.
3. Keep the `runtime/` folder next to `uaedb.exe`.

## Requirements

- Rust toolchain (build)
- `xdelta3` binary in `PATH` (or pass `--xdelta`)

For portable builds, UAEDB looks for:

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
If a bundle contains multiple entries, pass `--entry` to select which file
to patch (use `--list-entries` to see all paths).

Example patch creation:

```bash
xdelta3 -e -s original_unpacked.unity3d modified_unpacked.unity3d patch.xdelta
```

When a bundle expands into multiple files:

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d --list-entries
uaedb original.unity3d patch.xdelta original_patched.unity3d --entry "data.unity3d/GI/level84/..."
```

Tip: run with `--keep-work` to inspect the extracted entry (`entry.bin`) and
the uncompressed bundle data (`bundle.data`) inside the kept work directory.

## Troubleshooting

- If you move `uaedb.exe`, also move the `runtime/` folder with it.
- If `xdelta3` is not found, pass `--xdelta` with the full path to `xdelta3.exe`.

## Portable zip build (Windows)

Use the packaging script to build a portable zip that includes xdelta:

```powershell
.\scripts\package_windows.ps1
```

The output folder and zip will be created under `dist/`.

The script also generates `licenses/THIRD_PARTY_NOTICES.md` and includes
`docs/USAGE.md` (English) plus `docs/USAGE_KO.md` (Korean) in the zip.
