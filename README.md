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

## What's new in 0.0.3

- Open full-file comparisons from **Local Changes** and **Commit Changes**.
  Switch between inline and side-by-side views with aligned line numbers and
  change markers in the scrollbar.
- Use **view file** in the **Working Tree** and commit **Files** context menus to
  read the corresponding file version with syntax highlighting.
- Select multiple branches in **Refs** and review the full list before deleting
  them. Local branch deletions can be undone.
- See the last Git command in the collapsed command log and the app version in
  **Settings**.

## Releases

A release is a tag `release-<version>` on a commit in `main`, where
`<version>` matches the workspace version in the tagged commit's `Cargo.toml`.
For example, tag the commit with workspace version `0.0.3`:

```bash
git tag release-0.0.3 && git push origin release-0.0.3
```

Pushing the tag starts the release workflow if the tagged commit is already
in `main`. Otherwise, the workflow picks up the tag when the commit reaches
`main`. After the checks and UI sweep pass, the workflow publishes the
universal `.dmg` and its SHA-256 checksum.
[Release workflow](.github/workflows/release.yml) ·
[GitHub Releases](https://github.com/tom-kwak/GitFast/releases)

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
