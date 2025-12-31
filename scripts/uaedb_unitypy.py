#!/usr/bin/env python3
import argparse
import json
import os
import sys

INVALID_CHARS = '<>:"/\\|?*'


def ensure_unitypy():
    try:
        import UnityPy  # noqa: F401
        return
    except Exception:
        pass

    unitypy_path = os.environ.get("UNITYPY_PATH")
    if unitypy_path:
        sys.path.insert(0, unitypy_path)
    else:
        script_dir = os.path.dirname(os.path.abspath(__file__))
        candidate = os.path.abspath(os.path.join(script_dir, "..", "..", "UnityPy"))
        if os.path.isdir(candidate):
            sys.path.insert(0, candidate)

    try:
        import UnityPy  # noqa: F401
    except Exception as exc:
        raise RuntimeError(
            "UnityPy import failed. Install UnityPy or set UNITYPY_PATH."
        ) from exc


def sanitize_segment(segment):
    if not segment:
        return "_"
    cleaned = []
    for ch in segment:
        if ch in INVALID_CHARS or ord(ch) < 32:
            cleaned.append("_")
        else:
            cleaned.append(ch)
    cleaned_str = "".join(cleaned).strip()
    return cleaned_str if cleaned_str else "_"


def split_parts(component):
    component = component.replace("\\", "/")
    parts = [p for p in component.split("/") if p and p not in (".", "..")]
    return [sanitize_segment(p) for p in parts]


def disk_path_for_path(path_list):
    parts = []
    for part in path_list:
        parts.extend(split_parts(part))
    if not parts:
        parts = ["root"]
    return os.path.join(*parts)


def iter_leaf_files(obj, path_list):
    from UnityPy.files import BundleFile, WebFile

    if isinstance(obj, (BundleFile, WebFile)):
        for name in sorted(obj.files.keys()):
            yield from iter_leaf_files(obj.files[name], path_list + [name])
    else:
        yield path_list, obj


def get_bytes(obj):
    from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter

    if isinstance(obj, (EndianBinaryReader, EndianBinaryWriter)):
        return obj.bytes

    reader = getattr(obj, "reader", None)
    if reader is not None and hasattr(reader, "bytes"):
        return reader.bytes

    if hasattr(obj, "save"):
        return obj.save()

    raise TypeError(f"Unsupported file object type: {type(obj)}")


def replace_entry(env, path_list, data):
    from UnityPy.streams import EndianBinaryWriter

    if not path_list:
        raise ValueError("Empty manifest path")

    if len(path_list) == 1:
        key = path_list[0]
        old = env.files[key]
        writer = EndianBinaryWriter(data)
        writer.flags = getattr(old, "flags", 0)
        writer.name = getattr(old, "name", key)
        env.files[key] = writer
        return

    parent = env.files[path_list[0]]
    for name in path_list[1:-1]:
        parent = parent.files[name]
    key = path_list[-1]
    old = parent.files[key]
    writer = EndianBinaryWriter(data)
    writer.flags = getattr(old, "flags", 0)
    writer.name = getattr(old, "name", key)
    parent.files[key] = writer
    if hasattr(parent, "mark_changed"):
        parent.mark_changed()


def cmd_unpack(args):
    import UnityPy

    env = UnityPy.load(args.input)
    if not env.files:
        raise RuntimeError("No files found in input")

    entries = []
    for root_name in sorted(env.files.keys()):
        root_obj = env.files[root_name]
        for path_list, obj in iter_leaf_files(root_obj, [root_name]):
            data = get_bytes(obj)
            disk_path = disk_path_for_path(path_list)
            out_path = os.path.join(args.files, disk_path)
            os.makedirs(os.path.dirname(out_path), exist_ok=True)
            with open(out_path, "wb") as out_file:
                out_file.write(data)
            entries.append(
                {
                    "path": path_list,
                    "disk_path": disk_path,
                    "size": len(data),
                }
            )

    manifest = {
        "version": 1,
        "entries": entries,
    }
    with open(args.manifest, "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, ensure_ascii=True, indent=2)


def cmd_repack(args):
    import UnityPy
    from UnityPy.files import BundleFile, WebFile

    with open(args.manifest, "r", encoding="utf-8") as handle:
        manifest = json.load(handle)

    entries = manifest.get("entries", [])
    if not entries:
        raise RuntimeError("Manifest contains no entries")

    env = UnityPy.load(args.input)
    if len(env.files) != 1:
        raise RuntimeError("Expected a single root file in input")

    root_name = next(iter(env.files.keys()))
    root_obj = env.files[root_name]

    if isinstance(root_obj, (BundleFile, WebFile)):
        for entry in entries:
            disk_path = entry["disk_path"]
            path_list = entry["path"]
            in_path = os.path.join(args.files, disk_path)
            with open(in_path, "rb") as handle:
                data = handle.read()
            replace_entry(env, path_list, data)

        output_bytes = root_obj.save(packer=args.packer)
        with open(args.output, "wb") as out_file:
            out_file.write(output_bytes)
    else:
        if len(entries) != 1:
            raise RuntimeError("Non-bundle inputs must have a single entry")
        disk_path = entries[0]["disk_path"]
        in_path = os.path.join(args.files, disk_path)
        with open(in_path, "rb") as handle:
            data = handle.read()
        with open(args.output, "wb") as out_file:
            out_file.write(data)


def build_parser():
    parser = argparse.ArgumentParser(description="UAEDB UnityPy helper")
    subparsers = parser.add_subparsers(dest="command", required=True)

    unpack_parser = subparsers.add_parser("unpack", help="Unpack bundle")
    unpack_parser.add_argument("--input", required=True)
    unpack_parser.add_argument("--files", required=True)
    unpack_parser.add_argument("--manifest", required=True)

    repack_parser = subparsers.add_parser("repack", help="Repack bundle")
    repack_parser.add_argument("--input", required=True)
    repack_parser.add_argument("--files", required=True)
    repack_parser.add_argument("--manifest", required=True)
    repack_parser.add_argument("--output", required=True)
    repack_parser.add_argument(
        "--packer",
        default="original",
        choices=["none", "lz4", "lzma", "original"],
    )

    return parser


def main():
    ensure_unitypy()
    parser = build_parser()
    args = parser.parse_args()

    if args.command == "unpack":
        cmd_unpack(args)
    elif args.command == "repack":
        cmd_repack(args)
    else:
        raise RuntimeError(f"Unknown command: {args.command}")


if __name__ == "__main__":
    main()
