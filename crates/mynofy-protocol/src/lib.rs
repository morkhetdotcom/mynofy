#![forbid(unsafe_code)]
//! Wire types, strict JSON parsing and optional shared authentication primitives.
mod wire;
pub use wire::*;
#[cfg(feature = "crypto")]
pub mod crypto;
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub mod catalog;

pub mod correlation;
pub mod payload;
#[path = "pb_generated.rs"]
pub mod pb;
mod pb_bridge;
pub use prost::Message as ProtobufMessage;

pub mod catalog_values;

#[cfg(test)]
mod protobuf_tests;

#[cfg(feature = "transport")]
pub mod tunnel;

#[cfg(all(test, feature = "crypto"))]
mod sdk_conformance {
    use super::*;
    #[test]
    fn shared_sdk_vectors_match_rust_wire_and_crypto_transcripts() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../../../protocol/conformance/sdk-v1.json"))
                .unwrap();
        for item in vectors["frames"].as_array().unwrap() {
            let frame = pb::Frame::from_json(item["json"].clone()).unwrap();
            assert_eq!(
                crypto::encode(&frame.encode_to_vec()),
                item["protobuf_base64"]
            );
        }
        for item in vectors["payload_validation"].as_array().unwrap() {
            let shape = &item["payload"];
            let body = if let Some(event) = shape.get("event") {
                payload::Body::Event(serde_json::from_value(event.clone()).unwrap())
            } else {
                payload::Body::Data(
                    base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        shape["data"].as_str().unwrap(),
                    )
                    .unwrap()
                    .into(),
                )
            };
            let payload = payload::Payload {
                content_type: shape
                    .get("content_type")
                    .map(|v| v.as_str().unwrap().into()),
                body: Some(body),
            };
            assert_eq!(
                payload.validate().is_ok(),
                item["valid"].as_bool().unwrap(),
                "{}",
                item["name"]
            );
        }
        for item in vectors["auth"].as_array().unwrap() {
            let nonce = crypto::decode(item["nonce"].as_str().unwrap()).unwrap();
            let public = crypto::decode(item["public_key"].as_str().unwrap()).unwrap();
            assert_eq!(
                crypto::encode(&crypto::auth_bytes(
                    &nonce,
                    &public,
                    item["owner"].as_str().unwrap()
                )),
                item["transcript_base64"]
            );
            assert_eq!(crypto::initial_client_id(&public), item["client_id"]);
        }
        let v = &vectors["rotation"];
        let aad = crypto::rotation_aad(
            v["owner"].as_str().unwrap(),
            v["client_id"].as_str().unwrap(),
            v["rotation_id"].as_str().unwrap(),
            v["old_token"].as_str().unwrap(),
            v["until_ms"].as_u64().unwrap(),
        );
        assert_eq!(crypto::encode(&aad), v["aad_base64"]);
        let public = crypto::decode(v["public_key"].as_str().unwrap()).unwrap();
        let challenge = crypto::decode(v["challenge"].as_str().unwrap()).unwrap();
        assert_eq!(
            crypto::encode(&crypto::rotation_proof_bytes(&aad, &public, &challenge)),
            v["proof_base64"]
        );
    }
}

#[cfg(test)]
mod timestamp_tests;
