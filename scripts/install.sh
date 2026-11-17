#!/usr/bin/env bash
# Installs (or updates) gf.app from the latest GitHub release:
#
#   curl -fsSL https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.sh | bash
#
# Goes to /Applications, or ~/Applications when that is not writable.
# Downloading with curl leaves the app unquarantined, so the unnotarized
# build opens without Gatekeeper's "cannot be opened" refusal.
set -euo pipefail

REPO=tom-kwak/GitFast
NAME=gf

[[ "$(uname -s)" == Darwin ]] || { echo "gf: this installer is for macOS" >&2; exit 1; }

# Every release names its .dmg this way. GF_DMG_URL / GF_INSTALL_DIR
# override where from and where to (for testing a build).
url="${GF_DMG_URL:-https://github.com/$REPO/releases/latest/download/$NAME-macos-universal.dmg}"

dest="${GF_INSTALL_DIR:-/Applications}"
[[ -w "$dest" ]] || { dest="$HOME/Applications"; mkdir -p "$dest"; }

tmp="$(mktemp -d)"
mnt="$tmp/mnt"
cleanup() {
  hdiutil detach -quiet "$mnt" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT

echo "Downloading the latest gf"
curl -fL --progress-bar -o "$tmp/$NAME.dmg" "$url"
mkdir -p "$mnt"
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mnt" "$tmp/$NAME.dmg"

if pgrep -xq "$NAME" 2>/dev/null; then
  echo "gf is running: quit it to use the new version."
fi
rm -rf "${dest:?}/$NAME.app"
ditto "$mnt/$NAME.app" "$dest/$NAME.app"
xattr -dr com.apple.quarantine "$dest/$NAME.app" 2>/dev/null || true
echo "Installed $dest/$NAME.app"
