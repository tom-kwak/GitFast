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

Every push to `main` runs the checks, builds a universal `.dmg`
(`scripts/package-macos.sh`) and publishes it as a GitHub release tagged
`release-<version>` (the workspace version in `Cargo.toml`; bump it for the
next release, or the tag gets a `-build.<n>` suffix).
See `.github/workflows/release.yml`.

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
