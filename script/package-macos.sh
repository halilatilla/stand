#!/bin/sh
# Build Stand.app for Apple silicon. The icon and About panel come from this bundle.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build --release
dest="${1:-$root/dist/Stand.app}"
rm -rf "$dest"
mkdir -p "$dest/Contents/MacOS" "$dest/Contents/Resources"
cp "$root/target/release/stand" "$dest/Contents/MacOS/stand"
chmod 755 "$dest/Contents/MacOS/stand"
cp "$root/assets/macos/Info.plist" "$dest/Contents/Info.plist"
cp "$root/assets/AppIcon.icns" "$dest/Contents/Resources/AppIcon.icns"
codesign --force --sign - --identifier com.halilatilla.stand "$dest"
echo "$dest"
