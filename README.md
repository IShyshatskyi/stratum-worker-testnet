# stratum-worker-testnet

Public **test-network packaging** stub for **stratum-worker** — the MIT Stratum V2 CPU
worker used to connect to a published HCRN / Heaviest Chain Rule testnet pool.

> **This is not Filament.** Do not look for binaries under
> [`filament-testnet/releases`](https://github.com/IShyshatskyi/filament-testnet/releases).
> Filament-testnet is the light-client packaging repo only.

## Status (2026-10)

| Item | State |
|------|--------|
| Public source tree | **Not synced yet** — this README is a placeholder |
| GitHub Releases / binaries | **None** — download unavailable |
| Pool endpoint | Published per round on the marketing [Testnet](https://heaviest-chain.netlify.app/Testnet.html#mine) page |

When source lands here, the intended flow is: clone → build in an environment you
control → point `POOL_ENDPOINT` / `WALLET_ADDRESS` at the live Stratum URL.

## Planned layout (when synced)

```text
stratum-worker-testnet/
  README.md          ← you are here
  LICENSE            ← MIT
  Cargo.toml
  src/               ← stratum-worker binary
  docs/              ← connect + env vars
```

Illustrative env (values from Testnet when a round is live — do not invent hosts):

```bash
export POOL_ENDPOINT=HOST:3336
export WALLET_ADDRESS=0xYOUR_64_HEX_RECIPIENT
export WORKER_NAME=rig-1
export THREADS=2
# ./target/release/stratum-worker
```

## Relation to other repos

| Repo | Product |
|------|---------|
| [`filament-testnet`](https://github.com/IShyshatskyi/filament-testnet) | MMR light client |
| **`stratum-worker-testnet`** (this repo) | Pool worker |
| [`hcrn-ledger-app`](https://github.com/IShyshatskyi/hcrn-ledger-app) | Ledger Nano app stub |
| Private monorepo | Protocol source of truth; workers may sync from `tools/stratum-worker/` |

## Releases

**GitHub Releases for this product belong only on this repository** once artifacts exist.
Empty Releases here means “no binary drop yet,” not “go look at Filament.”

## Licence

MIT (intended). Licence file will ship with the first source sync.

## Links

- Marketing Downloads: https://heaviest-chain.netlify.app/Downloads.html  
- Live pool / wallet env: https://heaviest-chain.netlify.app/Testnet.html#mine  
