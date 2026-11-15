#!/usr/bin/env bash
# Builds the hub and the editor for this machine's architecture and packs
# them as dist/reign-linux-<x64|arm64>.tar.gz: the file users download and
# the hub updates itself from.
#
# usage: scripts/package-linux.sh
# needs: pkg-config libudev-dev libasound2-dev
set -euo pipefail
cd "$(dirname "$0")/.."

case "$(uname -m)" in
  x86_64) arch=x64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) echo "no Reign release for $(uname -m)" >&2; exit 1 ;;
esac

cargo build --release -p reign_hub -p reign_engine

pack=target/linux-pack
rm -rf "$pack"
mkdir -p "$pack" dist
cp target/release/reign-hub target/release/reign-engine LICENSE hub/assets/fonts/*-OFL.txt "$pack/"
tar -czf "dist/reign-linux-$arch.tar.gz" -C "$pack" .
echo "Built dist/reign-linux-$arch.tar.gz"
