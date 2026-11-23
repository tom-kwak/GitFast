#!/usr/bin/env bash
# gf memory benchmark. Usage: tools/mem-bench/README.md
#
#   tools/mem-bench/run.sh                 # clone the sample repositories, measure them
#   tools/mem-bench/run.sh ~/src/a ~/src/b # measure these repositories instead
#
# Environment:
#   GF_BENCH_DEEP   commits to scroll the first repository to (default 20000, or `all`)
#   GF_BENCH_DIR    where the sample repositories are cloned (default target/mem-bench/repos)
#   GF_BENCH_CARGO  extra cargo flags, e.g. --release
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

# Git Bash on Windows hands out /c/... paths; Rust needs C:/...
native() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}

if [ $# -eq 0 ]; then
    dir="${GF_BENCH_DIR:-$root/target/mem-bench/repos}"
    mkdir -p "$dir"
    # A very wide graph (git), a large Rust project (cargo), a small one (tokio).
    for r in git/git rust-lang/cargo tokio-rs/tokio; do
        d="$dir/$(basename "$r")"
        if [ ! -d "$d/.git" ]; then
            echo "cloning $r (full history) into $d"
            git clone --quiet "https://github.com/$r" "$d"
        fi
    done
    set -- "$dir/git" "$dir/cargo" "$dir/tokio"
fi

repos=""
for p in "$@"; do
    abs="$(cd "$p" && pwd)"
    repos+="$(native "$abs")"$'\n'
done
export GF_BENCH_REPOS="$repos"

# shellcheck disable=SC2086 # GF_BENCH_CARGO is a list of flags
cargo test -p gf-app --features mem-bench ${GF_BENCH_CARGO:-} --bin gf mem_bench \
    -- --ignored --nocapture --test-threads=1
