# stratum-worker-testnet

Public **test-network packaging** for **stratum-worker** — the MIT Stratum V2 CPU
worker that connects to a published HCRN / Heaviest Chain Rule pool.

> **This is not Filament.** Do not look for miner binaries under
> [`filament-testnet/releases`](https://github.com/IShyshatskyi/filament-testnet/releases).
> Filament-testnet is the light-client packaging repo only.

## Quick start (test the GitHub Release)

Works on **macOS x86_64** and **Linux x86_64** (Apple Silicon: use Rosetta, a Linux VM, or build from source).

### 1. Get a pool endpoint

- **Public Testnet** (when announced): [Testnet → Mine](https://heaviest-chain.netlify.app/Testnet.html#mine)
- **Developer shadow** (gated): [Devnet → Mine](https://heaviest-chain.netlify.app/Devnet.html#mine) — unlock with the team token; Stratum host:port is listed there

You need a value like `HOST:3336` (TCP). This is **not** an `http://` URL.

### 2. Download the Release binary

```bash
TAG=v0.1.0-testnet-R1
BASE=https://github.com/IShyshatskyi/stratum-worker-testnet/releases/download/$TAG

# macOS Intel (or Rosetta):
curl -fsSL -O "$BASE/stratum-worker-darwin-x86_64"
curl -fsSL -O "$BASE/stratum-worker-darwin-x86_64.sha256"
shasum -a 256 -c stratum-worker-darwin-x86_64.sha256
chmod +x stratum-worker-darwin-x86_64
BIN=./stratum-worker-darwin-x86_64

# Linux x86_64:
# curl -fsSL -O "$BASE/stratum-worker-linux-x86_64"
# curl -fsSL -O "$BASE/stratum-worker-linux-x86_64.sha256"
# sha256sum -c stratum-worker-linux-x86_64.sha256
# chmod +x stratum-worker-linux-x86_64
# BIN=./stratum-worker-linux-x86_64
```

### 3. Configure and run

```bash
# 32-byte recipient (0x + 64 hex). Pool pays this wallet when the round enables
# coinbase_pays_worker_wallet.
export WALLET_ADDRESS=0x$(openssl rand -hex 32)   # or paste your own
export POOL_ENDPOINT=HOST:3336                     # from Testnet/Devnet page
export WORKER_NAME=laptop-1
export THREADS=2                                   # optional; default = all CPUs
export LOG_INTERVAL_SECS=30
export RUST_LOG=info

"$BIN"
```

Healthy startup looks like:

```text
Connecting to HOST:3336 as worker '0x….laptop-1'
Connected — channel_id=…
First job received — job_id=…
{"hashrate_hps":…,"shares_submitted":…,"total_hashes":…}
```

Leave it running. Tip height on the published Keystone/explorer should advance while you hash.

### 4. Build from source (optional)

```bash
git clone https://github.com/IShyshatskyi/stratum-worker-testnet.git
cd stratum-worker-testnet
cargo build --release
export POOL_ENDPOINT=HOST:3336
export WALLET_ADDRESS=0xYOUR_64_HEX_RECIPIENT
export WORKER_NAME=rig-1
./target/release/stratum-worker
```

Prefer building in a VM/container you control if you do not trust a prebuilt binary yet.

## Status

| Item | State |
|------|--------|
| GitHub Releases | [`v0.1.0-testnet-R1`](https://github.com/IShyshatskyi/stratum-worker-testnet/releases/tag/v0.1.0-testnet-R1) — `darwin-x86_64`, `linux-x86_64` (+ `.sha256`) |
| Pool endpoint | [Testnet](https://heaviest-chain.netlify.app/Testnet.html#mine) when a round is live; developer shadow on [Devnet](https://heaviest-chain.netlify.app/Devnet.html#mine) |
| Sync from private monorepo | `./scripts/sync_stratum_worker_public.sh` (maintainers) |

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

## Releases

**GitHub Releases for this product belong only on this repository.**  
Missing assets for your OS means build from source — do **not** take miner binaries from Filament-testnet.

## Relation to other repos

| Repo | Product |
|------|---------|
| [`filament-testnet`](https://github.com/IShyshatskyi/filament-testnet) | MMR light client |
| **`stratum-worker-testnet`** (this repo) | Pool worker |
| [`hcrn-ledger-app`](https://github.com/IShyshatskyi/hcrn-ledger-app) | Ledger Nano app |
| Private monorepo `tools/stratum-worker/` | Upstream source of truth |

## Licence

MIT — see [`LICENSE`](LICENSE).
