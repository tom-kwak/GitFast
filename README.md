<p align="center">
  <img src="docs/assets/gf-icon.png" alt="gf" width="128" height="128">
</p>

<h1 align="center">gf</h1>

<p align="center">
  <b>gf</b> is short for <b>Git Fast</b>: a Git GUI client for macOS built to
  be the fastest one, written in Rust with
  <a href="https://github.com/iced-rs/iced">iced</a>.
</p>

<p align="center">
  <a href="https://github.com/tom-kwak/GitFast/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/tom-kwak/GitFast?label=release"></a>
  <img alt="macOS 11+" src="https://img.shields.io/badge/macOS-11%2B-black">
  <img alt="Apple silicon and Intel" src="https://img.shields.io/badge/universal-Apple%20silicon%20%7C%20Intel-black">
</p>

## Install

Paste in Terminal (macOS 11+, Apple silicon or Intel):

```bash
curl -fsSL https://raw.githubusercontent.com/tom-kwak/GitFast/main/scripts/install.sh | bash
```

It installs the latest release as `/Applications/gf.app` (or `~/Applications`) and
opens without a Gatekeeper prompt. Run it again to update.

gf requires **Git 2.43.1 or later**. Check your version with `git --version`.
Git is available through Xcode Command Line Tools or Homebrew.

gf runs on macOS today. Windows and Linux support is planned.

## Features

- **History**: a commit graph drawn in text (`● │ ├─╯`) with branch and tag
  badges. Find a file with `⌘F` to list only the commits that changed it.
- **Working Tree**: stage and unstage with checkboxes, or mark several files
  and stage, unstage, stash or discard them together.
- **Diffs**: syntax highlighting with TextMate grammars. Add your own VS Code
  `*.tmLanguage.json` or `*.sublime-syntax` files to
  `~/Library/Application Support/dev.gf.gf/syntaxes/`.
- **Branches, tags and stashes** in a sidebar tree with a quick filter.
- **Blame**: who last changed each line; click a line to find its commit.
- **Conflicts**: keep ours, theirs or both for each conflict block.
- **Interactive rebase**: pick, reword, squash, fixup or drop commits and
  reorder them in a list.
- **Undo**: `⌘Z` / `⌘⇧Z` for the steps gf can undo, with an undo history.
- **Command log** (`⌘J`): every git command gf runs, with its output, exit
  status and duration. The `gf ❯` command line runs any git command.
- **Tabs**: one per repository. Switch with `⌘1`–`⌘9` or `⌃⇥`.

gf works through your system `git`, and uses
[gitoxide](https://github.com/GitoxideLabs/gitoxide) for some operations,
falling back to `git` when gitoxide cannot handle one.

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
