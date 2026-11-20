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

## What's new in 0.0.5

- Switch between flat lists and collapsible folder trees in Working Tree and
  History. Each pane remembers its view. History shows file change types and
  places renamed files under their destination folders.
- Use file checkboxes to stage or unstage changes in Working Tree. Commits
  include the staged changes, including selected lines from partially staged files.
- Pull and Push run directly for routine updates and ask before known risky
  operations. The toolbar disables actions when it knows there is nothing to sync.
- Reopening a conflict keeps your choices while its contents are unchanged.

[Release notes](https://github.com/tom-kwak/GitFast/releases/tag/release-0.0.5)

## Releases

A release is a tag `release-<version>` on a commit in `main`, where
`<version>` matches the workspace version in the tagged commit's `Cargo.toml`.
For example, tag the commit with workspace version `0.0.5`:

```bash
git tag release-0.0.5 && git push origin release-0.0.5
```

Pushing the tag starts the release workflow if the tagged commit is already
in `main`. Otherwise, the workflow picks up the tag when the commit reaches
`main`. After the checks and UI sweep pass, the workflow publishes the
universal `.dmg` and its SHA-256 checksum.
[Release workflow](.github/workflows/release.yml) ·
[GitHub Releases](https://github.com/tom-kwak/GitFast/releases)

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
