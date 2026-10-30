// tools/stratum-worker/src/pow_loop.rs
//
// SP-18 — SHA-256d inner PoW loop (Rayon parallel).
//
// Each call to `mine_batch` searches a range of nonces for a hash that
// meets the target.  The calling code (main.rs) splits the full 2³² nonce
// space across retarget cycles and restarts the search on new jobs.

use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::job::Job;

/// Result of a successful nonce search.
pub struct FoundShare {
    pub nonce:   u32,
    pub hash:    [u8; 32],
}

/// Search `nonce_range` for a valid PoW solution.
///
/// Uses Rayon to split the range across available threads.
/// Returns `(None, evaluated)` when no nonce in the range meets the target
/// (`evaluated == nonce_range.len()`), or `(Some(share), evaluated)` on a
/// hit, where `evaluated` is the **real** number of nonces hashed before the
/// search stopped.
///
/// `find_map_any` short-circuits as soon as any nonce matches — the range
/// is not exhaustively searched on a hit, so a caller that unconditionally
/// credits the full batch size toward a hashrate/attempt-count stat
/// overstates real work whenever a match lands before the batch ends. At
/// this worker's throughput and the pool's ~10s VarDiff share interval, a
/// match inside a single 2^24-nonce batch is common, not rare, so this
/// isn't a rounding error — see NC-28 (`Correctness_and_Validation_Plan.md`).
pub fn mine_batch(job: &Job, nonce_range: std::ops::Range<u32>) -> (Option<FoundShare>, u64) {
    let evaluated = AtomicU64::new(0);
    let result = nonce_range
        .into_par_iter()
        .find_map_any(|nonce| {
            evaluated.fetch_add(1, Ordering::Relaxed);
            let hdr   = job.build_header(nonce);
            let hash  = sha256d(&hdr);
            if job.hash_meets_target(&hash) {
                Some(FoundShare { nonce, hash })
            } else {
                None
            }
        });
    (result, evaluated.load(Ordering::Relaxed))
}

/// SHA-256d: SHA-256(SHA-256(data)).
pub fn sha256d(data: &[u8]) -> [u8; 32] {
    let first  = Sha256::digest(data);
    let second = Sha256::digest(&first);
    second.into()
}
