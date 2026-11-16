#!/usr/bin/env python3
"""Writes the third-party notices for DiffCode for Windows: every crate compiled into DiffCode.exe (not build
scripts, procedural macros or tests) with its license texts, each distinct text once. The installer and the zip
ship the file beside the program, as the Mac app shows its notices in About DiffCode.

Run from windows/:  python installer/notices.py target/THIRD-PARTY-NOTICES.txt
"""
import hashlib
import json
import os
import subprocess
import sys

TARGET = "x86_64-pc-windows-msvc"
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "unlicense", "copyright")


def license_files(directory):
    found = []
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if name.lower().startswith(LICENSE_PREFIXES):
            if os.path.isfile(path):
                found.append(path)
            elif os.path.isdir(path):  # REUSE layout: LICENSES/MIT.txt
                found += [os.path.join(path, n) for n in sorted(os.listdir(path)) if os.path.isfile(os.path.join(path, n))]
    return found


def main(out):
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET]))
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "diffcode")
    shipped, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in shipped:
            continue
        # Procedural macros run while compiling; neither they nor what they use end up in the program.
        if pid != root and any("proc-macro" in t["kind"] for t in packages[pid]["targets"]):
            continue
        shipped.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    crates = sorted((packages[p] for p in shipped if packages[p]["source"]), key=lambda p: (p["name"], p["version"]))

    texts = {}  # hash -> (text, [crate labels])
    lines = []
    for crate in crates:
        label = f"{crate['name']} {crate['version']}"
        where = crate.get("repository") or f"https://crates.io/crates/{crate['name']}"
        lines.append(f"{label}  ({crate.get('license') or 'see its license file'})  {where}")
        files = license_files(os.path.dirname(crate["manifest_path"]))
        if not files:
            lines[-1] += "  [no license file in the package]"
        for path in files:
            with open(path, encoding="utf-8", errors="replace") as f:
                text = f.read().strip()
            key = hashlib.sha256(text.encode()).hexdigest()
            texts.setdefault(key, (text, []))[1].append(label)

    out_lines = [
        "DiffCode for Windows includes the following third-party software, used under the licenses below.",
        "DiffCode itself is free software under the GNU General Public License v3.0 (LICENSE.txt);",
        "its source is at https://github.com/ihou/DiffCode.",
        "",
        "=" * 78,
        "Components",
        "=" * 78,
        *lines,
        "",
        "=" * 78,
        "License texts",
        "=" * 78,
    ]
    for text, users in texts.values():
        out_lines += ["", "-" * 78, "Used by: " + ", ".join(users), "-" * 78, "", text]
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w", encoding="utf-8", newline="\r\n") as f:
        f.write("\n".join(out_lines) + "\n")
    print(f"{out}: {len(crates)} crates, {len(texts)} distinct license texts")


if __name__ == "__main__":
    main(sys.argv[1])
