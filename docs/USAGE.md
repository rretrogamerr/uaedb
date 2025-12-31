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
If a bundle contains multiple entries, pass `--entry` to select which file to
patch (use `--list-entries` to see all paths). Without `--entry`, UAEDB tries
each entry with the patch and requires exactly one match.

List entries and select a target:

```bash
uaedb original.unity3d patch.xdelta original_patched.unity3d --list-entries
uaedb original.unity3d patch.xdelta original_patched.unity3d --entry "data.unity3d/GI/level84/..."
```

Uncompress only:

```bash
uaedb original.unity3d --uncompress original.unity3d.uncompressed
```

## Notes

- If you move `uaedb.exe`, also move the `runtime/` folder.
- If `xdelta3` is not found, pass `--xdelta` with the full path to `xdelta3.exe`.
- Use `--keep-work` to inspect `entry.bin` and `bundle.data` inside the kept work directory.
