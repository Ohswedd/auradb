//! Async framing on top of the synchronous [`Frame`] codec.

use auradb_core::{Error, Result};
use auradb_protocol::{Frame, FLAG_PAYLOAD_CHECKSUM, HEADER_LEN};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Read one frame from `reader`.
///
/// Returns `Ok(None)` on a clean end-of-stream (no bytes, or connection closed
/// before a full header). A connection that closes mid-frame after a header is
/// reported as a protocol error.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_payload: usize,
) -> Result<Option<Frame>> {
    let mut header = [0u8; HEADER_LEN];
    match reader.read_exact(&mut header).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    }

    let payload_len = u32::from_be_bytes([header[12], header[13], header[14], header[15]]) as usize;
    if payload_len > max_payload {
        return Err(Error::LimitExceeded(format!(
            "payload length {payload_len} exceeds max {max_payload}"
        )));
    }
    let flags = u16::from_be_bytes([header[6], header[7]]);
    let trailer = if flags & FLAG_PAYLOAD_CHECKSUM != 0 {
        4
    } else {
        0
    };

    let mut full = Vec::with_capacity(HEADER_LEN + payload_len + trailer);
    full.extend_from_slice(&header);
    full.resize(HEADER_LEN + payload_len + trailer, 0);
    reader
        .read_exact(&mut full[HEADER_LEN..])
        .await
        .map_err(Error::Io)?;

    match Frame::decode(&full, max_payload)? {
        Some((frame, _)) => Ok(Some(frame)),
        None => Err(Error::Protocol("incomplete frame".into())),
    }
}

/// Write one frame to `writer`, returning the number of bytes written.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> Result<usize> {
    let bytes = frame.encode();
    writer.write_all(&bytes).await.map_err(Error::Io)?;
    writer.flush().await.map_err(Error::Io)?;
    Ok(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_protocol::{Opcode, RequestId, DEFAULT_MAX_PAYLOAD};

    #[tokio::test]
    async fn roundtrip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let frame = Frame::new(Opcode::Ping, RequestId(7), 0, b"hi".to_vec());
        write_frame(&mut a, &frame).await.unwrap();
        let got = read_frame(&mut b, DEFAULT_MAX_PAYLOAD)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, frame);
    }

    #[tokio::test]
    async fn clean_eof_returns_none() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert!(read_frame(&mut b, DEFAULT_MAX_PAYLOAD)
            .await
            .unwrap()
            .is_none());
    }
}
