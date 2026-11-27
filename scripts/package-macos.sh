#!/usr/bin/env bash
# Builds gf.app and a .dmg for it into dist/.
#
#   scripts/package-macos.sh [version]   # this Mac's architecture
#
# Apple silicon and Intel are built on their own Macs (see release.yml).
#
# The app is ad-hoc signed, not notarized: a copy downloaded with a browser
# is quarantined and macOS refuses to open it until that is cleared
# (scripts/install.sh downloads with curl, which does not quarantine).
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
NAME=gf
BUNDLE_ID=io.github.tom-kwak.gf
DIST=dist
APP="$DIST/$NAME.app"

ARCH="$(uname -m)"
cargo build --release --locked -p gf-app
BIN="target/release/$NAME"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$NAME"
cp packaging/macos/gf.icns "$APP/Contents/Resources/gf.icns"
cp LICENSE "$APP/Contents/Resources/LICENSE"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleExecutable</key><string>$NAME</string>
  <key>CFBundleIconFile</key><string>gf.icns</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
codesign --force --sign - "$APP"

DMG="$DIST/$NAME-$VERSION-macos-$ARCH.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -quiet -volname "$NAME $VERSION" -srcfolder "$STAGE" \
  -fs HFS+ -format UDZO "$DMG"
echo "$DMG"
