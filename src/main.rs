// tools/stratum-worker/src/main.rs
//
// SP-18 — Cloud PoW Worker (Simulated Stratum V2 Miner)
//
// Standalone binary that connects to a Stratum V2 pool, performs the
// Noise_NX handshake, receives jobs, does real CPU SHA-256d PoW, and
// submits shares.  Designed to run indefinitely on a cloud free-tier CPU.
//
// Configuration (environment variables):
//   POOL_ENDPOINT     — host:port  (default: 127.0.0.1:3333)
//   POOL_PUBKEY       — unused for Noise_NX (server key sent in handshake); kept for NK mode
//   WORKER_NAME       — miner identifier / user_identity (default: "sp18-worker")
//   WALLET_ADDRESS    — optional 0x+64 hex; if set, user_identity becomes
//                       "{WALLET_ADDRESS}.{WORKER_NAME}" so the pool can pay
//                       that wallet when coinbase_pays_worker_wallet is on
//   THREADS           — Rayon thread pool size (default: auto-detect)
//   LOG_INTERVAL_SECS — stats log cadence in seconds (default: 120)

mod connection;
mod job;
mod pow_loop;
mod sv2_codec;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::json;
use tokio::time::sleep;

fn unix_ts_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

struct Config {
    endpoint:          SocketAddr,
    server_pubkey:     [u8; 32],
    worker_name:       String,
    log_interval_secs: u64,
}

impl Config {
    fn from_env() -> Result<Self> {
        let endpoint_str = std::env::var("POOL_ENDPOINT")
            .unwrap_or_else(|_| "127.0.0.1:3333".into());
        let endpoint: SocketAddr = endpoint_str
            .parse()
            .with_context(|| format!("parse POOL_ENDPOINT={endpoint_str}"))?;

        let pubkey_hex = std::env::var("POOL_PUBKEY")
            .unwrap_or_else(|_| "0".repeat(64));
        let pubkey_bytes = hex::decode(&pubkey_hex)
            .with_context(|| "parse POOL_PUBKEY as hex")?;
        if pubkey_bytes.len() != 32 {
            anyhow::bail!("POOL_PUBKEY must be 64 hex chars (32 bytes)");
        }
        let mut server_pubkey = [0u8; 32];
        server_pubkey.copy_from_slice(&pubkey_bytes);

        let label = std::env::var("WORKER_NAME")
            .unwrap_or_else(|_| "sp18-worker".into());
        let worker_name = match std::env::var("WALLET_ADDRESS") {
            Ok(wallet) if !wallet.is_empty() => {
                // Pool auth: wallet before first '.'; rig label after.
                format!("{wallet}.{label}")
            }
            _ => label,
        };

        if let Ok(t) = std::env::var("THREADS") {
            if let Ok(n) = t.parse::<usize>() {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(n)
                    .build_global()
                    .ok();
            }
        }

        let log_interval_secs = std::env::var("LOG_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120u64);

        Ok(Self { endpoint, server_pubkey, worker_name, log_interval_secs })
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

struct Stats {
    hashes_computed: AtomicU64,
    shares_submitted: AtomicU64,
    /// Last Stratum share difficulty (`⌊2^240 / target⌋`).
    share_difficulty: AtomicU64,
}

impl Stats {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            hashes_computed:  AtomicU64::new(0),
            shares_submitted: AtomicU64::new(0),
            share_difficulty: AtomicU64::new(0),
        })
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let cfg   = Config::from_env()?;
    let stats = Stats::new();

    // Stats logging task.
    {
        let stats   = Arc::clone(&stats);
        let cadence = cfg.log_interval_secs;
        tokio::spawn(async move {
            let mut last_hashes = 0u64;
            let mut last_at     = Instant::now();
            loop {
                sleep(Duration::from_secs(cadence)).await;
                let hashes = stats.hashes_computed.load(Ordering::Relaxed);
                let shares = stats.shares_submitted.load(Ordering::Relaxed);
                let share_diff = stats.share_difficulty.load(Ordering::Relaxed);
                let elapsed = last_at.elapsed().as_secs_f64().max(0.001);
                let hashrate = (hashes - last_hashes) as f64 / elapsed;
                last_hashes = hashes;
                last_at     = Instant::now();
                // log::info (not bare println) so env_logger adds the same UTC
                // prefix as Share found / connect lines; `ts` is also in JSON.
                let msg = json!({
                    "ts":               unix_ts_secs(),
                    "hashrate_hps":     hashrate as u64,
                    "shares_submitted": shares,
                    "total_hashes":     hashes,
                    "share_difficulty": share_diff,
                });
                log::info!("{msg}");
            }
        });
    }

    // Reconnect loop with exponential backoff.
    let mut backoff = Duration::from_secs(1);
    loop {
        log::info!(
            "Connecting to {} as worker '{}'",
            cfg.endpoint, cfg.worker_name
        );
        match run_session(&cfg, Arc::clone(&stats)).await {
            Ok(()) => {
                log::info!("Session ended cleanly — reconnecting immediately");
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                log::warn!("Session error: {e:#} — reconnecting in {backoff:?}");
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

async fn run_session(cfg: &Config, stats: Arc<Stats>) -> Result<()> {
    let (mut conn, mut job_watcher) = connection::StratumConnection::connect(
        cfg.endpoint,
        &cfg.server_pubkey,
        &cfg.worker_name,
    )
    .await
    .context("handshake failed")?;

    log::info!(
        "Connected — channel_id={} extranonce_prefix={}",
        conn.channel_id,
        hex::encode(conn.extranonce_prefix)
    );

    let mut job = job_watcher.wait_for_change().await?;
    stats.share_difficulty.store(job.share_difficulty(), Ordering::Relaxed);
    log::info!(
        "First job received — job_id={} share_difficulty={} share_target={} header_bits={:08x}",
        job.job_id,
        job.share_difficulty(),
        job.share_target_hex(),
        job.header_bits,
    );
    let mut last_seen_job_id = job.job_id;
    let mut last_share_target = job.target;

    // Mine in small nonce batches, checking for a fresher job between every
    // batch. Job discovery itself is fully decoupled from this loop (see
    // connection.rs's module doc comment, Aug 16 2026 / NC-77) — a
    // background task owns the socket read side and continuously publishes
    // ready jobs; `job_watcher.poll()` below is a pure in-memory read that
    // can never block on the network, so no batch size chosen here can ever
    // cause this loop itself to miss a job update or stall waiting for one.
    //
    // `3 << 11` = 6,144 nonces: at ~205 KH/s this batch completes in ~30ms —
    // matching the granularity real ASIC hardware (e.g. common Bitmain
    // models) actually restarts/reports on today, so this also makes the
    // soak exercise the same hashrate-loss-to-granularity effect real
    // hardware would see, not just an idealized continuous hasher. (An
    // earlier version of this constant, `1 << 12` = 4,096, was instead sized
    // to `2^(256 - bitlen(max_target))` — the mean attempts against the
    // loosest possible configured target — which also works but doesn't
    // model real hardware's own restart granularity.)
    //
    // If per-batch dispatch overhead (Rayon's `find_map_any` work-stealing
    // setup, `spawn_blocking` dispatch) turns out to meaningfully cut into
    // real hashing throughput at ~33 batches/sec, the fallback is reverting
    // the max_target x16 relaxation (Correctness_and_Validation_Plan.md
    // NC-27's note) back to 2^240 and re-deriving a larger batch size from
    // it — not yet observed to be a problem, but flagged since this is the
    // first live run at this batch size.
    const BATCH: u32 = 3 << 11;
    let mut nonce_start: u32 = 0;

    loop {
        // ── PoW batch (synchronous Rayon) ────────────────────────────────
        let job_snapshot = job.clone();
        let found = tokio::task::spawn_blocking(move || {
            let end = nonce_start.wrapping_add(BATCH);
            let range = if end > nonce_start {
                nonce_start..end
            } else {
                0..BATCH
            };
            // NC-28: `mine_batch` short-circuits on the first match (Rayon
            // `find_map_any`), so it returns the real evaluated count
            // instead of always the full batch size — crediting a fixed
            // BATCH here regardless of outcome silently overstated
            // hashes_computed/total_hashes on every batch that found a
            // share before exhausting its 2^24-nonce range, which this
            // worker's throughput vs. the pool's ~10s VarDiff target makes
            // routine, not rare.
            pow_loop::mine_batch(&job_snapshot, range)
        })
        .await?;

        stats.hashes_computed.fetch_add(found.1, Ordering::Relaxed);
        nonce_start = nonce_start.wrapping_add(BATCH);

        if let Some(share) = found.0 {
            log::info!(
                "Share found! job_id={} nonce=0x{:08x} hash={} \
                 share_difficulty={} share_target={} \
                 header[version={:08x} prev_hash={} commitment_hash={} delta={} header_bits={:08x}] ntime={}",
                job.job_id, share.nonce, hex::encode(share.hash),
                job.share_difficulty(), job.share_target_hex(),
                job.version, hex::encode(job.prev_hash), hex::encode(job.commitment_hash()),
                job.delta, job.header_bits, job.ntime,
            );
            conn.submit_share(job.job_id, share.nonce, job.ntime, job.version)
                .await
                .context("submit_share")?;
            stats.shares_submitted.fetch_add(1, Ordering::Relaxed);
        }

        // ── Non-blocking job check — pure in-memory, cannot stall hashing ─
        if let Some(new_job) = job_watcher.poll()? {
            if new_job.target != last_share_target {
                last_share_target = new_job.target;
                stats.share_difficulty.store(new_job.share_difficulty(), Ordering::Relaxed);
                log::info!(
                    "Share target now share_difficulty={} share_target={}",
                    new_job.share_difficulty(),
                    new_job.share_target_hex(),
                );
            }
            if new_job.job_id != last_seen_job_id {
                last_seen_job_id = new_job.job_id;
                // CB-7 (plan §6.2): only a `clean_jobs=true` update (a real
                // beacon-tip change) makes the in-flight nonce range
                // worthless — the old job's height is provably superseded,
                // so there's nothing to preserve. A `clean_jobs=false`
                // update (the far more common case — `aux_hash` moved
                // because some shard advanced, not the beacon) leaves
                // `prev_mmr_root`/height untouched; the pool's own
                // `JobCache` (NC-32) still accepts a share against the
                // just-superseded job for its own real-time-based TTL, so
                // resetting the nonce search here would throw away real
                // progress for no correctness reason.
                if new_job.clean_jobs {
                    log::info!(
                        "New job received (clean_jobs=true) — job_id={} \
                         share_difficulty={} — discarding in-flight nonce range",
                        new_job.job_id,
                        new_job.share_difficulty(),
                    );
                    nonce_start = 0;
                } else {
                    // Shard aux updates are frequent — keep off the default INFO terminal.
                    log::debug!(
                        "New job received (clean_jobs=false) — job_id={} \
                         share_difficulty={} — keeping nonce search position",
                        new_job.job_id,
                        new_job.share_difficulty(),
                    );
                }
            }
            job = new_job;
        }
    }
}
