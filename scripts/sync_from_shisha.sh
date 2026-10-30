#!/usr/bin/env bash
# Convenience alias — same as sync_stratum_worker_public.sh
# (Unlike filament-testnet, this alias is active: worker packaging is a single tree.)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/sync_stratum_worker_public.sh" "$@"
