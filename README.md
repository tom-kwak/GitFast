<p align="center">
  <img src="docs/assets/gf-icon.png" alt="gf" width="128" height="128">
</p>

<h1 align="center">gf</h1>

<p align="center">
  <b>Git Fast.</b> A Git GUI anyone can pick up in a minute,<br>
  and fast enough that you never wait for it.<br>
  Faster, easier, and clearer to read than any other Git GUI client.
</p>

<p align="center">
  <a href="https://github.com/tom-kwak/GitFast/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/tom-kwak/GitFast?label=release"></a>
  <img alt="macOS 11+" src="https://img.shields.io/badge/macOS-11%2B-black">
  <img alt="Apple silicon and Intel" src="https://img.shields.io/badge/macOS-Apple%20silicon%20%7C%20Intel-black">
  <img alt="Windows 10+" src="https://img.shields.io/badge/Windows-10%2B%20x64-blue">
</p>

## The numbers

**git.git, 83,000 commits, scrolled to the very last one** ([how it is measured](tools/mem-bench/README.md))

| | first gf | gf 0.0.9 |
|---|---:|---:|
| Time to scroll the whole history | 92 s | **18 s** |
| Memory for the whole history | 1,090 MB | **117 MB** |
| Peak memory while scrolling | 2.2 GB | **238 MB** |
| Opening the repository | 18.8 MB | **4.0 MB** |

**Everyday git commands, git CLI → gf** ([benchmark](docs/GIX_MIGRATION.md#5-검증), a 40k-commit repository with submodules)

| | `git` | gf | |
|---|---:|---:|---:|
| status | 44.4 ms | **13.5 ms** | 3.3× |
| changed lines (numstat) | 28.9 ms | **5.5 ms** | 5.3× |
| history, 1,000 commits | 16.7 ms | **5.6 ms** | 3.0× |
| branches | 8.0 ms | **1.1 ms** | 7.3× |
| unpushed count | 13.1 ms | **0.5 ms** | 26× |
| stashes | 5.9 ms | **0.01 ms** | 590× |

**How:** Rust, a GPU-drawn UI ([iced](https://github.com/iced-rs/iced)), and reads
done in process with [gitoxide](https://github.com/GitoxideLabs/gitoxide) instead
of starting `git` each time. gitoxide is used only where its answer is
identical to git's, checked by contract tests; everything else, and every
write, goes through your own `git`.

## Worktrees, made simple

Run several branches side by side (one per AI agent, say) and see them all at once.

- **One card per tree:** branch, uncommitted files, and
  `from develop · 3 ahead · 1 behind`.
- **`+`** makes a tree. Right-click to open it, compare it, or **merge it back**.
- **`clean N`** removes every tree already merged. One click.

## Undo, like in any editor

`⌘Z` takes it back. `⌘⇧Z` brings it again. The undo history lists every step.

- Commits, resets, checkouts, pulls, discarded changes, stashes, branch and
  tag deletes, conflict resolutions, LFS and submodule changes.
- Redo survives a restart, and saved copies survive `git gc`.
- Already pushed? gf warns you before undoing.

So go ahead and try things. A wrong click is one `⌘Z` away from fixed.

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

## Releasing

```bash
git tag release-0.0.9 && git push origin release-0.0.9   # version from Cargo.toml
```

The [release workflow](.github/workflows/release.yml) checks, builds and publishes
the macOS and Windows packages with `docs/release-notes/<version>.md`.
`0.0.9-rc.1` style versions become pre-releases, each with its own `install.sh`.

## Development

[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) · [PLAN.md](PLAN.md)
