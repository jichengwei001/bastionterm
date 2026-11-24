#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
destination=${1:-"$PWD/dist"}
mkdir -p "$destination"
destination=$(cd "$destination" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
contents="$stage/BastionTerm.app/Contents"
mkdir -p "$contents/MacOS" "$contents/Resources"
binary=${2:-target/release/bastion-bridge-gpui}
arch=$(lipo -archs "$binary")
cp "$binary" "$contents/MacOS/"
cp assets/icon.icns "$contents/Resources/"
cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>cn.bastionbridge.gpui.preview</string>
<key>CFBundleName</key><string>BastionTerm</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleExecutable</key><string>bastion-bridge-gpui</string>
<key>CFBundleIconFile</key><string>icon.icns</string>
<key>CFBundleVersion</key><string>$version</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$stage/BastionTerm.app"
cp -R docs "$stage/"
cp LICENSE THIRD-PARTY.md README.md MCP配置与Agent提示词.md assets/OFL.txt assets/LICENSE-GPUI assets/LICENSE-vt100 assets/LICENSE-ICU4X assets/LICENSE-muda assets/browser/LICENSE-fit assets/browser/LICENSE-xterm "$stage/"
case "$arch" in
    arm64) triple=aarch64-apple-darwin ;;
    x86_64) triple=x86_64-apple-darwin ;;
    *) echo "Unsupported architecture: $arch" >&2; exit 1 ;;
esac
python3 scripts/dependency-notices.py "$triple" "$stage/DEPENDENCY-NOTICES.txt"
cp vendor/vte/LICENSE-APACHE "$stage/LICENSE-vte-apache"
cp vendor/vte/LICENSE-MIT "$stage/LICENSE-vte"
python3 - "$stage" "$destination/bastionterm-$version-macos-$arch.zip" <<'PYTHON'
from pathlib import Path
from sys import argv
from zipfile import ZIP_DEFLATED, ZipFile

stage = Path(argv[1])
with ZipFile(argv[2], "w", ZIP_DEFLATED) as archive:
    for item in sorted(stage.rglob("*")):
        if item.is_file():
            archive.write(item, item.relative_to(stage))
PYTHON
