#!/usr/bin/env bash
# Build a .deb around an already-built aiusg binary.
# Usage: package-deb.sh <binary> <version> <arch> <outdir>
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: package-deb.sh <binary> <version> <arch> <outdir>" >&2
  exit 2
fi

binary=$1
version=$2
arch=$3
outdir=$4

if [[ ! -f $binary ]]; then
  echo "missing binary: $binary" >&2
  exit 1
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

pkg="aiusg_${version}-1_${arch}"
root="$workdir/$pkg"
mkdir -p "$root/DEBIAN" "$root/usr/bin"
cp "$binary" "$root/usr/bin/aiusg"
chmod 755 "$root/usr/bin/aiusg"

size_kb=$(du -sk "$root/usr" | awk '{print $1}')

cat >"$root/DEBIAN/control" <<EOF
Package: aiusg
Version: ${version}-1
Section: utils
Priority: optional
Architecture: ${arch}
Depends: libc6 (>= 2.35), libgcc-s1, libdbus-1-3
Maintainer: Jake Barnby <jakeb994@gmail.com>
Installed-Size: ${size_kb}
Homepage: https://github.com/abnegate/aiusg
Description: Usage limits and reset times across all your AI provider accounts
 Reads how much of each AI subscription is left (Claude, Codex, Gemini,
 Copilot, Grok and Cursor) and prints every account as one table. Also
 serves the same data over MCP so agents can route to an account with
 headroom.
EOF

mkdir -p "$outdir"
dpkg-deb --build --root-owner-group "$root" "$outdir/${pkg}.deb"
