# UAEDB Maintenance Guide (for Agents)

This document summarizes key implementation decisions and pitfalls based on
`requirement.md` so future agents can make safe changes.

## What UAEDB Does

- Reads UnityFS bundles, produces an uncompressed UnityFS bundle (UABEA `.decomp`
  compatible), applies an xdelta patch, and recompresses.
- Supports two patch modes:
  - Full-bundle patch (default): apply xdelta to the uncompressed UnityFS bundle.
  - Entry patch (`--entry`): apply xdelta to a single entry, rebuild data and entry
    offsets.

## Core Behaviors and Decisions

- **Uncompress format**: `unpack_to_file` outputs a full UnityFS bundle
  (header + block info + data), not just the raw data stream.
- **Compression compatibility**:
  - LZ4 uses LZ4HC (Encode32HC). Implementation uses liblz4 (`lz4` crate).
  - LZMA uses Unity defaults: dict=0x800000, lc/lp/pb=3/0/2, BT4, nice_len=123,
    mode=Normal.
- **Block info vs data flags**:
  - Data blocks are compressed using `block_info_flags` (per-block flags).
  - Block info itself is compressed using header `data_flags & COMP_MASK`.
- **Block layout**:
  - When possible, reuse the original block layout so recompression matches
    UABEA/Unity output.
  - If layout is missing or unusable, re-chunk at 0x20000.
- **Raw data fallback (full-bundle patch)**:
  - Some patches corrupt block info (e.g., block sum < max entry end).
  - When detected, treat the patched bundle data as raw bytes from
    `data_start..EOF` and recompress.

## Patch Flows (High Level)

### Full-bundle patch (default)
1. `bundle.unpack_to_file` -> `bundle.uncompressed`
2. `xdelta3` applies patch -> `bundle_patched.uncompressed`
3. Extract data stream:
   - If block info looks valid: `decompress_to_file`.
   - If block info looks invalid: extract raw data from `data_start..EOF`.
4. Rebuild bundle using preserved block layout when possible.

### Entry patch (`--entry`)
1. `bundle.decompress_to_file` -> `bundle.data`
2. Extract entry -> `entry.bin`
3. Apply xdelta -> `entry_patched.bin`
4. Rebuild data stream + entry offsets -> `bundle_patched.data`
5. Rebuild bundle.

## Operational Notes

- `xdelta3` is an external binary. For portable Windows builds it is bundled
  under `runtime/xdelta/xdelta3.exe` and included in third-party notices.
- `patch.bat` (Windows) provides a user-friendly flow: backup, patch, replace,
  restore-on-failure, keep backup.
- Progress UI uses a simple CLI spinner with percentage updates.

## Files to Know

- `src/unityfs.rs`: UnityFS parsing/compress/decompress logic.
- `src/main.rs`: CLI flow, patch modes, progress UI, xdelta integration.
- `scripts/package_windows.ps1`: portable packaging (includes xdelta + licenses).
- `docs/USAGE*.md`: end-user instructions.

## Common Pitfalls

- Mixing `data_flags` with `block_info_flags` will produce invalid bundles.
- Using `decompress_to_file` on a patched bundle with corrupted block info can
  truncate data; use raw extraction fallback when block totals do not match
  entry ranges.
- Do not assume a bundle has a single entry; full-bundle patch is the default.
