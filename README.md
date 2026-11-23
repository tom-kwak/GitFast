<p align="center">
  <img src="docs/assets/gf-icon.png" alt="gf" width="128" height="128">
</p>

<h1 align="center">gf</h1>

<p align="center">
  <b>gf</b> is short for <b>Git Fast</b>: a Git GUI for macOS and Windows that stays
  fast on huge repositories. Written in Rust with
  <a href="https://github.com/iced-rs/iced">iced</a>.
</p>

<p align="center">
  <a href="https://github.com/tom-kwak/GitFast/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/tom-kwak/GitFast?label=release"></a>
  <img alt="macOS 11+" src="https://img.shields.io/badge/macOS-11%2B-black">
  <img alt="Apple silicon and Intel" src="https://img.shields.io/badge/macOS-Apple%20silicon%20%7C%20Intel-black">
  <img alt="Windows 10+" src="https://img.shields.io/badge/Windows-10%2B%20x64-blue">
</p>

## Why gf

- **Fast on big repositories.** The full history of git.git (83k commits) scrolls
  to the end in 18 s using 135 MB. Huge diffs wait until you ask for them, so
  they never freeze the window.
- **Made for the keyboard.** Every action has a shortcut, and a `gf ❯` command
  line runs any git command. Looks like a terminal, works like a GUI.
- **Nothing hidden.** gf runs your own `git`, and the command log (`⌘J`) shows
  every command it ran, with output, exit status and time.
- **Made for parallel work.** The worktrees panel shows one card per working
  tree (branch, uncommitted changes, ahead/behind), so you can see at a glance
  what each of your AI agents is doing.
- **Safe to try things.** `⌘Z` undoes commits, resets and other steps, with an
  undo history.

Also: a text commit graph with file history, staging by checkbox, blame,
conflict resolution, interactive rebase, syntax-highlighted diffs and one tab
per repository.

## Install

macOS (Terminal) and Windows (Git Bash):

```bash
curl -fsSL https://github.com/tom-kwak/GitFast/releases/latest/download/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://github.com/tom-kwak/GitFast/releases/latest/download/install.ps1 | iex
```

Run it again to update. Needs macOS 11+ or Windows 10+ (x64), and **Git 2.43.1+**.
Prefer a manual download? See [Releases](https://github.com/tom-kwak/GitFast/releases).
gf is not code-signed yet, so Windows SmartScreen may warn once ("More info" →
"Run anyway"). Linux is planned.

To try a pre-release (`0.0.9-rc.1`), use the `install.sh` / `install.ps1` from
its release page, for example
`.../releases/download/release-0.0.9-rc.1/install.sh`.

## Releasing

Tag a commit in `main` with `release-<version>` (the version in `Cargo.toml`):

```bash
git tag release-0.0.9 && git push origin release-0.0.9
```

The [release workflow](.github/workflows/release.yml) runs the checks and the
UI sweep (skipped when they already passed on the same code), builds the macOS
(Apple silicon, Intel) and Windows packages and publishes them with the notes
from `docs/release-notes/<version>.md`. A version with a suffix (`0.0.9-rc.1`)
is a pre-release.

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
