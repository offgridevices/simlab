#!/bin/bash
# Zips the built SimLab.app for a GitHub release, with a checksum and notes.
# Usage: scripts/package_app.sh v0.2.0 <out dir>
set -euo pipefail
cd "$(dirname "$0")/.."
TAG="$1"; OUT="$2"
APP=app/src-tauri/target/release/bundle/macos/SimLab.app
[ -d "$APP" ] || { echo "build SimLab.app first (make build)" >&2; exit 1; }
mkdir -p "$OUT"
ZIP="SimLab-$TAG-mac-apple-silicon.zip"
rm -f "$OUT/$ZIP"
# ditto keeps the code signature and file attributes intact (zip does not)
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/$ZIP"
( cd "$OUT" && shasum -a 256 "$ZIP" > "$ZIP.sha256" )
sed "s/{{TAG}}/$TAG/g; s/{{ZIP}}/$ZIP/g" scripts/release_notes.md > "$OUT/NOTES.md"
