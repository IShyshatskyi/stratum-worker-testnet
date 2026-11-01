// tools/stratum-worker/src/job.rs
//
// SP-18 / CB-7 (Stratum_Extranonce_Binding_Plan.md §6) — Job state:
// NewMiningJob + SetNewPrevHash handling.
//
// CB-4 widened `NewMiningJob`'s wire shape from a single fixed 32-byte
// `merkle_root` to `coinbase_prefix`/`coinbase_suffix` (the `coinb1`/`coinb2`
// split around the extranonce splice point, CB-9) plus `aux_hash` (the
// one-entry `merkle_branch` sibling) and a `clean_jobs` flag. This module is
// this crate's own independent parser/builder for that format — deliberately
// separate from the pool's `StrV2Reader`/`NewMiningJob` (this crate has zero
// dependency on `common-types`/`shisha-core`, see the plan's finding in
// §7) — kept in lockstep with `src/mining_pool/stratum/v2/messages.rs`'s
// encode by hand, verified via a shared byte fixture (CB-T3/CB-T9), not a
// shared function call.

use crate::pow_loop::sha256d;

/// Current mining job from the pool.
#[derive(Debug, Clone, Default)]
pub struct Job {
    pub job_id:    u32,
    pub version:   u32,
    pub prev_hash: [u8; 32],
    /// `coinb1` — fixed bytes before the extranonce splice point (CB-9).
    pub coinbase_prefix: Vec<u8>,
    /// `coinb2` — fixed bytes after the extranonce splice point (CB-9).
    pub coinbase_suffix: Vec<u8>,
    /// The one-entry `merkle_branch` sibling (plan §3.1) — opaque to this
    /// worker; folded into `commitment_hash` via `SHA256d(coinbase_hash ‖
    /// aux_hash)`.
    pub aux_hash: [u8; 32],
    /// `true`: this job's own `NewMiningJob` supersedes all prior in-flight
    /// work (a real beacon-tip change). `false`: only `aux_hash` moved —
    /// safe to finish the current nonce batch before switching (plan §6.2).
    /// Not yet consumed by the mining loop's own scheduling (still resets
    /// eagerly, see `main.rs`) — parsed and available for that follow-up.
    pub clean_jobs: bool,
    pub target:    [u8; 32],
    /// Pool-assigned extranonce1 (4 bytes LE), fixed for the session.
    pub extranonce_prefix: [u8; 4],
    /// Miner-rolled extranonce2 — always `0` until local rolling is
    /// implemented (plan §6.1/§6.2); wired through `build_header` already so
    /// that landing real rolling later doesn't need another header-path change.
    pub extranonce2: u32,
    /// Unix ntime from SetNewPrevHash (for share submit + roll bounds).
    pub ntime:     u32,
    /// Opaque consensus header work field (compact rBits) from SetNewPrevHash.
    pub header_bits: u32,
    /// Beacon delta (1/256 s) — bytes [68..72] of the 80-byte PoW header.
    pub delta:     i32,
    /// `job_id` carried by the most recent `SetNewPrevHash` — the job this
    /// prev_hash/ntime/header_bits/delta snapshot actually belongs to.
    prev_hash_job_id: u32,
    has_job_id:    bool,
    has_prev_hash: bool,
    /// `job_id` of the last job the caller was told is ready (via
    /// `take_if_newly_ready`). `None` until the first job ever completes.
    /// See that method's own doc comment for why this exists.
    last_notified_job_id: Option<u32>,
}

impl Job {
    pub fn has_prev_hash(&self) -> bool { self.has_prev_hash }
    pub fn has_job_id(&self)    -> bool { self.has_job_id }

    /// True once we hold a `prev_hash`/`delta`/`header_bits` snapshot that was
    /// actually announced *for this job's own `job_id`* — not merely a
    /// stale snapshot left over from a previous job.
    ///
    /// A bare `has_prev_hash()` is not sufficient: it is a sticky flag that,
    /// once set by an earlier job's `SetNewPrevHash`, stays true forever —
    /// so a `NewMiningJob` arriving without its own paired `SetNewPrevHash`
    /// (e.g. a heartbeat re-broadcast racing the tip-change broadcast) would
    /// be accepted as "ready" while mining against the *previous* job's
    /// prev_mmr_root/delta, producing a header the pool can never validate.
    pub fn is_ready(&self) -> bool {
        self.has_job_id && self.has_prev_hash && self.prev_hash_job_id == self.job_id
    }

    /// True the first time `self` becomes ready *for a `job_id` the caller
    /// hasn't already been told about* — false on every subsequent call for
    /// the same `job_id`, even though `is_ready()` itself stays true.
    ///
    /// `JobBroadcaster` re-sends the pool's current job on every ~60ms
    /// heartbeat tick, not just on a real tip change (`new_prev_hash=false`
    /// re-broadcasts). Each such re-broadcast is a genuine `NewMiningJob`
    /// frame carrying the *same* `job_id`/fields as before — merging it is a
    /// no-op, but plain `is_ready()` can't tell "still ready" from "just
    /// became ready," so a caller that resets its nonce search on `true`
    /// (matching `poll_job_updates`'/`submit_share`'s own contract) would
    /// reset on every single heartbeat repeat. Under a near-trivial PoW
    /// target that repeat rate can be every share submission, trapping the
    /// worker mining nonce 0 over and over and never making real progress —
    /// found via the E3 crash-recovery dry-run once `submit_share`'s own
    /// drain loop (which runs once per share, not once per batch) started
    /// hitting this far more often than `poll_job_updates` ever did.
    pub fn take_if_newly_ready(&mut self) -> bool {
        if !self.is_ready() {
            return false;
        }
        if self.last_notified_job_id == Some(self.job_id) {
            return false;
        }
        self.last_notified_job_id = Some(self.job_id);
        true
    }

    /// Merge a `NewMiningJob` payload into this job (keeps prev_hash / target).
    pub fn merge_new_mining_job(
        &mut self,
        data: &[u8],
        extranonce_prefix: &[u8; 4],
    ) -> anyhow::Result<()> {
        let j = Self::from_new_mining_job(data, extranonce_prefix)?;
        self.job_id = j.job_id;
        self.version = j.version;
        self.coinbase_prefix = j.coinbase_prefix;
        self.coinbase_suffix = j.coinbase_suffix;
        self.aux_hash = j.aux_hash;
        self.clean_jobs = j.clean_jobs;
        self.extranonce_prefix = j.extranonce_prefix;
        self.has_job_id = true;
        Ok(())
    }

    /// Parse a `NewMiningJob` payload (CB-4 wire shape).
    ///
    /// Layout: channel_id(u32) + job_id(u32) + future_job(u8) + version(u32)
    ///         + version_rolling_allowed(u8) + coinbase_prefix(1-byte len + N)
    ///         + coinbase_suffix(1-byte len + M) + aux_hash(32) + clean_jobs(u8)
    pub fn from_new_mining_job(
        data:             &[u8],
        extranonce_prefix: &[u8; 4],
    ) -> anyhow::Result<Self> {
        let mut pos = 0usize;

        let take = |data: &[u8], pos: &mut usize, n: usize| -> anyhow::Result<Vec<u8>> {
            if *pos + n > data.len() {
                anyhow::bail!(
                    "NewMiningJob payload truncated: need {} bytes at offset {}, have {}",
                    n, pos, data.len()
                );
            }
            let out = data[*pos..*pos + n].to_vec();
            *pos += n;
            Ok(out)
        };

        let _channel_id = u32::from_le_bytes(take(data, &mut pos, 4)?.try_into().unwrap());
        let job_id      = u32::from_le_bytes(take(data, &mut pos, 4)?.try_into().unwrap());
        let _future_job = take(data, &mut pos, 1)?[0] != 0;
        let version     = u32::from_le_bytes(take(data, &mut pos, 4)?.try_into().unwrap());
        let _version_rolling_allowed = take(data, &mut pos, 1)?[0] != 0;

        let prefix_len = take(data, &mut pos, 1)?[0] as usize;
        let coinbase_prefix = take(data, &mut pos, prefix_len)?;

        let suffix_len = take(data, &mut pos, 1)?[0] as usize;
        let coinbase_suffix = take(data, &mut pos, suffix_len)?;

        let aux_hash_bytes = take(data, &mut pos, 32)?;
        let mut aux_hash = [0u8; 32];
        aux_hash.copy_from_slice(&aux_hash_bytes);

        let clean_jobs = take(data, &mut pos, 1)?[0] != 0;

        let mut job = Self::default();
        job.job_id            = job_id;
        job.version           = version;
        job.coinbase_prefix   = coinbase_prefix;
        job.coinbase_suffix   = coinbase_suffix;
        job.aux_hash          = aux_hash;
        job.clean_jobs        = clean_jobs;
        job.extranonce_prefix = *extranonce_prefix;
        job.extranonce2       = 0;
        job.has_job_id        = true;
        // All-ones target until SetTarget arrives.
        job.target = [0xFF; 32];
        Ok(job)
    }

    /// Apply a `SetNewPrevHash` payload.
    /// Layout: channel_id(u32) + job_id(u32) + prev_hash(32) + min_ntime(u32)
    ///         + header_bits(u32) + delta(i32)
    pub fn set_prev_hash_from_msg(&mut self, data: &[u8]) {
        if data.len() < 52 {
            return;
        }
        // The pool tags SetNewPrevHash with the same wire job_id as the
        // NewMiningJob it belongs to (see job_broadcaster.rs) — record it so
        // `is_ready()` can verify this prev_hash actually corresponds to the
        // job currently held, instead of trusting a sticky "we've seen *some*
        // prev_hash at some point" flag.
        self.prev_hash_job_id = u32::from_le_bytes(data[4..8].try_into().unwrap());
        self.prev_hash.copy_from_slice(&data[8..40]);
        self.ntime = u32::from_le_bytes(data[40..44].try_into().unwrap());
        self.header_bits = u32::from_le_bytes(data[44..48].try_into().unwrap());
        self.delta = i32::from_le_bytes(data[48..52].try_into().unwrap());
        self.has_prev_hash = true;
    }

    /// Build the mining-facing coinbase bytes: `coinb1 ‖ extranonce1 ‖
    /// extranonce2 ‖ coinb2` (plan §3.3/§6.1).
    fn coinbase_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            self.coinbase_prefix.len() + 8 + self.coinbase_suffix.len(),
        );
        out.extend_from_slice(&self.coinbase_prefix);
        out.extend_from_slice(&self.extranonce_prefix);
        out.extend_from_slice(&self.extranonce2.to_le_bytes());
        out.extend_from_slice(&self.coinbase_suffix);
        out
    }

    /// `coinbase_hash` — leaf 0 of the two-leaf SHA256d commitment (plan §3.1).
    /// Recomputed from `coinbase_prefix`/`coinbase_suffix`/`extranonce_prefix`/
    /// `extranonce2` every call; cheap relative to the nonce search that
    /// follows it, and only needs to change when `extranonce2` rolls or a new
    /// job arrives — no caching implemented yet (correctness first).
    pub fn coinbase_hash(&self) -> [u8; 32] {
        sha256d(&self.coinbase_bytes())
    }

    /// `commitment_hash` — the parent of the two-leaf tree, `SHA256d(coinbase_hash
    /// ‖ aux_hash)`, placed into the header's merkle-root slot.
    pub fn commitment_hash(&self) -> [u8; 32] {
        let coinbase_hash = self.coinbase_hash();
        let mut preimage = [0u8; 64];
        preimage[0..32].copy_from_slice(&coinbase_hash);
        preimage[32..64].copy_from_slice(&self.aux_hash);
        sha256d(&preimage)
    }

    /// Build the 80-byte SHA-256d miner header for a given `nonce`.
    ///
    /// Matches `BeaconBlockCandidateHeader::serialize_for_mining`:
    ///   version(4) + prev_mmr_root(32) + commitment_hash(32) + delta(4) + bits(4) + nonce(4)
    ///
    /// The 80-byte layout itself is unchanged by CB-4 (plan §1.1) — only how
    /// `commitment_hash` gets computed changed (§3.1), from a single wire
    /// value to this two-leaf combine.
    pub fn build_header(&self, nonce: u32) -> [u8; 80] {
        let mut hdr = [0u8; 80];
        hdr[0..4].copy_from_slice(&self.version.to_le_bytes());
        hdr[4..36].copy_from_slice(&self.prev_hash);
        hdr[36..68].copy_from_slice(&self.commitment_hash());
        hdr[68..72].copy_from_slice(&self.delta.to_le_bytes());
        hdr[72..76].copy_from_slice(&self.header_bits.to_le_bytes());
        hdr[76..80].copy_from_slice(&nonce.to_le_bytes());
        hdr
    }

    /// True when `hash < target` (big-endian byte order, same as the pool server).
    pub fn hash_meets_target(&self, hash: &[u8; 32]) -> bool {
        for i in 0..32 {
            if hash[i] < self.target[i] {
                return true;
            }
            if hash[i] > self.target[i] {
                return false;
            }
        }
        false
    }

    /// Hex of the Stratum share target currently enforced (`OpenChannel.Success`
    /// / `SetTarget`). Same byte order the worker compares hashes against.
    pub fn share_target_hex(&self) -> String {
        hex::encode(self.target)
    }

    /// Pool-style difficulty scalar for logs: `max_target / share_target`
    /// with the testnet-stress / CPU ceiling `max_target = 2^240`
    /// (`[0, 1, 0, …]`). Matches `calculate_difficulty_with_max_target` on
    /// that ceiling — not Bitcoin's `2^256/target` display. Distinct from
    /// `header_bits` (chain work / rBits on the 80-byte header).
    pub fn share_difficulty(&self) -> u64 {
        difficulty_vs_2_240(&self.target)
    }
}

/// testnet-stress `GENESIS_MAX_TARGET` / CPU worker ceiling: `2^240`.
pub(crate) const MAX_TARGET_2_240: [u8; 32] = {
    let mut m = [0u8; 32];
    m[1] = 1;
    m
};

/// `⌊2^240 / target⌋`, saturating to `u64::MAX`, minimum 1 when target ≥ max.
pub fn difficulty_vs_2_240(target: &[u8; 32]) -> u64 {
    if target.iter().all(|&b| b == 0) {
        return u64::MAX;
    }
    if target.as_slice() >= MAX_TARGET_2_240.as_slice() {
        return 1;
    }
    // Binary search q in 1..=u64::MAX such that q * target <= 2^240.
    let mut lo = 1u64;
    let mut hi = u64::MAX;
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if mul_u256_u64_le(target, mid, &MAX_TARGET_2_240) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// True iff `factor * den` (big-endian u256) fits in 256 bits and is ≤ `num`.
fn mul_u256_u64_le(den: &[u8; 32], factor: u64, num: &[u8; 32]) -> bool {
    let mut out = [0u8; 32];
    let mut carry = 0u128;
    for i in (0..32).rev() {
        let v = den[i] as u128 * factor as u128 + carry;
        out[i] = (v & 0xff) as u8;
        carry = v >> 8;
    }
    if carry != 0 {
        return false;
    }
    out.as_slice() <= num.as_slice()
}

#[cfg(test)]
mod tests {
    use super::{difficulty_vs_2_240, Job, MAX_TARGET_2_240};

    fn sample_snph(job_id: u32, prev_byte: u8) -> Vec<u8> {
        let mut data = vec![0u8; 52];
        data[4..8].copy_from_slice(&job_id.to_le_bytes());
        data[8] = prev_byte;
        data[40..44].copy_from_slice(&1_700_000_000u32.to_le_bytes());
        data[44..48].copy_from_slice(&0x1e0fffffu32.to_le_bytes());
        data[48..52].copy_from_slice(&42i32.to_le_bytes());
        data
    }

    /// Build a CB-4-shaped `NewMiningJob` payload by hand (mirrors
    /// `messages.rs::NewMiningJob::encode`'s field order exactly).
    fn sample_nmj(job_id: u32, version: u32, aux_byte: u8) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_le_bytes()); // channel_id
        data.extend_from_slice(&job_id.to_le_bytes());
        data.push(0); // future_job = false
        data.extend_from_slice(&version.to_le_bytes());
        data.push(1); // version_rolling_allowed
        data.push(0); // coinbase_prefix len = 0
        data.push(0); // coinbase_suffix len = 0
        data.extend_from_slice(&[aux_byte; 32]); // aux_hash
        data.push(0); // clean_jobs = false
        data
    }

    #[test]
    fn snph_before_nmj_updates_prev_without_changing_job_id() {
        let prefix = [0u8; 4];
        let mut job = Job::from_new_mining_job(&sample_nmj(1, 1, 0xAA), &prefix).unwrap();
        job.set_prev_hash_from_msg(&sample_snph(2, 0xBB));
        assert_eq!(job.job_id, 1, "SNPH must not advance wire job_id before NMJ");
        assert_eq!(job.prev_hash[0], 0xBB);
        assert!(job.has_prev_hash());
    }

    #[test]
    fn take_if_newly_ready_fires_once_per_job_id() {
        let prefix = [0u8; 4];
        let mut job = Job::from_new_mining_job(&sample_nmj(1, 1, 0xAA), &prefix).unwrap();
        // Not ready yet (no matching SetNewPrevHash) — must not fire.
        assert!(!job.take_if_newly_ready());

        job.set_prev_hash_from_msg(&sample_snph(1, 0x11));
        // First time this job_id becomes ready — must fire exactly once.
        assert!(job.take_if_newly_ready());

        // Heartbeat re-broadcast of the *same* job_id: merging identical
        // data must not re-fire, even though is_ready() stays true. This is
        // the exact case that trapped the E3 worker mining nonce 0 forever
        // — every re-broadcast wrongly signaled "new job, reset search."
        job.merge_new_mining_job(&sample_nmj(1, 1, 0xAA), &prefix).unwrap();
        assert!(job.is_ready());
        assert!(!job.take_if_newly_ready());
        job.set_prev_hash_from_msg(&sample_snph(1, 0x11));
        assert!(!job.take_if_newly_ready());

        // A genuinely new job_id must fire again.
        job.merge_new_mining_job(&sample_nmj(2, 2, 0xCC), &prefix).unwrap();
        assert!(!job.take_if_newly_ready(), "job_id=2 has no matching prev_hash yet");
        job.set_prev_hash_from_msg(&sample_snph(2, 0x22));
        assert!(job.take_if_newly_ready());
        assert!(!job.take_if_newly_ready(), "must not double-fire for job_id=2 either");
    }

    #[test]
    fn full_job_refresh_sequence_matches_pool_header_fields() {
        let prefix = [0u8; 4];
        let mut job = Job::from_new_mining_job(&sample_nmj(1, 1, 0xAA), &prefix).unwrap();
        job.set_prev_hash_from_msg(&sample_snph(1, 0x11));
        job.merge_new_mining_job(&sample_nmj(2, 2, 0xCC), &prefix).unwrap();
        job.set_prev_hash_from_msg(&sample_snph(2, 0x22));
        job.merge_new_mining_job(&sample_nmj(2, 2, 0xDD), &prefix).unwrap();

        assert_eq!(job.job_id, 2);
        assert_eq!(job.version, 2);
        assert_eq!(job.prev_hash[0], 0x22);
        assert_eq!(job.aux_hash[0], 0xDD);

        let hdr = job.build_header(0x1234_5678);
        assert_eq!(&hdr[0..4], &2u32.to_le_bytes());
        assert_eq!(hdr[4], 0x22);
        assert_eq!(&hdr[76..80], &0x1234_5678u32.to_le_bytes());
    }

    #[test]
    fn coinbase_prefix_and_suffix_round_trip() {
        let prefix = [0u8; 4];
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&7u32.to_le_bytes());
        data.push(0);
        data.extend_from_slice(&1u32.to_le_bytes());
        data.push(1);
        data.push(3);
        data.extend_from_slice(&[0x01, 0x02, 0x03]);
        data.push(2);
        data.extend_from_slice(&[0x04, 0x05]);
        data.extend_from_slice(&[0xEE; 32]);
        data.push(1); // clean_jobs = true

        let job = Job::from_new_mining_job(&data, &prefix).unwrap();
        assert_eq!(job.coinbase_prefix, vec![0x01, 0x02, 0x03]);
        assert_eq!(job.coinbase_suffix, vec![0x04, 0x05]);
        assert_eq!(job.aux_hash, [0xEE; 32]);
        assert!(job.clean_jobs);
    }

    #[test]
    fn different_extranonce2_changes_commitment_hash() {
        let prefix = [1, 2, 3, 4];
        let mut job = Job::from_new_mining_job(&sample_nmj(1, 1, 0xAA), &prefix).unwrap();
        job.extranonce2 = 0;
        let h1 = job.commitment_hash();
        job.extranonce2 = 1;
        let h2 = job.commitment_hash();
        assert_ne!(h1, h2);
    }

    #[test]
    fn different_extranonce_prefix_changes_commitment_hash() {
        let mut job_a = Job::from_new_mining_job(&sample_nmj(1, 1, 0xAA), &[1, 0, 0, 0]).unwrap();
        let job_b = Job::from_new_mining_job(&sample_nmj(1, 1, 0xAA), &[2, 0, 0, 0]).unwrap();
        job_a.extranonce_prefix = [1, 0, 0, 0];
        assert_ne!(job_a.commitment_hash(), job_b.commitment_hash());
    }

    #[test]
    fn share_difficulty_vs_2_240_known_values() {
        assert_eq!(difficulty_vs_2_240(&MAX_TARGET_2_240), 1);
        // 2^234 = 2^240 / 64
        let mut t64 = [0u8; 32];
        t64[2] = 0x04;
        assert_eq!(difficulty_vs_2_240(&t64), 64);
        // Soft all-ones target → difficulty 1
        assert_eq!(difficulty_vs_2_240(&[0xff; 32]), 1);
    }
}
