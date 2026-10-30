#!/usr/bin/env bash
# Sync stratum-worker public packaging from a local private monorepo checkout.
#
# Copies:  $SHISHA/tools/stratum-worker/  →  this repo root
# Preserves packaging overlays listed in scripts/keep_public.txt.
#
# Usage:
#   ./scripts/sync_stratum_worker_public.sh [--check-only]
#   ./scripts/sync_stratum_worker_public.sh --dry-run [/path/to/shisha]
#   ./scripts/sync_stratum_worker_public.sh --status  [/path/to/shisha]
#   ./scripts/sync_stratum_worker_public.sh --apply   [/path/to/shisha]
#
# Maintainer path: scripts/local/upstream.env (gitignored). See scripts/local/README.md.
# Alias: ./scripts/sync_from_shisha.sh → this script.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ALLOW="$ROOT/scripts/public_allowlist.txt"
DENY="$ROOT/scripts/public_denylist.txt"
CHECK="$ROOT/scripts/check_public_boundary.sh"
KEEP="$ROOT/scripts/keep_public.txt"
LOCAL_DIR="$ROOT/scripts/local"
LOCAL_ENV="$LOCAL_DIR/upstream.env"
UPSTREAM_REL="tools/stratum-worker"

mode="status"
SHISHA=""

load_local_env() {
  [[ -f "$LOCAL_ENV" ]] || return 0
  while IFS= read -r line || [[ -n "${line:-}" ]]; do
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    if [[ "$line" =~ ^SHISHA= ]]; then
      export "$line"
    fi
  done < "$LOCAL_ENV"
}

usage() {
  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

restore_keep_public_overlays() {
  local list="$1"
  local backup_root="$2"
  local rel
  while IFS= read -r rel || [[ -n "${rel:-}" ]]; do
    [[ -z "$rel" || "$rel" =~ ^[[:space:]]*# ]] && continue
    rel="${rel#"${rel%%[![:space:]]*}"}"
    rel="${rel%"${rel##*[![:space:]]}"}"
    [[ -z "$rel" ]] && continue
    if [[ ! -e "$backup_root/$rel" ]]; then
      echo "error: keep-public overlay missing in backup: $rel" >&2
      return 1
    fi
    mkdir -p "$(dirname "$ROOT/$rel")"
    if [[ -d "$backup_root/$rel" ]]; then
      rsync -a "$backup_root/$rel/" "$ROOT/$rel/"
    else
      cp "$backup_root/$rel" "$ROOT/$rel"
    fi
    echo "  restored overlay: $rel"
  done < "$list"
}

report_diff() {
  local upstream="$1"
  echo
  echo "== stratum-worker — file diff vs upstream =="
  if ! command -v diff >/dev/null; then
    echo "  (diff not available)"
    return 0
  fi
  if [[ ! -d "$ROOT/src" ]]; then
    echo "  (no public src/ yet — run --apply)"
    return 0
  fi
  diff -rq --exclude .DS_Store --exclude target \
    "$upstream" "$ROOT" \
    --exclude scripts --exclude .git --exclude README.md --exclude VERSION \
    --exclude .gitignore --exclude LICENSE --exclude target \
    2>/dev/null | sed 's/^/  /' || true
}

apply_sync() {
  local sha="$1"
  local upstream="$2"
  local backup
  backup="$(mktemp -d "${TMPDIR:-/tmp}/sw-public-backup.XXXXXX")"
  echo
  echo "== apply =="
  echo "  backup: $backup"

  mkdir -p "$backup"
  while IFS= read -r rel || [[ -n "${rel:-}" ]]; do
    [[ -z "$rel" || "$rel" =~ ^[[:space:]]*# ]] && continue
    rel="${rel#"${rel%%[![:space:]]*}"}"
    rel="${rel%"${rel##*[![:space:]]}"}"
    [[ -z "$rel" || ! -e "$ROOT/$rel" ]] && continue
    mkdir -p "$backup/$(dirname "$rel")"
    if [[ -d "$ROOT/$rel" ]]; then
      rsync -a "$ROOT/$rel/" "$backup/$rel/"
    else
      cp "$ROOT/$rel" "$backup/$rel"
    fi
  done < "$KEEP"

  cleanup_on_fail() {
    echo "error: apply failed — restoring overlays from backup" >&2
    restore_keep_public_overlays "$KEEP" "$backup" || true
    rm -rf "$backup"
  }

  if ! command -v rsync >/dev/null; then
    echo "error: rsync is required for --apply" >&2
    rm -rf "$backup"
    return 1
  fi

  echo "  rsync $UPSTREAM_REL/ → repo root"
  rsync -a --delete \
    --exclude target --exclude .DS_Store \
    --filter 'P /.git/' \
    --filter 'P /scripts/' \
    --filter 'P /README.md' \
    --filter 'P /VERSION' \
    --filter 'P /.gitignore' \
    --filter 'P /Cargo.lock' \
    --filter 'P /LICENSE' \
    --filter 'P /LICENSE.txt' \
    --filter 'P /DISCLAIMER.md' \
    "$upstream/" "$ROOT/"

  echo "  restore keep-public overlays"
  if ! restore_keep_public_overlays "$KEEP" "$backup"; then
    cleanup_on_fail
    return 1
  fi

  echo "SYNCED_FROM_SHISHA=$sha" > "$ROOT/VERSION"
  echo "  wrote VERSION → SYNCED_FROM_SHISHA=$sha"

  echo "  boundary check"
  if ! "$CHECK"; then
    cleanup_on_fail
    return 1
  fi

  echo "  cargo check (advisory)"
  if (
    cd "$ROOT"
    RUSTFLAGS="${RUSTFLAGS:--Awarnings}" cargo check
  ); then
    echo "  cargo check OK"
  else
    echo "  warning: cargo check failed — inspect deps"
  fi

  rm -rf "$backup"
  echo "apply complete."
}

# ── main ──────────────────────────────────────────────────────────────────────
load_local_env
SHISHA="${SHISHA:-$ROOT/../shisha}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check-only) mode="check-only"; shift ;;
    --dry-run)    mode="dry-run"; shift ;;
    --status)     mode="status"; shift ;;
    --apply)      mode="apply"; shift ;;
    -h|--help)    usage 0 ;;
    --*)          echo "error: unknown option: $1" >&2; usage 1 ;;
    *)            SHISHA="$1"; shift ;;
  esac
done

if [[ "$SHISHA" != /* ]]; then
  SHISHA="$ROOT/$SHISHA"
fi
SHISHA="$(cd "$SHISHA" 2>/dev/null && pwd || echo "$SHISHA")"
UPSTREAM="$SHISHA/$UPSTREAM_REL"

chmod +x "$CHECK" 2>/dev/null || true

echo "== public boundary =="
"$CHECK" || true

if [[ "$mode" == "check-only" ]]; then
  "$CHECK"
  exit $?
fi

pin="(no VERSION)"
[[ -f "$ROOT/VERSION" ]] && pin="$(tr -d '\n' < "$ROOT/VERSION")"
echo
echo "== pin =="
echo "  $pin"
echo
echo "== policy =="
echo "  allow: $ALLOW"
echo "  deny:  $DENY"
echo "  keep-public overlays: $KEEP"
echo "  local maps: $LOCAL_DIR (gitignored; optional)"

if [[ ! -d "$SHISHA" ]]; then
  echo
  echo "error: upstream path not found: $SHISHA" >&2
  echo "       pass a path argument or set SHISHA in scripts/local/upstream.env" >&2
  exit 1
fi
if [[ ! -d "$UPSTREAM" ]]; then
  echo "error: expected $UPSTREAM" >&2
  exit 1
fi

sha="$(cd "$SHISHA" && git rev-parse HEAD)"
echo
echo "== upstream =="
echo "  path: $SHISHA"
echo "  HEAD: $sha"
echo "  copy: $UPSTREAM_REL/ → repo root"

report_diff "$UPSTREAM"

if [[ "$mode" == "dry-run" || "$mode" == "status" ]]; then
  echo
  echo "== status =="
  echo "  No files modified. Pass --apply to rsync $UPSTREAM_REL into this repo."
  exit 0
fi

if [[ "$mode" == "apply" ]]; then
  apply_sync "$sha" "$UPSTREAM"
  echo
  echo "== post-apply diff (should be overlays only) =="
  report_diff "$UPSTREAM"
  exit 0
fi

echo "error: unknown mode: $mode" >&2
exit 1
