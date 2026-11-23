#!/usr/bin/env bash
# Installs (or updates) gf.app from the latest GitHub release:
#
#   curl -fsSL https://github.com/tom-kwak/GitFast/releases/latest/download/install.sh | bash
#
# Each release carries its own copy of this script, which installs that
# release (a pre-release too): .../releases/download/release-<version>/install.sh
# The copy in the repository has no release in it and installs the latest one.
#
# One command for both platforms: on macOS it installs gf.app; in Git Bash
# on Windows (which gf needs anyway for Git) it runs install.ps1 for you.
#
# macOS: goes to /Applications, or ~/Applications when that is not writable.
# Downloading with curl leaves the app unquarantined, so the unnotarized
# build opens without Gatekeeper's "cannot be opened" refusal.
set -euo pipefail

REPO=tom-kwak/GitFast
NAME=gf
# The release tag to install; the release workflow fills it in. Empty: the latest.
TAG=

base="https://github.com/$REPO/releases/latest/download"
[[ -z "$TAG" ]] || base="https://github.com/$REPO/releases/download/$TAG"

case "$(uname -s)" in
  Darwin) ;;
  MINGW* | MSYS* | CYGWIN*)
    # Windows: install.ps1 does the work (GF_ZIP_URL / GF_INSTALL_DIR pass through).
    echo "Windows detected: running install.ps1"
    exec powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "irm $base/install.ps1 | iex"
    ;;
  *) echo "gf: no installer for $(uname -s) yet (macOS and Windows only)" >&2; exit 1 ;;
esac

# Every release names its .dmg this way, one per architecture. GF_DMG_URL /
# GF_INSTALL_DIR override where from and where to (for testing a build).
case "$(uname -m)" in
  arm64) arch=arm64 ;;
  x86_64) arch=x86_64 ;;
  *) echo "gf: unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
url="${GF_DMG_URL:-$base/$NAME-macos-$arch.dmg}"

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
