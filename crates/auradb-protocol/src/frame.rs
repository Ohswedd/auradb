//! AWP frame layout, encoding, and decoding.
//!
//! The frame header is fixed at [`HEADER_LEN`] bytes and is laid out as:
//!
//! ```text
//! offset  field
//! 0..4    magic "AURA"
//! 4       version
//! 5       opcode (frame type)
//! 6..8    flags (u16, big-endian)
//! 8..10   header length (u16, big-endian) - always HEADER_LEN
//! 10      compression (0 = none)
//! 11      reserved (0)
//! 12..16  payload length (u32, big-endian)
//! 16..32  request id (u128, big-endian)
//! 32..40  transaction id (u64, big-endian)
//! 40..44  header checksum (CRC32 of bytes 0..40, big-endian)
//! 44..    payload
//! [payload checksum] CRC32 of payload, present iff FLAG_PAYLOAD_CHECKSUM
//! ```

use auradb_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::opcode::Opcode;

/// The 4-byte frame magic.
pub const MAGIC: [u8; 4] = *b"AURA";
/// The protocol version implemented by this build.
pub const PROTOCOL_VERSION: u8 = 1;
/// The fixed header length in bytes.
pub const HEADER_LEN: usize = 44;
/// Offset at which the header checksum begins (it covers bytes `0..HEADER_CHECKSUM_OFFSET`).
const HEADER_CHECKSUM_OFFSET: usize = 40;
/// Default maximum accepted payload size (16 MiB).
pub const DEFAULT_MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// Flag: a 4-byte CRC32 payload checksum follows the payload body.
pub const FLAG_PAYLOAD_CHECKSUM: u16 = 0x0001;

/// Compression mode (only `None` is implemented).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// No compression.
    None,
}

impl Compression {
    fn as_u8(self) -> u8 {
        match self {
            Compression::None => 0,
        }
    }

    fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(Compression::None),
            other => Err(Error::unsupported(format!("compression mode {other}"))),
        }
    }
}

/// A 128-bit request identifier used to correlate responses with requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u128);

impl RequestId {
    /// The zero request id (used for unsolicited frames).
    pub const ZERO: RequestId = RequestId(0);
}

/// A decoded or to-be-encoded protocol frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Protocol version.
    pub version: u8,
    /// The operation this frame represents.
    pub opcode: Opcode,
    /// Frame flags bitfield.
    pub flags: u16,
    /// Compression applied to the payload.
    pub compression: Compression,
    /// Request id for correlation.
    pub request_id: RequestId,
    /// Associated transaction id (0 = none / auto-commit).
    pub txn_id: u64,
    /// Opaque payload bytes (typically JSON).
    pub payload: Vec<u8>,
}

impl Frame {
    /// Build a frame with the current protocol version and a payload checksum.
    pub fn new(opcode: Opcode, request_id: RequestId, txn_id: u64, payload: Vec<u8>) -> Self {
        Frame {
            version: PROTOCOL_VERSION,
            opcode,
            flags: FLAG_PAYLOAD_CHECKSUM,
            compression: Compression::None,
            request_id,
            txn_id,
            payload,
        }
    }

    /// Build a frame whose payload is the JSON encoding of `value`.
    pub fn json<T: Serialize>(
        opcode: Opcode,
        request_id: RequestId,
        txn_id: u64,
        value: &T,
    ) -> Result<Self> {
        let payload = serde_json::to_vec(value)
            .map_err(|e| Error::Protocol(format!("payload serialization failed: {e}")))?;
        Ok(Frame::new(opcode, request_id, txn_id, payload))
    }

    /// Decode the JSON payload into a typed value.
    pub fn decode_json<T: for<'de> Deserialize<'de>>(&self) -> Result<T> {
        serde_json::from_slice(&self.payload)
            .map_err(|e| Error::Protocol(format!("payload deserialization failed: {e}")))
    }

    /// Whether the frame requests a payload checksum.
    pub fn has_payload_checksum(&self) -> bool {
        self.flags & FLAG_PAYLOAD_CHECKSUM != 0
    }

    /// The total encoded size of this frame in bytes.
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len() + if self.has_payload_checksum() { 4 } else { 0 }
    }

    /// Encode the frame to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.extend_from_slice(&MAGIC);
        buf.push(self.version);
        buf.push(self.opcode.as_u8());
        buf.extend_from_slice(&self.flags.to_be_bytes());
        buf.extend_from_slice(&(HEADER_LEN as u16).to_be_bytes());
        buf.push(self.compression.as_u8());
        buf.push(0); // reserved
        buf.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.request_id.0.to_be_bytes());
        buf.extend_from_slice(&self.txn_id.to_be_bytes());
        let header_checksum = crc32fast::hash(&buf[..HEADER_CHECKSUM_OFFSET]);
        buf.extend_from_slice(&header_checksum.to_be_bytes());
        debug_assert_eq!(buf.len(), HEADER_LEN);
        buf.extend_from_slice(&self.payload);
        if self.has_payload_checksum() {
            let payload_checksum = crc32fast::hash(&self.payload);
            buf.extend_from_slice(&payload_checksum.to_be_bytes());
        }
        buf
    }

    /// Attempt to decode a single frame from the front of `buf`.
    ///
    /// Returns `Ok(None)` if more bytes are needed, `Ok(Some((frame, n)))` with
    /// the number of bytes consumed on success, or an error for a malformed
    /// frame or a payload exceeding `max_payload`.
    pub fn decode(buf: &[u8], max_payload: usize) -> Result<Option<(Frame, usize)>> {
        if buf.len() < HEADER_LEN {
            return Ok(None);
        }
        if buf[0..4] != MAGIC {
            return Err(Error::Protocol("bad magic bytes".into()));
        }
        let version = buf[4];
        if version == 0 || version > PROTOCOL_VERSION {
            return Err(Error::Protocol(format!(
                "unsupported protocol version {version}"
            )));
        }
        let opcode = Opcode::from_u8(buf[5])?;
        let flags = u16::from_be_bytes([buf[6], buf[7]]);
        let header_length = u16::from_be_bytes([buf[8], buf[9]]) as usize;
        if header_length != HEADER_LEN {
            return Err(Error::Protocol(format!(
                "invalid header length {header_length}"
            )));
        }
        let compression = Compression::from_u8(buf[10])?;
        let payload_len = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]) as usize;
        if payload_len > max_payload {
            return Err(Error::LimitExceeded(format!(
                "payload length {payload_len} exceeds max {max_payload}"
            )));
        }
        let stored_header_checksum = u32::from_be_bytes([buf[40], buf[41], buf[42], buf[43]]);
        let computed = crc32fast::hash(&buf[..HEADER_CHECKSUM_OFFSET]);
        if stored_header_checksum != computed {
            return Err(Error::Corruption("header checksum mismatch".into()));
        }

        let has_payload_checksum = flags & FLAG_PAYLOAD_CHECKSUM != 0;
        let trailer = if has_payload_checksum { 4 } else { 0 };
        let total = HEADER_LEN + payload_len + trailer;
        if buf.len() < total {
            return Ok(None);
        }

        let request_id = RequestId(u128::from_be_bytes(
            buf[16..32].try_into().expect("16 bytes"),
        ));
        let txn_id = u64::from_be_bytes(buf[32..40].try_into().expect("8 bytes"));
        let payload = buf[HEADER_LEN..HEADER_LEN + payload_len].to_vec();

        if has_payload_checksum {
            let off = HEADER_LEN + payload_len;
            let stored = u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
            let computed = crc32fast::hash(&payload);
            if stored != computed {
                return Err(Error::Corruption("payload checksum mismatch".into()));
            }
        }

        Ok(Some((
            Frame {
                version,
                opcode,
                flags,
                compression,
                request_id,
                txn_id,
                payload,
            },
            total,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Frame {
        Frame::new(
            Opcode::Query,
            RequestId(0x1122_3344_5566_7788_99aa_bbcc_ddee_ff00),
            42,
            br#"{"op":"find"}"#.to_vec(),
        )
    }

    #[test]
    fn roundtrip() {
        let f = sample();
        let bytes = f.encode();
        assert_eq!(bytes.len(), f.encoded_len());
        let (decoded, n) = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap().unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(decoded, f);
    }

    #[test]
    fn roundtrip_without_payload_checksum() {
        let mut f = sample();
        f.flags = 0;
        let bytes = f.encode();
        let (decoded, _) = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap().unwrap();
        assert_eq!(decoded, f);
    }

    #[test]
    fn unknown_magic_rejected() {
        let mut bytes = sample().encode();
        bytes[0] = b'X';
        // header checksum will also fail, but magic is checked first.
        assert!(Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).is_err());
    }

    #[test]
    fn bad_version_rejected() {
        let mut f = sample();
        f.version = 99;
        let bytes = f.encode();
        let err = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap_err();
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn corrupt_header_checksum_rejected() {
        let mut bytes = sample().encode();
        bytes[42] ^= 0xff;
        let err = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap_err();
        assert!(matches!(err, Error::Corruption(_)));
    }

    #[test]
    fn corrupt_payload_checksum_rejected() {
        let mut bytes = sample().encode();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        let err = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap_err();
        assert!(matches!(err, Error::Corruption(_)));
    }

    #[test]
    fn oversized_payload_rejected() {
        let bytes = sample().encode();
        let err = Frame::decode(&bytes, 4).unwrap_err();
        assert!(matches!(err, Error::LimitExceeded(_)));
    }

    #[test]
    fn truncated_frame_needs_more() {
        let bytes = sample().encode();
        assert!(Frame::decode(&bytes[..HEADER_LEN - 1], DEFAULT_MAX_PAYLOAD)
            .unwrap()
            .is_none());
        assert!(Frame::decode(&bytes[..HEADER_LEN + 2], DEFAULT_MAX_PAYLOAD)
            .unwrap()
            .is_none());
    }

    #[test]
    fn json_payload_roundtrip() {
        #[derive(Serialize, serde::Deserialize, PartialEq, Debug)]
        struct P {
            a: u32,
            b: String,
        }
        let p = P {
            a: 7,
            b: "x".into(),
        };
        let f = Frame::json(Opcode::Mutate, RequestId::ZERO, 0, &p).unwrap();
        let bytes = f.encode();
        let (decoded, _) = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap().unwrap();
        assert_eq!(decoded.decode_json::<P>().unwrap(), p);
    }
}
