#!/usr/bin/env bash
# Fail if the public packaging tree looks like a monorepo leak.
#
# Usage:
#   ./scripts/check_public_boundary.sh
#   ./scripts/check_public_boundary.sh --strict
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DENY_PUBLIC="$ROOT/scripts/public_denylist.txt"
DENY_LOCAL="$ROOT/scripts/local/extra_denylist.txt"
STRICT=0
[[ "${1:-}" == "--strict" ]] && STRICT=1

fail=0
warn=0

# Unexpected top-level entries
while IFS= read -r -d '' entry; do
  name="$(basename "$entry")"
  case "$name" in
    .git|.gitignore|README.md|VERSION|Cargo.toml|Cargo.lock|Dockerfile|LICENSE|LICENSE.txt|DISCLAIMER.md|scripts|src|target|.DS_Store)
      continue ;;
  esac
  echo "error: unexpected top-level entry: $name" >&2
  fail=1
done < <(find "$ROOT" -mindepth 1 -maxdepth 1 \( -type d -o -type f \) -print0)

hit_paths() {
  local pattern="$1"
  local search_roots=()
  [[ -d "$ROOT/src" ]] && search_roots+=("$ROOT/src")
  [[ -f "$ROOT/Dockerfile" ]] && search_roots+=("$ROOT/Dockerfile")
  [[ -f "$ROOT/Cargo.toml" ]] && search_roots+=("$ROOT/Cargo.toml")
  ((${#search_roots[@]} == 0)) && return 0
  find "${search_roots[@]}" \( -type f -o -type d \) -path "*${pattern}*" 2>/dev/null | head -20
}

scan_denylist_file() {
  local file="$1"
  [[ -f "$file" ]] || return 0
  local section="" line pat matches
  while IFS= read -r line || [[ -n "${line:-}" ]]; do
    [[ -z "$line" || "$line" =~ ^[[:space:]]*# ]] && continue
    if [[ "$line" =~ ^\[(paths|cargo|sources)\]$ ]]; then
      section="${BASH_REMATCH[1]}"
      continue
    fi
    pat="${line#"${line%%[![:space:]]*}"}"
    pat="${pat%"${pat##*[![:space:]]}"}"
    [[ -z "$pat" || -z "$section" ]] && continue
    case "$section" in
      paths)
        matches="$(hit_paths "$pat" || true)"
        if [[ -n "$matches" ]]; then
          echo "error: denylist path hit ($file): $pat" >&2
          echo "$matches" | sed 's/^/  /' >&2
          fail=1
        fi
        ;;
      cargo)
        if [[ -f "$ROOT/Cargo.toml" ]] && grep -F -n -- "$pat" "$ROOT/Cargo.toml" >/dev/null 2>&1; then
          echo "error: denylist Cargo.toml hit ($file): $pat" >&2
          fail=1
        fi
        ;;
      sources)
        if [[ -d "$ROOT/src" ]]; then
          matches="$(grep -R -n -F -- "$pat" "$ROOT/src" 2>/dev/null | head -20 || true)"
          if [[ -n "$matches" ]]; then
            if (( STRICT )); then
              echo "error: denylist source hit ($file): $pat" >&2
              fail=1
            else
              echo "warning: denylist source hit ($file): $pat" >&2
              warn=1
            fi
            echo "$matches" | sed 's/^/  /' >&2
          fi
        fi
        ;;
    esac
  done < "$file"
}

scan_denylist_file "$DENY_PUBLIC"
scan_denylist_file "$DENY_LOCAL"

if (( fail != 0 )); then
  echo "public boundary check FAILED" >&2
  exit 1
fi
if (( warn != 0 )); then
  echo "public boundary check OK (with warnings; pass --strict to fail)"
else
  echo "public boundary check OK"
fi
