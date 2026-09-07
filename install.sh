#!/bin/sh
# bilro installer. Downloads a release binary when one exists for this machine,
# builds from source when it does not, then registers the hooks and the MCP
# server. Safe to run twice: nothing is registered a second time.
set -eu

REPO="brandaodeveloperapp/bilro"
DEST="${BILRO_DIR:-$HOME/.local/bin}"

fail() { printf '\n  %s\n\n' "$1" >&2; exit 1; }
step() { printf '  %s\n' "$1"; }

target_triple() {
  os_name=$(uname -s)
  arch=$(uname -m)
  case "$os_name-$arch" in
    Darwin-arm64)  echo "aarch64-apple-darwin" ;;
    Darwin-x86_64) echo "x86_64-apple-darwin" ;;
    Linux-x86_64)  echo "x86_64-unknown-linux-gnu" ;;
    Linux-aarch64) echo "aarch64-unknown-linux-gnu" ;;
    *) echo "" ;;
  esac
}

download() {
  url="https://github.com/$REPO/releases/latest/download/bilro-$1.tar.gz"
  step "downloading $1"
  tmp=$(mktemp -d)
  if ! curl -fsSL "$url" -o "$tmp/bilro.tar.gz" 2>/dev/null; then
    rm -rf "$tmp"
    return 1
  fi
  tar -xzf "$tmp/bilro.tar.gz" -C "$tmp"
  mkdir -p "$DEST"
  mv "$tmp"/bilro "$DEST/bilro"
  if [ -f "$tmp/bilro-panel" ]; then mv "$tmp/bilro-panel" "$DEST/bilro-panel"; fi
  chmod +x "$DEST/bilro" "$DEST/bilro-panel" 2>/dev/null || true
  rm -rf "$tmp"
  return 0
}

build_from_source() {
  command -v cargo >/dev/null 2>&1 || fail "no prebuilt binary for this machine and no cargo to build one. Install Rust from https://rustup.rs and run again."

  here=$(cd "$(dirname "$0")" 2>/dev/null && pwd)
  if [ -f "$here/Cargo.toml" ] && grep -q '^name = "bilro"' "$here/Cargo.toml" 2>/dev/null; then
    step "building from the checkout this script lives in"
    source_dir="$here"
    cleanup=""
  else
    step "cloning and building (a few minutes the first time)"
    tmp=$(mktemp -d)
    if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
      gh repo clone "$REPO" "$tmp/bilro" -- --depth 1 >/dev/null 2>&1 \
        || fail "could not clone $REPO with gh. Check: gh auth status"
    else
      git clone --depth 1 "https://github.com/$REPO" "$tmp/bilro" >/dev/null 2>&1 \
        || fail "could not clone $REPO. If it is private, install the GitHub CLI and run: gh auth login"
    fi
    source_dir="$tmp/bilro"
    cleanup="$tmp"
  fi

  ( cd "$source_dir" && cargo build --release --quiet 2>/dev/null ) || fail "the build failed. Run it by hand to see why: cd $fonte && cargo build --release"
  mkdir -p "$DEST"
  cp "$source_dir/target/release/bilro" "$DEST/bilro"
  cp "$source_dir/target/release/bilro-panel" "$DEST/bilro-panel" 2>/dev/null || true
  if [ -n "$cleanup" ]; then rm -rf "$cleanup"; fi
}

printf '\n  bilro\n\n'

PLATFORM=$(target_triple)
if [ -n "$PLATFORM" ] && download "$PLATFORM"; then
  step "binary installed"
else
  build_from_source
fi

case ":$PATH:" in
  *":$DEST:"*) ;;
  *) printf '\n  %s is not on your PATH. Add it to your shell:\n    export PATH="%s:$PATH"\n' "$DEST" "$DEST" ;;
esac

step "registering hooks and the MCP server"
"$DEST/bilro" install

printf '  done. Start here:\n\n    bilro doctor      check the installation\n    bilro stats       what it already holds\n    bilro-panel       open the panel\n\n'
