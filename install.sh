#!/bin/bash
# Builds SimLab on this Mac and puts SimLab.app in /Applications.
# Run once from the repo:   make install   (or ./install.sh)
# Later updates use the same script (SimLab's Settings → Update runs it for you).
#
# Nothing is installed system-wide without asking. Apps built on your own Mac
# need no paid Apple signing: SimLab is signed "ad hoc" for this Mac only.
#
# Options: --yes (answer yes to installs), --no-open (do not open the app after),
#          --build-only (build SimLab.app but do not install it)

set -euo pipefail
cd "$(dirname "$0")"
REPO="$(pwd)"
YES=0
OPEN=1
BUILD_ONLY=0
for a in "$@"; do
  case "$a" in
    --yes) YES=1 ;;
    --no-open) OPEN=0 ;;
    --build-only) BUILD_ONLY=1 ;;
  esac
done

say() { printf '\033[1;38;5;208m▸\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
ask() { # ask "question" → 0 for yes
  if [ "$YES" = 1 ]; then return 0; fi
  printf '%s [y/N] ' "$1"; read -r reply; [[ "$reply" =~ ^[Yy] ]]
}

[ "$(uname -s)" = Darwin ] || fail "SimLab is a Mac app."
[ "$(uname -m)" = arm64 ] || fail "SimLab needs an Apple Silicon Mac (M1 or newer)."
xcode-select -p >/dev/null 2>&1 || fail "Apple's command line tools are missing. Run: xcode-select --install   then run ./install.sh again."

export PATH="/opt/homebrew/bin:/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

need() { # need <command> <brew formula> <why>
  if ! command -v "$1" >/dev/null 2>&1; then
    command -v brew >/dev/null 2>&1 || fail "$1 is missing ($3) and Homebrew is not installed. Install Homebrew from https://brew.sh first."
    if ask "$1 is missing ($3). Install it with Homebrew (brew install $2)?"; then
      brew install "$2"
    else
      fail "SimLab needs $1."
    fi
  fi
}

need cargo rustup "builds the engine"
if ! rustc --version >/dev/null 2>&1; then rustup default stable; fi
need node node "builds the screens"
need pnpm pnpm "builds the screens"
need uv uv "installs the Gmsh mesher safely, with locked hashes"

say "Building the screens"
( cd web && pnpm install --frozen-lockfile --silent && pnpm build >/dev/null )

say "Building the engine (the first time takes a few minutes)"
cargo build --release --locked -p simlab-engine
mkdir -p app/src-tauri/binaries
cp target/release/simlab-engine app/src-tauri/binaries/simlab-engine-aarch64-apple-darwin

say "Building SimLab.app"
cp web/src/styles/tokens.css app/splash/tokens.css   # one source of design truth
( cd app && pnpm install --frozen-lockfile --silent \
  && pnpm tauri icon src-tauri/icons/source.png -o src-tauri/icons >/dev/null \
  && pnpm tauri build --bundles app >/dev/null )
BUILT="app/src-tauri/target/release/bundle/macos/SimLab.app"
[ -d "$BUILT" ] || fail "the app was not built"
if [ "$BUILD_ONLY" = 1 ]; then say "Built: $BUILT"; exit 0; fi

if pgrep -xq SimLab; then
  say "Closing the running SimLab"
  osascript -e 'quit app "SimLab"' >/dev/null 2>&1 || true
  sleep 2
fi
say "Installing to /Applications"
rm -rf /Applications/SimLab.app
cp -R "$BUILT" /Applications/SimLab.app

DATA="$HOME/Library/Application Support/SimLab"
mkdir -p "$DATA"
printf '%s\n' "$REPO" > "$DATA/source_dir"

say "Done. SimLab is in your Applications folder."
echo
echo "  Connect Claude Code to SimLab (optional):"
echo "    claude mcp add simlab -- /Applications/SimLab.app/Contents/MacOS/simlab-engine mcp"
echo
if [ "$OPEN" = 1 ]; then open /Applications/SimLab.app; fi
