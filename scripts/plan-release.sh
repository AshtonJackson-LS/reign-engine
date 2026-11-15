#!/usr/bin/env bash
# Decides what a push to main releases, for .github/workflows/release.yml.
#
#   stable  Cargo.toml has a version with no tag yet (a Release PR was just
#           merged): release it.
#   beta    otherwise, if anything changed since the last release: a beta of
#           the next version, like 0.3.0-beta.4 (the 4th change since v0.2.0).
#   none    nothing changed since the last release.
#
# The next version comes from the labels of the pull requests merged since
# the last release: `breaking` bumps the major number, `feature` the minor,
# anything else the patch.
#
# Prints mode=, version= and next= lines (the workflow appends them to
# $GITHUB_OUTPUT) and writes the changes to release-notes.md.
# Needs the full git history with tags, and `gh` logged in (GH_TOKEN).
set -euo pipefail
cd "$(dirname "$0")/.."

current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
last="v$current"

if ! git rev-parse -q --verify "refs/tags/$last" >/dev/null; then
  # The Release PR wrote these notes into CHANGELOG.md.
  awk -v heading="## $last" '
    index($0, heading) == 1 { found = 1; next }
    found && /^## / { exit }
    found { print }
  ' CHANGELOG.md >release-notes.md 2>/dev/null || true
  printf 'mode=stable\nversion=%s\nnext=%s\n' "$current" "$current"
  exit 0
fi

count=$(git rev-list --first-parent --count "$last..HEAD")
if [ "$count" -eq 0 ]; then
  echo "mode=none"
  exit 0
fi

# Sort the changes since the last release by their pull request's labels.
breaking="" features="" fixes="" other=""
while IFS= read -r subject; do
  pr=$(grep -o '#[0-9]\+' <<<"$subject" | tail -n 1 | tr -d '#' || true)
  if [ -n "$pr" ] && info=$(gh pr view "$pr" --json title,labels \
      --jq '[.title, ([.labels[].name] | join(","))] | @tsv' 2>/dev/null); then
    title=${info%%$'\t'*}
    labels=,${info#*$'\t'},
    line="- $title (#$pr)"
  else
    labels=""
    line="- $subject"
  fi
  case "$labels" in
    *,release,*) continue ;;
    *,breaking,*) breaking+="$line"$'\n' ;;
    *,feature,*) features+="$line"$'\n' ;;
    *,fix,*) fixes+="$line"$'\n' ;;
    *) other+="$line"$'\n' ;;
  esac
done < <(git log --first-parent --format=%s "$last..HEAD")

IFS=. read -r major minor patch <<<"$current"
if [ -n "$breaking" ]; then
  next="$((major + 1)).0.0"
elif [ -n "$features" ]; then
  next="$major.$((minor + 1)).0"
else
  next="$major.$minor.$((patch + 1))"
fi

section() {
  if [ -n "$2" ]; then printf '### %s\n\n%s\n' "$1" "$2"; fi
}
{
  section "Breaking changes" "$breaking"
  section "Features" "$features"
  section "Fixes" "$fixes"
  section "Other changes" "$other"
} >release-notes.md

printf 'mode=beta\nversion=%s-beta.%s\nnext=%s\n' "$next" "$count" "$next"
