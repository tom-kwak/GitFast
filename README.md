<p align="center">
  <img src="docs/assets/gf-icon.png" alt="gf" width="128" height="128">
</p>

<h1 align="center">gf</h1>

<p align="center">
  <b>gf</b> is short for <b>Git Fast</b>: a Git GUI client for macOS and Windows built to
  be the fastest one, written in Rust with
  <a href="https://github.com/iced-rs/iced">iced</a>.
</p>

<p align="center">
  <a href="https://github.com/tom-kwak/GitFast/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/tom-kwak/GitFast?label=release"></a>
  <img alt="macOS 11+" src="https://img.shields.io/badge/macOS-11%2B-black">
  <img alt="Apple silicon and Intel" src="https://img.shields.io/badge/universal-Apple%20silicon%20%7C%20Intel-black">
  <img alt="Windows 10+" src="https://img.shields.io/badge/Windows-10%2B%20x64-blue">
</p>

> **New in 0.0.8:** gf runs on Windows (install it from PowerShell with one
> line, [Windows](#windows) below), and a **Worktrees view** shows every
> working tree of a repository at a glance ([Features](#features)).

## Install

### macOS

Paste in Terminal (macOS 11+, Apple silicon or Intel):

```bash
curl -fsSL https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.sh | bash
```

It installs the latest release as `/Applications/gf.app` (or `~/Applications`) and
opens without a Gatekeeper prompt. Run it again to update.

gf requires **Git 2.43.1 or later**. Check your version with `git --version`.
Git is available through Xcode Command Line Tools or Homebrew.

### Windows

Available since 0.0.8.

Paste in PowerShell (Windows 10 1809+ or 11, x64):

```powershell
irm https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.ps1 | iex
```

It installs the latest release into `%LOCALAPPDATA%\Programs\gf` for the current
user (no administrator rights), adds it to your `PATH` and the Start menu, and
checks the download against its published SHA-256. Run it again to update.
gf needs [Git for Windows](https://git-scm.com/download/win) 2.43.1 or later; if
git is missing, gf says so on its start screen.

gf is not code-signed yet, so SmartScreen may warn the first time
("More info" → "Run anyway"). To check a manual download, compare
`Get-FileHash gf-windows-x64.zip` with `gf-windows-x64.zip.sha256` from the release.

Shortcuts read `⌘` on macOS and `Ctrl` on Windows (`⌘⇧Z` is `Ctrl+Shift+Z`).

Linux support is planned.

## Features

- **History**: a commit graph drawn in text (`● │ ├─╯`) with branch and tag
  badges. Find a file with `⌘F` to list only the commits that changed it.
- **Working Tree**: stage and unstage with checkboxes, or mark several files
  and stage, unstage, stash or discard them together.
- **Diffs**: syntax highlighting with TextMate grammars. Add your own VS Code
  `*.tmLanguage.json` or `*.sublime-syntax` files to
  `~/Library/Application Support/dev.gf.gf/syntaxes/` (macOS) or
  `%APPDATA%\gf\gf\config\syntaxes\` (Windows).
- **Branches, tags and stashes** in a sidebar tree with a quick filter.
- **Worktrees** (`⌘⇧B`, beside REFS): one card per working tree, for running
  several at once (e.g. one per AI agent). A card shows the branch, what is
  uncommitted, recent activity (`⚡`) and where the tree came from
  (`from develop · 3 ahead · 1 behind`, or `merged`). Right-click to open it in
  a tab, IDE or terminal, compare it with its base, merge it into its base,
  lock or remove it. `+` adds a tree, with a dropdown of existing branches;
  `clean N` removes the merged ones.
- **Blame**: who last changed each line; click a line to find its commit.
- **Conflicts**: keep ours, theirs or both for each conflict block.
- **Interactive rebase**: pick, reword, squash, fixup or drop commits and
  reorder them in a list.
- **Undo**: `⌘Z` / `⌘⇧Z` for the steps gf can undo, with an undo history.
- **Command log** (`⌘J`): every git command gf runs, with its output, exit
  status and duration. The `gf ❯` command line runs any git command.
- **Tabs**: one per repository. Switch with `⌘1`–`⌘9` or `⌃⇥`.

Shortcuts are written for macOS here; on Windows, `⌘` is `Ctrl`.

gf works through your system `git`, and uses
[gitoxide](https://github.com/GitoxideLabs/gitoxide) for some operations,
falling back to `git` when gitoxide cannot handle one.

## What's new in 0.0.8

- **Windows version**: gf runs on Windows 10 1809+ and 11 (x64), installed
  with one PowerShell line ([Windows](#windows)). Shortcuts use Ctrl
  (`Ctrl+Shift+Z`), files open in File Explorer, and text is drawn in
  Cascadia Mono.
- gf finds Git for Windows even when it is not on `PATH`, and the start
  screen says how to get git when it cannot start it.
- The open-repository window takes Windows paths (`C:\src\gf`,
  `\\server\share`).
- **Worktrees view**: see all working trees of a repository as cards and act
  on them from the right-click menu, including merging a tree's branch into
  its base. The base is the branch the tree was made from (gf remembers it),
  not always `main`.
- **Safer undo / redo**: undoing pushed commits warns first, a restore is all
  or nothing, and the redo stack survives a restart.

[Release notes](https://github.com/tom-kwak/GitFast/releases/tag/release-0.0.8)

## Releases

A release is a tag `release-<version>` on a commit in `main`, where
`<version>` matches the workspace version in the tagged commit's `Cargo.toml`.
For example, tag the commit with workspace version `0.0.8`:

```bash
git tag release-0.0.8 && git push origin release-0.0.8
```

Pushing the tag starts the release workflow if the tagged commit is already
in `main`. Otherwise, the workflow picks up the tag when the commit reaches
`main`. After the checks (macOS and Windows) and UI sweep pass, the workflow
publishes the universal `.dmg`, the Windows `.zip` and their SHA-256
checksums. The checks are skipped when CI already passed on the tagged commit
(the usual case: push `develop` and wait for its CI before merging to `main`
and tagging), which saves the 20+ minutes Windows takes to build. The release notes start with
`docs/release-notes/<version>.md` ("What's new", written for each release),
followed by the install steps.
[Release workflow](.github/workflows/release.yml) ·
[GitHub Releases](https://github.com/tom-kwak/GitFast/releases)

## Development

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and [PLAN.md](PLAN.md).
