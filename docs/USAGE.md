# UAEDB Portable (Windows)

## Install

1. Download the portable zip from the GitHub release.
2. Extract the zip to any folder.
3. Keep the `runtime/` folder next to `uaedb.exe`.

## Usage

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d
```

`patch.xdelta` must be a file. Directories are rejected with an error.
By default UAEDB applies the patch to the full uncompressed bundle. Use
`--entry` to patch a specific entry instead (use `--list-entries` to see
all paths).

End users can run `patch.bat` in the game folder. It expects
`data.unity3d` and `data.xdelta`, creates `data.unity3d.bak`, replaces
`data.unity3d` on success, and keeps the backup for manual cleanup.

List entries and select a target entry:

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d --list-entries
uaedb original.unity3d patch.xdelta original_patched.unity3d --entry "data.unity3d/GI/level84/..."
```

Create a patch from uncompressed bundles:

```bash
uaedb original.unity3d --uncompress original.unity3d.uncompressed
uaedb modified.unity3d --uncompress modified.unity3d.uncompressed
xdelta3 -e -s original.unity3d.uncompressed modified.unity3d.uncompressed patch.xdelta
```

Uncompress only:

```bash
uaedb original.unity3d --uncompress original.unity3d.uncompressed
```

This outputs an uncompressed UnityFS bundle (matching UABEA's `.decomp` format).

## Notes

- If you move `uaedb.exe`, also move the `runtime/` folder.
- If `xdelta3` is not found, pass `--xdelta` with the full path to `xdelta3.exe`.
- Use `--keep-work` to inspect `entry.bin`, `entry_patched.bin`, `bundle_patched.data`,
  or `bundle.uncompressed`, `bundle_patched.uncompressed`, `bundle.data` inside the kept work directory.
