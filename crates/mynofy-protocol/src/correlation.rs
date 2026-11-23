//! Bounded request-ID projection for rejection before command admission.
//!
//! The generated projection uses Bytes for field 1 rather than String: Protobuf
//! represents both identically. With Bytes input, IDs are shared slices, never
//! attacker-sized String allocations. Unknown fields (including payloads) are
//! skipped by prost. This does not validate or authorize the command itself.
use crate::{ApiError, ProtobufMessage, Result, pb};
use bytes::Bytes;

/// Extract a client command ID without decoding its payload or executing it.
///
/// The input must be the already bounded WebSocket Bytes buffer. At most 128
/// bytes are copied into the returned String. Prost preserves field/oneof merge
/// semantics and enforces its normal wire framing and recursion checks.
pub fn request_id(raw: Bytes, max_frame_bytes: usize) -> Result<String> {
    if raw.len() > max_frame_bytes {
        return Err(ApiError::invalid());
    }
    let metadata = pb::FrameMetadata::decode(raw).map_err(|_| ApiError::invalid())?;
    if metadata.version != 1 {
        return Err(ApiError::invalid());
    }
    let id = metadata.into_request_id().ok_or_else(ApiError::invalid)?;
    if id.is_empty() || id.len() > 128 {
        return Err(ApiError::invalid());
    }
    Ok(std::str::from_utf8(&id)
        .map_err(|_| ApiError::invalid())?
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::Payload;
    fn publish(id: &str) -> Bytes {
        pb::Frame {
            version: 1,
            body: Some(pb::Body::Publish(pb::Publish {
                id: id.into(),
                topic: "t".into(),
                payload: Payload::bytes(vec![7; 4096], None),
                expires: None,
            })),
        }
        .encode_to_vec()
        .into()
    }
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        while value >= 128 {
            out.push((value as u8 & 127) | 128);
            value >>= 7;
        }
        out.push(value as u8);
        out
    }
    fn field(tag: u32, data: &[u8]) -> Vec<u8> {
        let mut out = varint(u64::from(tag) * 8 + 2);
        out.extend(varint(data.len() as u64));
        out.extend(data);
        out
    }
    #[test]
    fn bounds_and_utf8_are_checked_before_copying_the_id() {
        for id in ["a".to_owned(), "x".repeat(128), "ü".repeat(64)] {
            let raw = publish(&id);
            let size = raw.len();
            assert_eq!(request_id(raw.clone(), size).unwrap(), id);
            assert!(request_id(raw, size - 1).is_err());
        }
        for id in [
            "".to_owned(),
            "x".repeat(129),
            "ü".repeat(65),
            "x".repeat(100_000),
        ] {
            assert!(request_id(publish(&id), 200_000).is_err());
        }
        let mut raw = vec![8, 1];
        raw.extend(field(12, &field(1, &[255])));
        assert!(request_id(raw.into(), 1024).is_err());
    }
    #[test]
    fn projection_shares_id_bytes_and_skips_opaque_payload_contents() {
        let mut body = field(1, b"p42");
        body.extend(field(3, &vec![255; 100_000]));
        let mut wire = vec![8, 1];
        wire.extend(field(12, &body));
        let raw: Bytes = wire.into();
        // A full decoder must reject this invalid Payload; rejection metadata must not decode it.
        assert!(pb::Frame::decode(raw.clone()).is_err());
        let id = pb::FrameMetadata::decode(raw.clone())
            .unwrap()
            .into_request_id()
            .unwrap();
        assert!(
            id.as_ptr() >= raw.as_ptr()
                && (id.as_ptr() as usize + id.len()) <= raw.as_ptr() as usize + raw.len()
        );
        assert_eq!(request_id(raw, 200_000).unwrap(), "p42");
    }
    #[test]
    fn field_order_unknown_fields_and_oneof_merging_match_prost() {
        let mut raw = field(12, &field(1, b"first"));
        raw.extend(field(100, b"unknown"));
        raw.extend(field(12, &field(2, b"topic"))); // Same variant merges, retaining ID.
        raw.extend([8, 1]);
        assert_eq!(request_id(raw.clone().into(), 1024).unwrap(), "first");
        raw.extend(field(10, &field(1, b"last"))); // A different command replaces the body.
        assert_eq!(request_id(raw.clone().into(), 1024).unwrap(), "last");
        assert_eq!(
            pb::Frame::decode(raw.clone().as_slice())
                .unwrap()
                .request_id(),
            Some("last")
        );
        raw.extend(field(13, &field(1, b"response"))); // Responses are not commands.
        assert!(request_id(raw.clone().into(), 1024).is_err());
        raw.extend(field(10, &field(2, b"t"))); // Different variant cannot inherit the old ID.
        assert!(request_id(raw.into(), 1024).is_err());
        let mut raw = vec![8, 1];
        let mut body = field(1, b"old");
        body.extend(field(1, b"new"));
        raw.extend(field(10, &body));
        assert_eq!(request_id(raw.into(), 1024).unwrap(), "new");
    }
    #[test]
    fn malformed_envelopes_and_unknown_directions_are_rejected() {
        for raw in [
            vec![],
            vec![8, 2],
            vec![8, 1],
            vec![8, 1, 98, 255],
            vec![8, 1, 98, 4, 10, 3, b'x'],
            vec![8, 1, 96, 1],
            vec![8, 1, 0],
            vec![255; 10],
        ] {
            assert!(request_id(raw.into(), 1024).is_err());
        }
        let mut raw = vec![8, 2];
        raw.extend(field(10, &field(1, b"valid-id")));
        assert!(request_id(raw.into(), 1024).is_err());
        let mut raw = vec![8, 1];
        raw.extend(field(100, &field(1, b"unknown-body")));
        assert!(request_id(raw.into(), 1024).is_err());
    }
    #[test]
    fn all_shared_request_vectors_keep_their_correlation() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/conformance/sdk-v1.json"))
                .unwrap();
        for vector in vectors["frames"].as_array().unwrap() {
            let json = &vector["json"];
            let frame = pb::Frame::from_json(json.clone()).unwrap();
            let result = request_id(frame.encode_to_vec().into(), 1024 * 1024);
            if let Some(id) = json.get("id").and_then(serde_json::Value::as_str) {
                assert_eq!(result.unwrap(), id);
            } else {
                assert!(result.is_err());
            }
        }
    }
    #[test]
    fn unknown_groups_are_skipped_with_the_decoder_recursion_bound() {
        let mut raw = vec![8, 1];
        raw.extend(varint(100 * 8 + 3)); // unknown group
        raw.extend([8, 42]);
        raw.extend(varint(100 * 8 + 4));
        raw.extend(field(10, &field(1, b"group")));
        assert_eq!(request_id(raw.into(), 1024).unwrap(), "group");
        let mut nested = vec![8, 1];
        nested.extend(field(10, &field(1, b"too-deep")));
        for _ in 0..110 {
            nested.extend(varint(100 * 8 + 3));
        }
        for _ in 0..110 {
            nested.extend(varint(100 * 8 + 4));
        }
        assert!(request_id(nested.into(), 4096).is_err());
    }
    #[test]
    fn truncations_and_seeded_mutations_never_panic_or_return_oversized_ids() {
        let raw = publish("probe");
        for size in 0..raw.len() {
            let _ = request_id(raw.slice(..size), raw.len());
        }
        let mut seed = 0x1511_2026_u64;
        for _ in 0..4096 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mut changed = raw.to_vec();
            let index = (seed as usize) % changed.len();
            changed[index] ^= (seed >> 32) as u8;
            let changed: Bytes = changed.into();
            let projected = request_id(changed.clone(), raw.len());
            if let Ok(id) = &projected {
                assert!(!id.is_empty() && id.len() <= 128);
            }
            if let Ok(full) = pb::Frame::decode(changed)
                && full.version == 1
                && let Ok(view) = full.to_json()
                && let Some(id) = view.get("id").and_then(serde_json::Value::as_str)
                && !id.is_empty()
                && id.len() <= 128
            {
                assert_eq!(projected.unwrap(), id);
            }
        }
    }
}
