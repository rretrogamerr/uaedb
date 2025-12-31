#!/usr/bin/env python3
import argparse
import os
import re
import shutil

LICENSE_NAMES = {
    "license",
    "license.txt",
    "license.md",
    "licence",
    "licence.txt",
    "copying",
    "copying.txt",
    "notice",
    "notice.txt",
    "copyright",
}


def sanitize(name):
    return re.sub(r"[^A-Za-z0-9._-]+", "_", name)


def read_metadata(dist_info):
    name = ""
    version = ""
    license_text = ""
    metadata_path = os.path.join(dist_info, "METADATA")
    if not os.path.isfile(metadata_path):
        return name, version, license_text

    with open(metadata_path, "r", encoding="utf-8", errors="ignore") as handle:
        for line in handle:
            if line.startswith("Name:") and not name:
                name = line.split(":", 1)[1].strip()
            elif line.startswith("Version:") and not version:
                version = line.split(":", 1)[1].strip()
            elif line.startswith("License:") and not license_text:
                license_text = line.split(":", 1)[1].strip()
            if name and version and license_text:
                break

    return name, version, license_text


def find_license_files(dist_info):
    files = []
    for entry in os.listdir(dist_info):
        lower = entry.lower()
        if lower in LICENSE_NAMES:
            files.append(os.path.join(dist_info, entry))
    return files


def ensure_dir(path):
    os.makedirs(path, exist_ok=True)


def collect(pydeps, out_dir, includes):
    ensure_dir(out_dir)
    license_dir = out_dir

    notices = []

    if os.path.isdir(pydeps):
        for entry in sorted(os.listdir(pydeps)):
            if not entry.endswith(".dist-info"):
                continue
            dist_info = os.path.join(pydeps, entry)
            name, version, license_text = read_metadata(dist_info)
            display = name or entry
            files = find_license_files(dist_info)

            copied = []
            for file_path in files:
                base = os.path.basename(file_path)
                if name and version:
                    target_name = f"{sanitize(name)}-{sanitize(version)}-{sanitize(base)}"
                else:
                    target_name = sanitize(f"{display}-{base}")
                target_path = os.path.join(license_dir, target_name)
                shutil.copyfile(file_path, target_path)
                copied.append(target_name)

            notices.append(
                {
                    "name": name or display,
                    "version": version,
                    "license": license_text,
                    "files": copied,
                }
            )

    for include in includes:
        if not os.path.isfile(include):
            continue
        base = os.path.basename(include)
        target_name = sanitize(base)
        target_path = os.path.join(license_dir, target_name)
        shutil.copyfile(include, target_path)
        notices.append(
            {
                "name": base,
                "version": "",
                "license": "",
                "files": [target_name],
            }
        )

    summary_path = os.path.join(out_dir, "THIRD_PARTY_NOTICES.md")
    with open(summary_path, "w", encoding="utf-8") as handle:
        handle.write("# Third-Party Notices\n\n")
        for notice in notices:
            parts = [notice["name"]]
            if notice["version"]:
                parts.append(notice["version"])
            if notice["license"]:
                parts.append(f"({notice['license']})")
            line = " ".join(parts)
            if notice["files"]:
                line += f" - {', '.join(notice['files'])}"
            handle.write(f"- {line}\n")


def main():
    parser = argparse.ArgumentParser(description="Collect third-party licenses")
    parser.add_argument("--pydeps", required=True, help="Path to runtime/pydeps")
    parser.add_argument("--out", required=True, help="Output directory")
    parser.add_argument("--include", action="append", default=[], help="Extra license file to include")
    args = parser.parse_args()

    collect(args.pydeps, args.out, args.include)


if __name__ == "__main__":
    main()
