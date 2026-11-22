# gf

A fast, light Git GUI for macOS, written in Rust with iced.

## Install

Paste in Terminal (macOS 11+, Apple silicon or Intel):

```bash
curl -fsSL https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.sh | bash
```

It installs the latest release as `/Applications/gf.app` (or `~/Applications`) and
opens without a Gatekeeper prompt. Run it again to update.

gf requires **Git 2.43.1 or later**. Check your version with `git --version`.
Git is available through Xcode Command Line Tools or Homebrew.

## What's new in 0.0.7

- History can find a file (`⌘F`) and show only the commits that changed it,
  following renames. `×` or Esc returns to the previous view. Commits not yet
  pushed are marked with the Push arrow.
- Mark several working tree files with ⌘-click, ⇧-click, ⇧↑/↓ or ⌘A, then
  stage, unstage, stash or discard them together from one menu.
- The Commit button shows how many staged files it commits, the FILES view
  switches with one ≡ toggle, and folders are dimmed with a folder icon.
- Switching branch with changes asks one short question: stash & switch (`s`,
  ⏎) or bring changes (`c`).

[Release notes](https://github.com/tom-kwak/GitFast/releases/tag/release-0.0.7)

## Releases

A release is a tag `release-<version>` on a commit in `main`, where
`<version>` matches the workspace version in the tagged commit's `Cargo.toml`.
For example, tag the commit with workspace version `0.0.7`:

```bash
git tag release-0.0.7 && git push origin release-0.0.7
```

Pushing the tag starts the release workflow if the tagged commit is already
in `main`. Otherwise, the workflow picks up the tag when the commit reaches
`main`. After the checks and UI sweep pass, the workflow publishes the
universal `.dmg` and its SHA-256 checksum.
[Release workflow](.github/workflows/release.yml) ·
[GitHub Releases](https://github.com/tom-kwak/GitFast/releases)

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
