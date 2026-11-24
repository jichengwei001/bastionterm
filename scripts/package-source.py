"""Package only sharable build inputs and checks."""
from pathlib import Path
import re
import sys
from zipfile import ZipFile, ZIP_DEFLATED

root = Path(__file__).resolve().parents[1]
destination = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "dist"
destination.mkdir(parents=True, exist_ok=True)
version = re.search(r'^version = "([^"]+)"', (root / "Cargo.toml").read_text(), re.M)[1]
files = [root / name for name in (
    "LICENSE", "THIRD-PARTY.md", "Cargo.toml", "Cargo.lock", "build.rs", "README.md", "MCP配置与Agent提示词.md", ".gitignore")]
for directory in ("src", "assets", "scripts", "checks", "vendor", "docs", ".github/workflows"):
    files.extend(p for p in sorted((root / directory).rglob("*"))
                 if p.is_file() and not p.name.startswith(".")
                 and "__pycache__" not in p.parts and p.suffix not in (".log", ".pyc"))
out = destination / f"bastionterm-{version}-source.zip"
with ZipFile(out, "w", ZIP_DEFLATED) as archive:
    for path in files:
        archive.write(path, "bastionterm/" + path.relative_to(root).as_posix())
print(out)
