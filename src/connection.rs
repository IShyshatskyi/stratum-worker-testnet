// tools/stratum-worker/src/connection.rs
//
// SP-18 — Noise_NX handshake + Stratum V2 framing (matches pool server codec).
//
// Aug 16, 2026 rewrite (Correctness_and_Validation_Plan.md NC-77): the
// mining loop used to call a single-task `poll_job_updates` between PoW
// batches to check for a fresh job without blocking hashing, guarding the
// read with a short timeout so a quiet socket wouldn't stall the loop. A
// live soak found that check essentially never worked — 0 of 19 job
// updates were ever picked up via that path across a full run; every one
// was only discovered as a side effect of the worker's own next share
// submission. A follow-up fix widened the read's own timeout (1ms →
// 500ms once the socket was confirmed readable), but that still let the
// mining loop block on network I/O in the case where a frame arrived
// slowly — exactly the failure mode this rewrite removes: the worker must
// never stop hashing to wait on the network, at all, regardless of how
// generous the timeout is.
//
// Fixed by decoupling reads from the mining loop entirely, mirroring the
// pool's own server-side fix for the identical class of problem (RF-28,
// `crates/common-types/src/transport.rs`'s `NoiseReader`/`NoiseWriter`
// split): snow's *stateless* transport mode lets the cipher state be a
// plain `Arc` shared read-only between an owned TCP read half and an
// owned write half, each tracking its own explicit nonce. A dedicated
// background task owns the read half outright for the life of the
// session and loops forever — no timeout needed, since it isn't gating
// anything the mining loop needs synchronously — applying every frame to
// its own local `Job` and publishing a fresh snapshot over a
// `tokio::sync::watch` channel whenever the job is ready. The mining loop
// just polls that channel (`JobWatcher::poll`): pure in-memory, zero I/O,
// cannot stall hashing no matter how slow or fast the network is.
// `submit_share` becomes a plain write — the background reader already
// continuously drains the socket (including the pool's ACK/ERR
// responses, silently discarded there), so the old "drain the ACK so the
// TCP window doesn't stall" logic isn't needed either.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Context};
use snow::{Builder, params::NoiseParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

use crate::job::Job;
use crate::sv2_codec::{
    decode_frame, open_channel_frame, parse_open_channel_success, setup_connection_frame,
    submit_shares_frame, MSG_NEW_MINING_JOB, MSG_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
    MSG_SET_NEW_PREV_HASH, MSG_SET_TARGET, MSG_SETUP_CONNECTION_SUCCESS,
};

const NOISE_PARAMS: &str = "Noise_NX_25519_ChaChaPoly_BLAKE2s";
const NOISE_MAX_MSG: usize = 65535;

/// Dedicated write half — owned outright by `StratumConnection`, one writer
/// per connection, no lock needed.
struct NoiseWriter {
    inner: OwnedWriteHalf,
    noise: Arc<snow::StatelessTransportState>,
    /// Nonce for the next outgoing message. Must increment exactly once per
    /// message actually written, in that order — guaranteed here since
    /// `StratumConnection` only ever calls `send_frame` from one task.
    next_nonce: u64,
}

impl NoiseWriter {
    async fn send_frame(&mut self, frame: &[u8]) -> anyhow::Result<()> {
        let nonce = self.next_nonce;
        self.next_nonce += 1;
        let mut buf = vec![0u8; NOISE_MAX_MSG + 16];
        let enc_len = self.noise.write_message(nonce, frame, &mut buf)?;
        self.inner.write_all(&(enc_len as u16).to_le_bytes()).await?;
        self.inner.write_all(&buf[..enc_len]).await?;
        Ok(())
    }
}

/// Dedicated read half — owned outright by the background job-watcher task
/// once the connection is set up; there is never more than one reader, so
/// no lock is needed here either.
struct NoiseReader {
    inner: OwnedReadHalf,
    noise: Arc<snow::StatelessTransportState>,
    /// Nonce for the next incoming message — correct because TCP delivers
    /// bytes in send order, so the Nth frame this side decodes is always
    /// the Nth frame the peer sent (mirrors `NoiseWriter::next_nonce`).
    next_nonce: u64,
}

impl NoiseReader {
    async fn recv_frame(&mut self) -> anyhow::Result<(u16, u8, Vec<u8>)> {
        let mut len_buf = [0u8; 2];
        self.inner.read_exact(&mut len_buf).await?;
        let enc_len = u16::from_le_bytes(len_buf) as usize;
        let mut enc = vec![0u8; enc_len];
        self.inner.read_exact(&mut enc).await?;
        let nonce = self.next_nonce;
        self.next_nonce += 1;
        let mut plaintext = vec![0u8; NOISE_MAX_MSG];
        let n = self.noise.read_message(nonce, &enc, &mut plaintext)?;
        let raw = &plaintext[..n];
        let Some((ext, msg_type, start, end)) = decode_frame(raw)? else {
            bail!("incomplete SV2 frame");
        };
        Ok((ext, msg_type, raw[start..end].to_vec()))
    }
}

/// Applies one received frame to `job`. Returns `true` if `job` is a fully
/// paired, ready job afterward — the background reader task publishes on
/// every `true`, including target-only updates and heartbeat repeats of an
/// unchanged job_id; `JobWatcher`'s consumer distinguishes "actually new"
/// from "repeat" itself, by comparing `job_id`, so gating publication on
/// mere readiness (rather than a "newly ready" special case) is sufficient
/// and cannot leak a not-yet-paired job (mixed old/new fields) to the
/// mining loop — the exact bug NC-32/CB-7 fixed once already.
fn apply_frame_to_job(
    job: &mut Job,
    msg_type: u8,
    data: &[u8],
    extranonce_prefix: &[u8; 4],
) -> anyhow::Result<bool> {
    match msg_type {
        MSG_NEW_MINING_JOB => {
            job.merge_new_mining_job(data, extranonce_prefix)?;
        }
        MSG_SET_NEW_PREV_HASH => {
            job.set_prev_hash_from_msg(data);
        }
        MSG_SET_TARGET => {
            if data.len() >= 36 {
                job.target.copy_from_slice(&data[4..36]);
            }
        }
        // SubmitSharesSuccess/Error ACKs and anything else land here and
        // are silently discarded — the background reader draining them is
        // what keeps the TCP window open; nothing else needs them.
        _ => {}
    }
    Ok(job.is_ready())
}

/// Non-blocking view onto the background reader task's latest known job.
/// Never touches the network — reads a value the reader task already
/// published, so polling it can never stall the mining loop.
pub struct JobWatcher {
    rx: tokio::sync::watch::Receiver<Job>,
}

impl JobWatcher {
    /// Blocks until the job changes — used only to obtain the very first
    /// ready job at session start (mirrors the old blocking `wait_for_job`).
    pub async fn wait_for_change(&mut self) -> anyhow::Result<Job> {
        self.rx
            .changed()
            .await
            .context("job watcher channel closed (connection lost)")?;
        Ok(self.rx.borrow_and_update().clone())
    }

    /// Non-blocking: `Some(job)` only if a new snapshot has been published
    /// since the last call. Pure in-memory read — never awaits, never
    /// touches the socket, safe to call every batch iteration. `Err` means
    /// the background reader task ended (connection lost) — propagate so
    /// the caller reconnects, matching the old behaviour where a read
    /// error surfaced through `poll_job_updates`/`submit_share` directly.
    pub fn poll(&mut self) -> anyhow::Result<Option<Job>> {
        match self.rx.has_changed() {
            Ok(true) => Ok(Some(self.rx.borrow_and_update().clone())),
            Ok(false) => Ok(None),
            Err(_) => bail!("job watcher channel closed (connection lost)"),
        }
    }
}

pub struct StratumConnection {
    writer: NoiseWriter,
    pub channel_id: u32,
    pub extranonce_prefix: [u8; 4],
    share_sequence: u32,
}

impl StratumConnection {
    /// Connects, completes the Noise_NX handshake and Stratum V2
    /// SetupConnection/OpenChannel exchange, then hands the read half off
    /// to a background task and returns `(connection, job_watcher)`.
    pub async fn connect(
        addr:          SocketAddr,
        _server_pubkey: &[u8; 32],
        worker_name:   &str,
    ) -> anyhow::Result<(Self, JobWatcher)> {
        let mut stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("TCP connect to {addr}"))?;

        // Noise_NX: server static key arrives in the responder's second message.
        let params: NoiseParams = NOISE_PARAMS.parse()?;
        let mut handshake = Builder::new(params).build_initiator()?;

        let mut buf = vec![0u8; NOISE_MAX_MSG];
        let n = handshake.write_message(&[], &mut buf)?;
        write_noise_frame(&mut stream, &buf[..n]).await?;

        let frame = read_noise_frame(&mut stream).await?;
        let mut plaintext = vec![0u8; NOISE_MAX_MSG];
        handshake.read_message(&frame, &mut plaintext)?;

        // Stateless mode — see module doc comment for why: it's what makes
        // the reader/writer split below possible (each half tracks its own
        // explicit nonce instead of sharing one `&mut self`-only state).
        let transport = Arc::new(handshake.into_stateless_transport_mode()?);
        let (read_half, write_half) = stream.into_split();
        let mut reader = NoiseReader { inner: read_half, noise: Arc::clone(&transport), next_nonce: 0 };
        let mut writer = NoiseWriter { inner: write_half, noise: transport, next_nonce: 0 };

        let setup = setup_connection_frame(
            &addr.ip().to_string(),
            addr.port(),
            "stratum-worker",
            worker_name,
        )?;
        writer.send_frame(&setup).await?;

        let (_ext, msg_type, payload) = reader.recv_frame().await?;
        if msg_type != MSG_SETUP_CONNECTION_SUCCESS {
            bail!("expected SetupConnection.Success (0x01), got 0x{msg_type:02x}");
        }
        if payload.len() < 6 {
            bail!("SetupConnection.Success payload too short");
        }

        let open = open_channel_frame(1, worker_name, 1e9f32)?;
        writer.send_frame(&open).await?;

        let (channel_id, pool_target, extranonce_prefix) = loop {
            let (_ext, msg_type, payload) = reader.recv_frame().await?;
            if msg_type == MSG_OPEN_STANDARD_MINING_CHANNEL_SUCCESS {
                break parse_open_channel_success(&payload)?;
            }
        };

        let conn = Self {
            writer,
            channel_id,
            extranonce_prefix,
            share_sequence: 0,
        };

        // Background reader: owns the socket's read side for the rest of
        // the session. No timeout on recv_frame() here — it isn't gating
        // anything the mining loop needs synchronously, so there's nothing
        // to time out *for*; the loop just keeps draining and republishing
        // whenever the locally-tracked job is ready.
        let (job_tx, job_rx) = tokio::sync::watch::channel(Job::default());
        tokio::spawn(async move {
            let mut job = Job::default();
            job.extranonce_prefix = extranonce_prefix;
            // Seed the real target from OpenChannel.Success, not the
            // maximally-permissive [0xFF; 32] placeholder `Job::default()`
            // implies. Missing this (an earlier version of this rewrite
            // dropped it, naming the parsed value `_target`) means every
            // nonce looks like a "found share" locally until the first
            // SET_TARGET arrives — confirmed live: ~109 hashes per
            // "share found", a flood of low_difficulty rejections
            // (656k in a 12-minute soak vs. ~1k in a prior 5h soak), and
            // real search time wasted submitting near-worthless attempts
            // instead of continuing toward the pool's actual difficulty.
            job.target = pool_target;
            loop {
                match reader.recv_frame().await {
                    Ok((_ext, msg_type, data)) => {
                        match apply_frame_to_job(&mut job, msg_type, &data, &extranonce_prefix) {
                            Ok(true) => {
                                if job_tx.send(job.clone()).is_err() {
                                    break; // no receivers left — session ending
                                }
                            }
                            Ok(false) => {}
                            Err(e) => {
                                log::warn!("job watcher: malformed frame ignored: {e:#}");
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("job watcher: connection read ended: {e:#}");
                        break;
                    }
                }
            }
        });

        Ok((conn, JobWatcher { rx: job_rx }))
    }

    /// Submit a share. Does not wait for or drain the pool's ACK — the
    /// background reader task already continuously drains the socket
    /// (including `SubmitSharesSuccess`/`Error` frames, silently discarded
    /// there — see `apply_frame_to_job`), so there is nothing left for this
    /// call to do but write.
    pub async fn submit_share(
        &mut self,
        job_id:  u32,
        nonce:   u32,
        ntime:   u32,
        version: u32,
    ) -> anyhow::Result<()> {
        self.share_sequence = self.share_sequence.wrapping_add(1);
        // CB-5/CB-7: extranonce2 always 0 until local rolling is implemented.
        let frame = submit_shares_frame(
            self.channel_id,
            self.share_sequence,
            job_id,
            nonce,
            ntime,
            version,
            0,
        )?;
        self.writer.send_frame(&frame).await
    }
}

async fn write_noise_frame(stream: &mut TcpStream, data: &[u8]) -> anyhow::Result<()> {
    stream.write_all(&(data.len() as u16).to_le_bytes()).await?;
    stream.write_all(data).await?;
    Ok(())
}

async fn read_noise_frame(stream: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    stream.read_exact(&mut len_buf).await?;
    let len = u16::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}
