# stratum-worker-testnet

Public **test-network packaging** for **stratum-worker** — the MIT Stratum V2 CPU
worker used to connect to a published HCRN / Heaviest Chain Rule testnet pool.

> **This is not Filament.** Do not look for binaries under
> [`filament-testnet/releases`](https://github.com/IShyshatskyi/filament-testnet/releases).
> Filament-testnet is the light-client packaging repo only.

## Status

| Item | State |
|------|--------|
| Sync from private monorepo | `./scripts/sync_stratum_worker_public.sh` |
| GitHub Releases / binaries | **None** until a tagged release is cut here |
| Pool endpoint | [Testnet](https://heaviest-chain.netlify.app/Testnet.html#mine) when a round is live |

## Sync from the private monorepo (maintainers)

Same pattern as [`filament-testnet`](https://github.com/IShyshatskyi/filament-testnet)
`scripts/sync_filament_public.sh`: run **inside this public clone**, point at a local
shisha checkout that contains `tools/stratum-worker/`.

```bash
cp scripts/local/upstream.env.example scripts/local/upstream.env
# set SHISHA=/absolute/path/to/shisha

./scripts/sync_stratum_worker_public.sh --status
./scripts/sync_stratum_worker_public.sh --dry-run
./scripts/sync_stratum_worker_public.sh --apply

./scripts/check_public_boundary.sh
```

`sync_from_shisha.sh` is an alias for the same script.

What `--apply` does:

1. `rsync` `$SHISHA/tools/stratum-worker/` → this repo root  
2. Restores packaging overlays (`README.md`, `scripts/`, `LICENSE`, …) from `keep_public.txt`  
3. Writes `VERSION` (`SYNCED_FROM_SHISHA=<git sha>`)  
4. Runs `check_public_boundary.sh` and an advisory `cargo check`

## Build (after sync)

```bash
cargo build --release
export POOL_ENDPOINT=HOST:3336          # from Testnet
export WALLET_ADDRESS=0xYOUR_64_HEX_RECIPIENT
export WORKER_NAME=rig-1
./target/release/stratum-worker
```

Prefer building in an environment you control if you do not trust the tree yet.

## Releases

**GitHub Releases for this product belong only on this repository.**  
Empty Releases means “no binary drop yet,” not “go look at Filament.”

## Relation to other repos

| Repo | Product |
|------|---------|
| [`filament-testnet`](https://github.com/IShyshatskyi/filament-testnet) | MMR light client |
| **`stratum-worker-testnet`** (this repo) | Pool worker |
| [`hcrn-ledger-app`](https://github.com/IShyshatskyi/hcrn-ledger-app) | Ledger Nano app |
| Private monorepo `tools/stratum-worker/` | Upstream source of truth |

## Licence

MIT — see [`LICENSE`](LICENSE).
