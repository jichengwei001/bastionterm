"""Collect notices from the exact Cargo packages in a target's runtime dependency tree."""
import argparse
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("target")
parser.add_argument("output", type=Path)
args = parser.parse_args()
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", args.target]))
tree = subprocess.check_output([
    "cargo", "tree", "--locked", "--target", args.target, "--edges", "normal",
    "--prefix", "none", "--format", "{p}"], text=True)
packages = {tuple(line.split()[:2]) for line in tree.splitlines() if line.strip()}
with args.output.open("w", encoding="utf-8", newline="\n") as out:
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if (package["name"], "v" + package["version"]) not in packages:
            continue
        root = Path(package["manifest_path"]).parent
        print(f"\n=== {package['name']} {package['version']} ===", file=out)
        print(package.get("license") or "See package license file", file=out)
        if package.get("repository"):
            print(package["repository"], file=out)
        files = {p for p in root.iterdir() if p.is_file() and
                 p.name.lower().startswith(("license", "licence", "copying", "notice", "copyright", "unlicense"))}
        for directory in (root / "licenses", root / "LICENSES"):
            if directory.is_dir():
                files.update(p for p in directory.rglob("*") if p.is_file())
        if package.get("license_file"):
            files.add(root / package["license_file"])
        for path in sorted(files):
            if path.is_file():
                print(path.name, path.read_text(encoding="utf-8", errors="replace"), sep="\n", file=out)
