#!/usr/bin/env bash
# Off-machine backup of the knowledge base (D2).
#
# Design decisions, and why they are not the obvious ones:
#
#  1. A **git bundle**, not a synced repository. Putting a live .git directory inside a
#     file-sync service invites corruption: git writes objects incrementally and a sync client
#     can copy a half-written pack. A bundle is one immutable file, so a sync client can only
#     ever copy it whole or not at all.
#  2. **Rotated, with a hard size cap.** The operator asked for a copy that stays small. Default
#     is 7 bundles; the cap evicts the oldest regardless of count.
#  3. **One-directional.** Nothing ever reads back from the destination automatically. The live
#     vault stays local and authoritative; restoring is a deliberate act (see --restore).
#  4. The vault itself remains un-synced. This backs up an *artefact derived from* it. The
#     contract's "no sync, no publish" is preserved: no service is given live access to the
#     canonical store, and no client can write to it.
#
# Usage:
#   scripts/vault-backup.sh              # create a bundle, rotate, report
#   scripts/vault-backup.sh --verify     # drill: restore the newest bundle and check it
#   scripts/vault-backup.sh --list       # what exists, with sizes and ages
set -euo pipefail

VAULT="${MEMORY_FABRIC_VAULT:-$HOME/system-memory}"
DEST="${ENGRAM_BACKUP_DIR:-$HOME/Library/Mobile Documents/com~apple~CloudDocs/engram-vault-backup}"
KEEP="${ENGRAM_BACKUP_KEEP:-7}"
CAP_MB="${ENGRAM_BACKUP_CAP_MB:-500}"

command -v git >/dev/null || { echo "git is required" >&2; exit 1; }
[ -d "$VAULT/.git" ] || { echo "vault is not under version control: $VAULT" >&2; exit 1; }
mkdir -p "$DEST"

case "${1:-backup}" in
  --list)
    printf '%-34s %8s  %s\n' FILE SIZE AGE
    for f in "$DEST"/vault-*.bundle; do
      [ -e "$f" ] || continue
      printf '%-34s %8s  %s\n' "$(basename "$f")" "$(du -h "$f" | cut -f1)" \
        "$(date -r "$f" '+%Y-%m-%d %H:%M')"
    done
    echo "total: $(du -sh "$DEST" | cut -f1)   cap ${CAP_MB} MB, keep ${KEEP}"
    ;;

  --verify)
    newest="$(ls -1t "$DEST"/vault-*.bundle 2>/dev/null | head -1 || true)"
    [ -n "$newest" ] || { echo "no bundle to verify" >&2; exit 1; }
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    start=$(date +%s)
    git clone --quiet --no-local "$newest" "$tmp/restored" 2>/dev/null
    elapsed=$(( $(date +%s) - start ))
    in_vault=$(cd "$VAULT" && git ls-files | wc -l | tr -d ' ')
    in_restore=$(cd "$tmp/restored" && git ls-files | wc -l | tr -d ' ')
    # Content check, not just a count: the newest commit must reproduce the same tree hash.
    want=$(cd "$VAULT" && git rev-parse HEAD^{tree})
    got=$(cd "$tmp/restored" && git rev-parse HEAD^{tree})
    echo "RESTORE DRILL"
    echo "  bundle      : $(basename "$newest")"
    echo "  RTO         : ${elapsed}s (clone of a single file)"
    echo "  files       : $in_restore restored vs $in_vault live"
    echo "  tree hash   : $got"
    echo "  matches live: $([ "$want" = "$got" ] && echo YES || echo 'NO — the backup is not a faithful copy')"
    [ "$want" = "$got" ] || exit 1
    # A verifiable sample: does a note actually contain what the live one does?
    sample="$(cd "$VAULT" && git ls-files '*.md' | head -1)"
    if [ -n "$sample" ]; then
      a=$(cd "$VAULT" && git show "HEAD:$sample" | shasum -a 256 | cut -d' ' -f1)
      b=$(cd "$tmp/restored" && git show "HEAD:$sample" | shasum -a 256 | cut -d' ' -f1)
      echo "  sample note : $sample $([ "$a" = "$b" ] && echo 'byte-identical' || echo 'DIFFERS')"
      [ "$a" = "$b" ] || exit 1
    fi
    echo "$(date -u '+%Y-%m-%dT%H:%M:%SZ') rto=${elapsed}s ok" > "$DEST/.last-verify"
    ;;

  *)
    stamp="$(date +%Y-%m-%d)"
    out="$DEST/vault-$stamp.bundle"
    # A bundle of --all carries every ref, so a restore is the whole history, not just HEAD.
    git -C "$VAULT" bundle create "$out.tmp" --all >/dev/null 2>&1
    mv "$out.tmp" "$out"
    git -C "$VAULT" bundle verify "$out" >/dev/null 2>&1
    echo "backup: $(basename "$out")  $(du -h "$out" | cut -f1)"

    # Rotate: by count, then by the size cap, oldest first either way.
    # Portable on macOS bash 3.2, which has no mapfile.
    i=0
    ls -1t "$DEST"/vault-*.bundle 2>/dev/null | while IFS= read -r f; do
      i=$((i + 1))
      if [ "$i" -gt "$KEEP" ]; then rm -f "$f"; echo "  rotated out (count): $(basename "$f")"; fi
    done
    while [ "$(du -sm "$DEST" | cut -f1)" -gt "$CAP_MB" ]; do
      oldest="$(ls -1tr "$DEST"/vault-*.bundle 2>/dev/null | head -1 || true)"
      [ -n "$oldest" ] || break
      rm -f "$oldest"; echo "  rotated out (cap ${CAP_MB} MB): $(basename "$oldest")"
    done
    echo "  destination: ${DEST//$HOME/\~}"
    echo "  total now  : $(du -sh "$DEST" | cut -f1) in $(ls -1 "$DEST"/vault-*.bundle 2>/dev/null | wc -l | tr -d ' ') bundle(s)"
    ;;
esac
