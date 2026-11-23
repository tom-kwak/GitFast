<p align="center">
  <img src="docs/assets/gf-icon.png" alt="gf" width="128" height="128">
</p>

<h1 align="center">gf</h1>

<p align="center">
  <b>Git Fast.</b> A Git GUI for macOS and Windows, written in Rust.
</p>

<p align="center">
  <a href="https://github.com/tom-kwak/GitFast/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/tom-kwak/GitFast?label=release"></a>
  <img alt="macOS 11+" src="https://img.shields.io/badge/macOS-11%2B-black">
  <img alt="Apple silicon and Intel" src="https://img.shields.io/badge/macOS-Apple%20silicon%20%7C%20Intel-black">
  <img alt="Windows 10+" src="https://img.shields.io/badge/Windows-10%2B%20x64-blue">
</p>

<p align="center">
  <img src="docs/assets/screenshot.png" alt="gf showing the history graph, a commit's files and its diff" width="800">
</p>

## Install

macOS (Terminal) · Windows (Git Bash):

```bash
curl -fsSL https://github.com/tom-kwak/GitFast/releases/latest/download/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://github.com/tom-kwak/GitFast/releases/latest/download/install.ps1 | iex
```

macOS 11+ (Apple silicon, Intel) · Windows 10+ x64 · Git 2.43.1+. Run again to update.

## What it does

- **History**: a commit graph drawn in text, with branch and tag labels.
  `⌘F` lists only the commits that changed a given file.
- **Working tree**: stage and unstage with checkboxes; stage, stash or
  discard several files at once.
- **Diffs** with syntax highlighting, **blame**, **conflict resolution**
  (ours, theirs or both per block) and **interactive rebase**.
- **Command log** (`⌘J`): every git command gf ran, with output, exit status
  and duration. The `gf ❯` line runs any git command.
- **Tabs**: one per repository.

## Speed: `git` CLI vs. gf

To show a repository, gf reads its status, changed lines, history, branches
and stashes. The `git` CLI starts a new process for each of these; gf reads
them inside its own process with [gitoxide](https://github.com/GitoxideLabs/gitoxide).
The table times the same read both ways.

| Read | `git` CLI | gf | gf is |
|---|---:|---:|---:|
| status | 44.4 ms | 13.5 ms | 3.3× faster |
| changed lines (`numstat`, unstaged) | 28.9 ms | 5.5 ms | 5.3× faster |
| history, 1,000 commits (`log --topo-order`) | 16.7 ms | 5.6 ms | 3.0× faster |
| branches | 8.0 ms | 1.1 ms | 7.3× faster |
| commits not yet pushed | 13.1 ms | 0.5 ms | 26× faster |
| stashes | 5.9 ms | 0.01 ms | 590× faster |

How it was measured: `crates/gf-git/tests/gix_bench.rs`, release build, best of
5 runs, on a repository with 40k commits and submodules. Both ways returned
the same results. Recorded in [docs/GIX_MIGRATION.md §5](docs/GIX_MIGRATION.md#5-검증)
when the gitoxide reads were added; it compares gf with the `git` CLI, not with
other Git GUIs.

Other choices that keep gf responsive:

- gitoxide is used only where contract tests show it returns the same result
  as `git`; anything else, and every write, runs your installed `git`.
- History loads one page at a time, and the graph keeps only what it draws.
- Diffs over 20,000 lines or 1 MiB are drawn only when you ask; diffs over
  32 MiB are refused.

## Worktrees

Several working trees of one repository, for example one per AI agent, in one panel (`⌘⇧B`).

- One card per worktree: branch, uncommitted files, and its relation to the
  branch it came from (`from develop · 3 ahead · 1 behind`, or `merged`).
- `+` adds a worktree; the branch field lists existing branches.
- Right-click a card to open it in a tab, IDE or terminal, compare it with
  its base, or merge it into its base.
- `clean N` removes the worktrees already merged.

## Undo

`⌘Z` undoes the last recorded step and `⌘⇧Z` redoes it. The undo history
lists the steps. Push and fetch are not recorded.

- Recorded steps: commits, resets, checkouts, pulls, discarded changes,
  stashes, branch and tag deletes, conflict resolutions, LFS track/untrack
  and submodule updates.
- The undo and redo stacks are kept across restarts, and saved copies
  survive `git gc`.
- Undoing commits that were already pushed asks first.

## Shortcuts

On Windows, `⌘` is `Ctrl` (`⌘⇧Z` is `Ctrl+Shift+Z`).

| Keys | Action |
|---|---|
| `⌘Z` / `⌘⇧Z` | Undo / redo |
| `⌘↩` | Commit |
| `⌘R` | Refresh |
| `⌘O` | Open a repository |
| `⌘W` | Close the tab |
| `⌘1`–`⌘9` | Go to tab 1–9 |
| `⌘⇧[` / `⌘⇧]` | Previous / next tab |
| `⌃⇥` / `⌃⇧⇥` | Tabs in most-recently-used order |
| `⌘P` | List all tabs |
| `⌘[` / `⌘]` | History / working tree |
| `⌘E` | Switch between history and working tree |
| `⌘B` | Show or hide the refs panel |
| `⌘⇧B` | Refs ⇄ worktrees |
| `⌘⇧H` | Show or hide the history |
| `⌘J` | Show or hide the command log |
| `⌘L` | Go to the `gf ❯` command line |
| `⌘F` | Find a file in the history; find text in a diff or blame |
| `⌘G` / `⌘⇧G` | Next / previous match |
| `⌘C` / `⌘A` | Copy the selected diff lines / select all |
| `⌘,` | Settings |
| `⌘Q` | Quit |

## Releasing

```bash
git tag release-0.0.9 && git push origin release-0.0.9   # version from Cargo.toml
```

The [release workflow](.github/workflows/release.yml) checks, builds and publishes
the macOS and Windows packages with `docs/release-notes/<version>.md`.
`0.0.9-rc.1` style versions become pre-releases, each with its own `install.sh`.

## Development

[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) · [PLAN.md](PLAN.md)
