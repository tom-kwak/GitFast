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

## Releases

A release is a tag `release-<version>` on a commit in `main`, where
`<version>` is the workspace version in `Cargo.toml`:

```bash
git tag release-0.0.2
git push origin release-0.0.2
```

Pushing the tag onto a commit `main` already has releases at once; a tag on
a commit that is not in `main` yet (on `develop`) releases when `main` gets
it, e.g. when `develop` is merged. The checks and the UI sweep run on that
commit, then a universal `.dmg` (`scripts/package-macos.sh`) is published as
the GitHub release. A tag whose version does not match `Cargo.toml` fails.
See `.github/workflows/release.yml`.

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
