// tools/stratum-worker/src/sv2_codec.rs
//
// Minimal Stratum V2 frame codec — mirrors src/mining_pool/stratum/v2/codec.rs.

use std::io;

pub const FRAME_HEADER_LEN: usize = 6;
pub const EXT_STANDARD: u16 = 0x0000;

pub const MSG_SETUP_CONNECTION: u8 = 0x00;
pub const MSG_SETUP_CONNECTION_SUCCESS: u8 = 0x01;
pub const MSG_OPEN_STANDARD_MINING_CHANNEL: u8 = 0x10;
pub const MSG_OPEN_STANDARD_MINING_CHANNEL_SUCCESS: u8 = 0x11;
pub const MSG_NEW_MINING_JOB: u8 = 0x1a;
pub const MSG_SET_NEW_PREV_HASH: u8 = 0x20;
pub const MSG_SUBMIT_SHARES_STANDARD: u8 = 0x1b;
pub const MSG_SUBMIT_SHARES_SUCCESS: u8 = 0x1c;
pub const MSG_SUBMIT_SHARES_ERROR: u8 = 0x1d;
pub const MSG_SET_TARGET: u8 = 0x21;

pub fn encode_frame(
    extension_type: u16,
    msg_type: u8,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> io::Result<()> {
    let len = payload.len() as u32;
    out.extend_from_slice(&extension_type.to_le_bytes());
    out.push(msg_type);
    out.push((len & 0xFF) as u8);
    out.push(((len >> 8) & 0xFF) as u8);
    out.push(((len >> 16) & 0xFF) as u8);
    out.extend_from_slice(payload);
    Ok(())
}

pub fn decode_frame(buf: &[u8]) -> io::Result<Option<(u16, u8, usize, usize)>> {
    if buf.len() < FRAME_HEADER_LEN {
        return Ok(None);
    }
    let extension_type = u16::from_le_bytes([buf[0], buf[1]]);
    let msg_type = buf[2];
    let payload_len = (buf[3] as u32) | ((buf[4] as u32) << 8) | ((buf[5] as u32) << 16);
    let end = FRAME_HEADER_LEN + payload_len as usize;
    if buf.len() < end {
        return Ok(None);
    }
    Ok(Some((extension_type, msg_type, FRAME_HEADER_LEN, end)))
}

pub struct StrV2Buf<const CAP: usize> {
    buf: [u8; CAP],
    pos: usize,
}

impl<const CAP: usize> StrV2Buf<CAP> {
    pub fn new() -> Self {
        Self { buf: [0u8; CAP], pos: 0 }
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > CAP - self.pos {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "StrV2Buf capacity exceeded"));
        }
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }

    pub fn put_u8(&mut self, v: u8) -> io::Result<()> {
        self.write_bytes(&[v])
    }

    pub fn put_u16(&mut self, v: u16) -> io::Result<()> {
        self.write_bytes(&v.to_le_bytes())
    }

    pub fn put_u32(&mut self, v: u32) -> io::Result<()> {
        self.write_bytes(&v.to_le_bytes())
    }

    pub fn put_bool(&mut self, v: bool) -> io::Result<()> {
        self.put_u8(if v { 1 } else { 0 })
    }

    pub fn put_u256(&mut self, v: &[u8; 32]) -> io::Result<()> {
        self.write_bytes(v)
    }

    pub fn put_str0_255(&mut self, s: &str) -> io::Result<()> {
        let b = s.as_bytes();
        if b.len() > 255 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "STR0_255 too long"));
        }
        self.put_u8(b.len() as u8)?;
        self.write_bytes(b)
    }

    pub fn write_f32(&mut self, v: f32) -> io::Result<()> {
        self.put_u32(v.to_bits())
    }

    pub fn finish(&self) -> &[u8] {
        &self.buf[..self.pos]
    }
}

pub struct StrV2Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> StrV2Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_bytes(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.buf.len().saturating_sub(self.pos) < n {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "StrV2Reader: buffer too short"));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    pub fn get_u8(&mut self) -> io::Result<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn get_u16(&mut self) -> io::Result<u16> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn get_u32(&mut self) -> io::Result<u32> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn get_bool(&mut self) -> io::Result<bool> {
        Ok(self.get_u8()? != 0)
    }

    pub fn get_u256(&mut self) -> io::Result<[u8; 32]> {
        Ok(self.read_bytes(32)?.try_into().unwrap())
    }

    pub fn get_b0_32(&mut self) -> io::Result<Vec<u8>> {
        let len = self.get_u8()? as usize;
        Ok(self.read_bytes(len)?.to_vec())
    }
}

pub fn setup_connection_frame(
    endpoint_host: &str,
    endpoint_port: u16,
    vendor: &str,
    device_id: &str,
) -> io::Result<Vec<u8>> {
    let mut buf: StrV2Buf<512> = StrV2Buf::new();
    buf.put_u8(2)?; // mining protocol
    buf.put_u16(2)?;
    buf.put_u16(2)?;
    buf.put_u32(0)?;
    buf.put_str0_255(endpoint_host)?;
    buf.put_u16(endpoint_port)?;
    buf.put_str0_255(vendor)?;
    buf.put_str0_255("sp18")?;
    buf.put_str0_255("1.0")?;
    buf.put_str0_255(device_id)?;
    let payload = buf.finish().to_vec();
    let mut out = Vec::new();
    encode_frame(EXT_STANDARD, MSG_SETUP_CONNECTION, &payload, &mut out)?;
    Ok(out)
}

pub fn open_channel_frame(
    request_id: u32,
    user_identity: &str,
    nominal_hash_rate: f32,
) -> io::Result<Vec<u8>> {
    let mut buf: StrV2Buf<304> = StrV2Buf::new();
    buf.put_u32(request_id)?;
    buf.put_str0_255(user_identity)?;
    buf.write_f32(nominal_hash_rate)?;
    buf.put_u256(&[0xFF; 32])?;
    let payload = buf.finish().to_vec();
    let mut out = Vec::new();
    encode_frame(EXT_STANDARD, MSG_OPEN_STANDARD_MINING_CHANNEL, &payload, &mut out)?;
    Ok(out)
}

pub fn parse_open_channel_success(payload: &[u8]) -> io::Result<(u32, [u8; 32], [u8; 4])> {
    let mut r = StrV2Reader::new(payload);
    let _request_id = r.get_u32()?;
    let channel_id = r.get_u32()?;
    let target = r.get_u256()?;
    let extranonce = r.get_b0_32()?;
    let mut prefix = [0u8; 4];
    let copy_len = extranonce.len().min(4);
    prefix[..copy_len].copy_from_slice(&extranonce[..copy_len]);
    Ok((channel_id, target, prefix))
}

/// CB-5 (`Stratum_Extranonce_Binding_Plan.md` §4.3): `extranonce2` is the
/// miner-rolled half of the coinbase extranonce — always `0` until CB-7's
/// local rolling is implemented, but wired through now so this frame stays
/// byte-compatible with the pool's widened `SubmitSharesStandard` (24 → 28
/// bytes). This function is this crate's own independent copy of that wire
/// format (the worker deliberately has no dependency on the pool's message
/// types), so it must be kept in lockstep with
/// `src/mining_pool/stratum/v2/messages.rs::SubmitSharesStandard` by hand.
pub fn submit_shares_frame(
    channel_id: u32,
    sequence_number: u32,
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
    extranonce2: u32,
) -> io::Result<Vec<u8>> {
    let mut buf: StrV2Buf<28> = StrV2Buf::new();
    buf.put_u32(channel_id)?;
    buf.put_u32(sequence_number)?;
    buf.put_u32(job_id)?;
    buf.put_u32(nonce)?;
    buf.put_u32(ntime)?;
    buf.put_u32(version)?;
    buf.put_u32(extranonce2)?;
    let payload = buf.finish().to_vec();
    let mut out = Vec::new();
    encode_frame(EXT_STANDARD, MSG_SUBMIT_SHARES_STANDARD, &payload, &mut out)?;
    Ok(out)
}
