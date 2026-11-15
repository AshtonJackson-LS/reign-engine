#!/usr/bin/env bash
# Opens or refreshes the "Release vX.Y.Z" pull request: the version bump
# and the CHANGELOG.md entry, on the branch release/next. Merging it makes
# the release. Run by .github/workflows/release.yml after plan-release.sh.
#
# usage: scripts/update-release-pr.sh NEXT_VERSION   (reads release-notes.md)
set -euo pipefail
cd "$(dirname "$0")/.."

next=${1:?usage: scripts/update-release-pr.sh NEXT_VERSION}
branch=release/next
title="Release v$next"

# The labels plan-release.sh sorts changes by.
gh label create breaking --color B60205 --description "Breaks projects or scenes: bumps the major version" --force
gh label create feature --color 0E8A16 --description "Something new: bumps the minor version" --force
gh label create fix --color FBCA04 --description "A bug fix" --force
gh label create release --color 5319E7 --description "The release pull request" --force

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git switch -C "$branch"

scripts/set-version.sh "$next"
cargo update --workspace --quiet

entry=$(printf '## v%s (%s)\n\n%s\n' "$next" "$(date -u +%Y-%m-%d)" "$(cat release-notes.md)")
if [ -f CHANGELOG.md ]; then
  { head -n 1 CHANGELOG.md; printf '\n%s\n' "$entry"; tail -n +2 CHANGELOG.md; } >CHANGELOG.new
  mv CHANGELOG.new CHANGELOG.md
else
  printf '# Changelog\n\n%s\n' "$entry" >CHANGELOG.md
fi

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit --quiet -m "$title"
git push --quiet --force origin "$branch"

body=$(printf 'Merging this publishes **v%s** to every Reign Hub.\n\nIt updates by itself as more pull requests are merged. To change what goes in, edit the labels on those pull requests (`breaking`, `feature`, `fix`).\n\n%s\n' "$next" "$(cat release-notes.md)")
if [ -n "$(gh pr list --head "$branch" --state open --json number --jq '.[].number')" ]; then
  gh pr edit "$branch" --title "$title" --body "$body"
else
  gh pr create --head "$branch" --base main --title "$title" --body "$body" --label release
fi

# Pull requests opened by GitHub Actions don't start other workflows on
# their own; CI is started by hand so the checks main requires can pass.
gh workflow run ci.yml --ref "$branch"
