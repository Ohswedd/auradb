//! Property and fuzz tests for the frame codec: decoding arbitrary bytes must
//! never panic, and any valid frame must round-trip through encode/decode.

use auradb_protocol::{Frame, Opcode, RequestId, DEFAULT_MAX_PAYLOAD};
use proptest::prelude::*;

proptest! {
    /// Decoding arbitrary input never panics; it returns Ok(Some), Ok(None)
    /// (needs more bytes), or a structured error.
    #[test]
    fn decode_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD);
    }

    /// Any frame round-trips: encode then decode yields the same frame and
    /// consumes exactly the encoded length.
    #[test]
    fn roundtrip_arbitrary_frames(
        opcode_byte in 0u8..16,
        request_id in any::<u128>(),
        txn_id in any::<u64>(),
        payload in proptest::collection::vec(any::<u8>(), 0..1024),
    ) {
        // Map the random byte onto a real request opcode space.
        let opcode = match opcode_byte % 6 {
            0 => Opcode::Hello,
            1 => Opcode::Ping,
            2 => Opcode::Query,
            3 => Opcode::Mutate,
            4 => Opcode::Explain,
            _ => Opcode::CursorFetch,
        };
        let frame = Frame::new(opcode, RequestId(request_id), txn_id, payload);
        let bytes = frame.encode();
        let (decoded, n) = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD).unwrap().unwrap();
        prop_assert_eq!(n, bytes.len());
        prop_assert_eq!(decoded, frame);
    }

    /// A valid frame with one byte flipped anywhere is either rejected
    /// (checksum/limit/opcode) or, if still structurally valid, never panics.
    #[test]
    fn single_bit_corruption_is_handled(
        flip_index in 0usize..40,
        flip_mask in 1u8..=255,
    ) {
        let frame = Frame::new(Opcode::Query, RequestId(1), 0, b"payload".to_vec());
        let mut bytes = frame.encode();
        bytes[flip_index] ^= flip_mask;
        // Must not panic; the header checksum should catch most corruption.
        let _ = Frame::decode(&bytes, DEFAULT_MAX_PAYLOAD);
    }
}
