#!/bin/bash
# Sets the version everywhere, commits, and tags it on this Mac.
# Nothing is pushed: pushing the tag is what makes GitHub build the
# downloadable SimLab.app (.github/workflows/release.yml).
# Usage: scripts/release.sh 0.2.0
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:-}"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: make release VERSION=1.2.3" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first" >&2; exit 1; }
git rev-parse -q --verify "refs/tags/v$V" >/dev/null && { echo "v$V already exists" >&2; exit 1; }

# the one place each part keeps its version
python3 - "$V" <<'PY'
import re,sys; p="Cargo.toml"; s=open(p).read()
open(p,"w").write(re.sub(r'(?m)^version = ".*"', f'version = "{sys.argv[1]}"', s, count=1))
PY
python3 - "$V" <<'PY'
import json,sys
for p in ["app/src-tauri/tauri.conf.json", "web/package.json", "app/package.json"]:
    d=json.load(open(p))
    if "version" in d or p.endswith("tauri.conf.json"):
        d["version"]=sys.argv[1]; open(p,"w").write(json.dumps(d, indent=2, ensure_ascii=False)+"\n")
PY
cargo update --workspace --offline >/dev/null 2>&1 || cargo metadata --format-version 1 >/dev/null
git add -A
git commit -q -m "SimLab $V"
git tag -a "v$V" -m "SimLab $V"
echo "Tagged v$V here. To publish the download, push it:"
echo "  git push origin HEAD v$V"
