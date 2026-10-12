#!/usr/bin/env bash
# Install, update, or remove the Local Memory Fabric pi extension.
#
# Design rules, in order of importance:
#
#   1. UNINSTALL MUST NOT TOUCH MEMORY. The index, proposals and config are the user's data.
#      Removing the code never removes what it indexed. That is why uninstall leaves the state
#      directory alone unless --purge is passed EXPLICITLY, and even then it backs up first.
#   2. Rollback must be possible without a network. Update keeps the previous source tree, so
#      `install.sh rollback` restores it. A tool that reads personal memory needs a way out.
#   3. No silent failure. If the Rust engine cannot be built or downloaded, say so loudly:
#      the extension then falls back to the Python engine, which is ~55x slower.
#
# Usage:
#   ./install.sh              install or update (idempotent)
#   ./install.sh status       show what is installed and which engine is live
#   ./install.sh rollback     restore the previous version
#   ./install.sh uninstall    remove the extension, KEEP memory
#   ./install.sh uninstall --purge   also remove the index (backs it up first)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE="${MEMORY_FABRIC_HOME:-$HOME/.pi/agent/scratch/memory-fabric}"
BACKUP="${STATE}.backup"
EXT_DIR="$HOME/.pi/agent/extensions/local-memory-fabric"
MIN_MAJOR=26   # node --experimental-strip-types
BIN_TARGET="$HERE/rust/target/release/memory-fabric"

say()  { printf '%s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

check_node() {
  command -v node >/dev/null 2>&1 || die "node is required but not on PATH."
  local major; major="$(node -p 'process.versions.node.split(".")[0]')"
  [ "$major" -ge "$MIN_MAJOR" ] || die "node $major found; this extension needs >= $MIN_MAJOR for --experimental-strip-types."
}

download_prebuilt_rust() {
  local os; os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  local arch; arch="$(uname -m)"
  local target=""
  if [ "$os" = "darwin" ] && [ "$arch" = "arm64" ]; then
    target="memory-fabric-macos-arm64"
  elif [ "$os" = "linux" ] && [ "$arch" = "x86_64" ]; then
    target="memory-fabric-linux-x64"
  fi
  [ -z "$target" ] && return 1

  # Check if a dist artifact is already present locally
  if [ -f "$HERE/dist/$target" ] && [ -f "$HERE/dist/$target.sha256" ]; then
    say "  found local prebuilt binary in dist/; verifying checksum..."
    local expected; expected="$(awk '{print $1}' "$HERE/dist/$target.sha256")"
    local actual; actual="$(shasum -a 256 "$HERE/dist/$target" 2>/dev/null | awk '{print $1}' || sha256sum "$HERE/dist/$target" | awk '{print $1}')"
    if [ "$expected" = "$actual" ]; then
      mkdir -p "$(dirname "$BIN_TARGET")"
      cp "$HERE/dist/$target" "$BIN_TARGET"
      chmod +x "$BIN_TARGET"
      say "  installed local prebuilt binary: $BIN_TARGET"
      return 0
    fi
  fi

  local tag="v0.4.0"
  local url="https://github.com/Ghost-9/local-memory-fabric/releases/download/${tag}/${target}"
  local sha_url="${url}.sha256"
  mkdir -p "$(dirname "$BIN_TARGET")"
  local tmp_bin="${BIN_TARGET}.tmp"
  local tmp_sha="${BIN_TARGET}.sha256"

  say "  downloading prebuilt Rust engine (${target}) from GitHub releases (${tag})..."
  if command -v curl >/dev/null 2>&1; then
    if curl -fsSL "$url" -o "$tmp_bin" && curl -fsSL "$sha_url" -o "$tmp_sha" 2>/dev/null; then
      local expected; expected="$(awk '{print $1}' "$tmp_sha")"
      local actual; actual="$(shasum -a 256 "$tmp_bin" 2>/dev/null | awk '{print $1}' || sha256sum "$tmp_bin" | awk '{print $1}')"
      rm -f "$tmp_sha"
      if [ "$expected" != "$actual" ]; then
        warn "checksum mismatch for prebuilt binary (expected $expected, got $actual)"
        rm -f "$tmp_bin"
        return 1
      fi
      mv "$tmp_bin" "$BIN_TARGET"
      chmod +x "$BIN_TARGET"
      say "  prebuilt Rust engine installed and verified: $BIN_TARGET"
      return 0
    fi
  fi
  rm -f "$tmp_bin" "${tmp_sha:-}"
  return 1
}

build_rust() {
  if [ -x "$BIN_TARGET" ]; then
    say "  Rust engine already present: $BIN_TARGET"
    return 0
  fi
  # First try prebuilt download for instant start without cargo
  if download_prebuilt_rust; then
    return 0
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    warn "cargo not found and prebuilt binary download unavailable."
    warn "The extension will fall back to the Python engine (~55x slower per query)."
    warn "Install Rust from https://rustup.rs or place the binary in rust/target/release/ to get the fast path."
    return 1
  fi
  say "  building the Rust engine from source (this takes a minute the first time)..."
  if ( cd "$HERE/rust" && cargo build --release --locked --quiet ); then
    say "  Rust engine built: $BIN_TARGET"
    return 0
  fi
  warn "Rust build FAILED. The extension will fall back to Python (~55x slower)."
  return 1
}

install_dev_toolchain() {
  # Only needed for ./test.sh; the extension itself has no runtime dependency.
  command -v npm >/dev/null 2>&1 || { warn "npm not found; the type-check gate will fail. Run 'npm install' before ./test.sh."; return 1; }
  say "  installing the dev toolchain (type checker, for the gate only)..."
  npm install --silent --no-audit --no-fund || warn "npm install failed; ./test.sh step 9 will fail."
}

do_install() {
  check_node
  say "Installing Local Memory Fabric"
  say "  source : $HERE"
  say "  state  : $STATE  (created if absent; never overwritten)"
  mkdir -p "$STATE"

  # Keep the previous tree so rollback works offline.
  if [ -d "$EXT_DIR" ] && [ "$EXT_DIR" != "$HERE" ]; then
    rm -rf "$BACKUP"
    cp -R "$EXT_DIR" "$BACKUP"
    say "  previous version kept at $BACKUP (rollback available)"
  elif [ -d "$EXT_DIR/.git" ] || [ -f "$EXT_DIR/package.json" ]; then
    # Installing in place over a git checkout: git is the rollback story.
    say "  in-place update: 'git checkout .' or 'git revert' is your rollback."
  fi

  # Never clobber the user's config: merge only if absent.
  if [ ! -f "$STATE/config.json" ]; then
    say "  no existing config; the engine will use its defaults."
  else
    say "  existing config preserved at $STATE/config.json"
  fi

  build_rust || true
  install_dev_toolchain || true

  say ""
  say "Installed. Verify with:  ./test.sh"
  say "Note: memory is NOT touched by install, update or uninstall."
}

do_uninstall() {
  local purge=false
  [ "${1:-}" = "--purge" ] && purge=true
  say "Uninstalling Local Memory Fabric"
  if [ -d "$EXT_DIR" ] && [ "$EXT_DIR" != "$HERE" ]; then
    rm -rf "$EXT_DIR" && say "  removed $EXT_DIR"
  else
    say "  extension directory is this script's own directory; remove it manually."
  fi
  if $purge; then
    local stamp; stamp="$(date +%Y%m%d-%H%M%S)"
    if [ -d "$STATE" ]; then
      mv "$STATE" "${STATE}.purged-${stamp}"
      say "  index and config MOVED to ${STATE}.purged-${stamp} (not deleted)"
      say "  delete that directory yourself when you are sure."
    fi
  else
    say "  memory KEPT at $STATE"
    say "  to remove it too: ./install.sh uninstall --purge  (backs up first)"
  fi
  say ""
  say "Uninstalled. Your memory was not deleted."
}

do_rollback() {
  [ -d "$BACKUP" ] || die "no previous version found at $BACKUP; nothing to roll back to."
  say "Rolling back from $BACKUP"
  [ -d "$EXT_DIR" ] && rm -rf "$EXT_DIR"
  cp -R "$BACKUP" "$EXT_DIR"
  say "  restored. Rebuild the Rust engine if you want the fast path:"
  say "    cd $EXT_DIR/rust && cargo build --release --locked"
}

do_status() {
  say "Local Memory Fabric status"
  say "  source        : $HERE"
  say "  state         : $STATE"
  [ -d "$STATE" ] && say "  state exists  : yes" || say "  state exists  : no (nothing indexed yet)"
  if [ -f "$STATE/config.json" ]; then
    local eff; eff="$(node -p 'try{String(require("fs").readFileSync(process.argv[1],"utf8")).includes("\"enforce_export_policy\": true")}catch(e){"unknown"}' "$STATE/config.json" 2>/dev/null || echo unknown)"
    say "  export policy : enforce_export_policy is effectively $eff (default: off, by decision — see issue #8)"
  fi
  if [ -x "$BIN_TARGET" ]; then
    say "  engine        : RUST (fast, ~55x)"
  else
    warn "  engine        : PYTHON FALLBACK (~55x slower). Build it: cd rust && cargo build --release --locked"
  fi
  say "  type check    : $([ -f "$HERE/node_modules/typescript/bin/tsc" ] && echo 'toolchain present' || echo 'MISSING — run npm install (needed for ./test.sh only)')"
  if command -v python3 >/dev/null 2>&1; then
    say "  index         : $(printf '%s' '{"op":"status"}' | python3 "$HERE/memory_fabric.py" --json-stdin 2>/dev/null || echo 'unknown')"
  fi
}

case "${1:-install}" in
  install)    do_install ;;
  update)     do_install ;;
  uninstall)  do_uninstall "${2:-}" ;;
  rollback)   do_rollback ;;
  status)     do_status ;;
  *)          say "Usage: ./install.sh [install|update|status|rollback|uninstall [--purge]]"; exit 1 ;;
esac
