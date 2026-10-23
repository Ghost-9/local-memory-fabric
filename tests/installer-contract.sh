#!/usr/bin/env bash
set -euo pipefail
source "$HOME/.cargo/env"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
PROJECT="$TMP/project"
TOOLS="$TMP/tools"
NO_CARGO="$TMP/no-cargo"
BASE_PATH="/usr/bin:/bin"
mkdir -p "$PROJECT/rust" "$TOOLS" "$NO_CARGO" "$TMP/home"
mkdir -p "$TMP/release-assets"
cp "$REPO/install.sh" "$PROJECT/install.sh"
cp "$REPO/rust/Cargo.toml" "$PROJECT/rust/Cargo.toml"
chmod +x "$PROJECT/install.sh"

cat > "$TOOLS/node" <<'NODE'
#!/bin/sh
if [ "${1:-}" = "-p" ]; then printf '26\n'; exit 0; fi
exit 0
NODE
cat > "$TOOLS/uname" <<'UNAME'
#!/bin/sh
case "${1:-}" in
  -s) printf 'Darwin\n' ;;
  -m) printf 'arm64\n' ;;
  *) exit 1 ;;
esac
UNAME
cat > "$TOOLS/curl" <<'CURL'
#!/bin/sh
printf '%s\n' "$*" >> "$CURL_LOG"
url=""
out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    http*) url="$1"; shift ;;
    *) shift ;;
  esac
done
if [ -z "$out" ]; then exit 2; fi
case "$url" in
  *.sha256) printf '%s  memory-fabric-macos-arm64\n' "$ARTIFACT_SHA" > "$out" ;;
  *) cp "$ARTIFACT" "$out" ;;
esac
CURL
cat > "$TOOLS/cargo" <<'CARGO'
#!/bin/sh
printf '%s\n' "$*" >> "$CARGO_LOG"
version="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)"/\1/p' Cargo.toml | head -1)"
mkdir -p target/release
cat > target/release/memory-fabric <<BIN
#!/bin/sh
printf 'Engram / Local Memory Fabric $version (schema 4)\\n'
BIN
chmod +x target/release/memory-fabric
CARGO
chmod +x "$TOOLS/node" "$TOOLS/uname" "$TOOLS/curl" "$TOOLS/cargo"
cp "$TOOLS/node" "$TOOLS/uname" "$TOOLS/curl" "$NO_CARGO/"

write_binary() {
  local version="$1" target="$2"
  cat > "$target" <<BIN
#!/bin/sh
printf 'Engram / Local Memory Fabric $version (schema 4)\\n'
BIN
  chmod +x "$target"
}

export CARGO_LOG="$TMP/cargo.log" CURL_LOG="$TMP/curl.log"
export MEMORY_FABRIC_HOME="$TMP/state-source"
export HOME="$TMP/home"
PATH="$TOOLS:$BASE_PATH" "$PROJECT/install.sh" install > "$TMP/source.out"
[ -x "$PROJECT/rust/target/release/memory-fabric" ]
grep -q 'building Rust engine' "$TMP/source.out"
grep -q 'build --release --locked --quiet' "$CARGO_LOG"
[ ! -s "$CURL_LOG" ]

VERSION="$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)"/\1/p' "$REPO/rust/Cargo.toml" | head -1)"
write_binary "$VERSION" "$TMP/release-assets/memory-fabric-macos-arm64"
if command -v shasum >/dev/null 2>&1; then
  ARTIFACT_SHA="$(shasum -a 256 "$TMP/release-assets/memory-fabric-macos-arm64" | awk '{print $1}')"
else
  ARTIFACT_SHA="$(sha256sum "$TMP/release-assets/memory-fabric-macos-arm64" | awk '{print $1}')"
fi
export ARTIFACT="$TMP/release-assets/memory-fabric-macos-arm64" ARTIFACT_SHA
export MEMORY_FABRIC_HOME="$TMP/state-download"
rm -f "$PROJECT/rust/target/release/memory-fabric"
PATH="$NO_CARGO:$BASE_PATH" "$PROJECT/install.sh" install > "$TMP/download.out"
grep -q "downloaded Rust engine $VERSION" "$TMP/download.out"

write_binary "0.4.0" "$TMP/release-assets/memory-fabric-macos-arm64"
if command -v shasum >/dev/null 2>&1; then
  ARTIFACT_SHA="$(shasum -a 256 "$TMP/release-assets/memory-fabric-macos-arm64" | awk '{print $1}')"
else
  ARTIFACT_SHA="$(sha256sum "$TMP/release-assets/memory-fabric-macos-arm64" | awk '{print $1}')"
fi
export ARTIFACT_SHA MEMORY_FABRIC_HOME="$TMP/state-mismatch"
rm -f "$PROJECT/rust/target/release/memory-fabric"
if PATH="$NO_CARGO:$BASE_PATH" "$PROJECT/install.sh" install > "$TMP/mismatch.out" 2>&1; then
  echo "installer accepted a prebuilt binary that differs from Cargo.toml" >&2
  exit 1
fi
grep -q 'Rust engine version mismatch' "$TMP/mismatch.out"
grep -q 'Build the matching binary' "$TMP/mismatch.out"

echo "  source build wins over downloads; matching download accepted; stale download rejected"
