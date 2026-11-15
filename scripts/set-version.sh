#!/usr/bin/env bash
# Sets the workspace version in Cargo.toml, like 0.3.0 or 0.3.0-beta.4.
#
# usage: scripts/set-version.sh VERSION
set -euo pipefail
cd "$(dirname "$0")/.."

version=${1:?usage: scripts/set-version.sh VERSION}
perl -0pi -e "s/(\\[workspace\\.package\\]\\r?\\nversion = \")[^\"]+/\${1}$version/" Cargo.toml
grep -q "^version = \"$version\"" Cargo.toml || { echo "couldn't set the version in Cargo.toml" >&2; exit 1; }
