# UAEDB Portable (Windows)

## Install

1. Download the portable zip from the GitHub release.
2. Extract the zip to any folder.
3. Keep the `runtime/` and `scripts/` folders next to `uaedb.exe`.

## Usage

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d
```

`patch.xdelta` must be a file. Directories are rejected with an error.
UAEDB expects the bundle to uncompress into a single file.

## Notes

- If you move `uaedb.exe`, also move the `runtime/` and `scripts/` folders.
- If `xdelta3` is not found, pass `--xdelta` with the full path to `xdelta3.exe`.
- Use `--keep-work` to inspect the unpacked file under `workdir/unpack/files/`.
