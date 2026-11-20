# gf

A fast, light Git GUI for macOS, written in Rust with iced.

## Install

Paste in Terminal (macOS 11+, Apple silicon or Intel):

```bash
curl -fsSL https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.sh | bash
```

It installs the latest release as `/Applications/gf.app` (or `~/Applications`) and
opens without a Gatekeeper prompt. Run it again to update. gf needs `git`
(Xcode Command Line Tools or Homebrew).

## What's new in 0.0.4

- Press **⌘F** to find text in file previews and full-file comparisons. Matches
  are highlighted in inline and side-by-side views. Use **⌘G** or **⌘⇧G** to
  jump between matches.
- The welcome screen has a text-art gf logo that follows the active theme.
  The crab moves from side to side.
- The macOS app has a new icon in Finder and the Dock.

[Release notes](https://github.com/tom-kwak/GitFast/releases/tag/release-0.0.4)

## Releases

A release is a tag `release-<version>` on a commit in `main`, where
`<version>` matches the workspace version in the tagged commit's `Cargo.toml`.
For example, tag the commit with workspace version `0.0.4`:

```bash
git tag release-0.0.4 && git push origin release-0.0.4
```

Pushing the tag starts the release workflow if the tagged commit is already
in `main`. Otherwise, the workflow picks up the tag when the commit reaches
`main`. After the checks and UI sweep pass, the workflow publishes the
universal `.dmg` and its SHA-256 checksum.
[Release workflow](.github/workflows/release.yml) ·
[GitHub Releases](https://github.com/tom-kwak/GitFast/releases)

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
