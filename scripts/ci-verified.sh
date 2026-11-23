#!/usr/bin/env bash
# Usage: scripts/ci-verified.sh ci|sweep <commit>
#
# Exit 0 when <commit> has the same code (everything except *.md and docs/)
# as a commit that already passed that check, so running it again would
# prove nothing; exit 1 otherwise. Needs full git history, `gh`, GH_TOKEN
# and GITHUB_REPOSITORY. Any failure to look runs up means "not verified".
#
#   ci     fmt / clippy / test: a successful ci.yml run.
#   sweep  UI sweep: a release.yml run whose UI sweep job passed (a run
#          with nothing to release succeeds without sweeping).
set -uo pipefail

kind="${1:?ci or sweep}"
target="$(git rev-parse "${2:?commit}^{commit}")" || exit 1

candidates() {
  case "$kind" in
    ci)
      gh run list --workflow ci.yml --status success --limit 100 \
        --json headSha -q '.[].headSha'
      ;;
    sweep)
      gh run list --workflow release.yml --limit 30 \
        --json databaseId,headSha -q '.[] | "\(.databaseId) \(.headSha)"' |
        while read -r id sha; do
          passed="$(gh api "repos/${GITHUB_REPOSITORY:?}/actions/runs/$id/jobs" \
            -q '[.jobs[] | select((.name | startswith("UI sweep")) and .conclusion == "success")] | length')"
          [ "${passed:-0}" -gt 0 ] && echo "$sha"
        done
      ;;
    *) echo "unknown kind: $kind" >&2; exit 1 ;;
  esac
}

while read -r sha; do
  [ -n "$sha" ] || continue
  git cat-file -e "$sha^{commit}" 2>/dev/null || continue
  if git diff --quiet "$sha" "$target" -- . ':(exclude)*.md' ':(exclude)docs'; then
    echo "$kind already passed on $sha, which has the same code as $target" >&2
    exit 0
  fi
done < <(candidates 2>/dev/null)
exit 1
