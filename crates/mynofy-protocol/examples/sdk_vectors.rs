//! Public deterministic interoperability data, not operational credentials.
use mynofy_protocol::{ProtobufMessage, crypto, pb};
use serde_json::json;
fn main() {
    let inputs = vec![
        json!({"version":1,"type":"publish","id":"bytes","topic":"sdk/events","payload":{"kind":"bytes","data":"AP9oZWxsbw==","content_type":"application/octet-stream"}}),
        json!({"version":1,"type":"publish","id":"event","topic":"sdk/events","payload":{"kind":"event","catalog":1,"catalog_version":1,"category":12,"action":42,"status":2,"parameters":[{"u64":u64::MAX},{"i64":i64::MIN},{"bool":false}]}}),
        json!({"version":1,"type":"poll","id":"poll","topic":"sdk/events","after":"0000000Z","limit":100}),
        json!({"version":1,"type":"list_messages","id":"own","topic":"sdk/events","limit":10}),
        json!({"version":1,"type":"get_message","id":"get","message_id":"00000001"}),
        json!({"version":1,"type":"get_permissions","id":"acl","topic":"sdk/events"}),
        json!({"version":1,"type":"get_catalog","id":"catalog","name":"mynofy","catalog_version":1}),
        json!({"version":1,"type":"messages","request_id":"poll","items":[],"next_cursor":"0000000Z","has_more":false}),
        json!({"version":1,"type":"error","request_id":"pub","code":"rate_limited","detail":"wait","retry_after_ms":123}),
    ];
    let frames: Vec<_> = inputs
        .into_iter()
        .map(|value| {
            let frame = pb::Frame::from_json(value.clone()).unwrap();
            json!({"json":value,"protobuf_base64":crypto::encode(&frame.encode_to_vec())})
        })
        .collect();
    let public: [u8; 32] = std::array::from_fn(|i| i as u8);
    let nonce = [7; 32];
    let auth:Vec<_>=["owner","mynofy-ä"].into_iter().map(|owner|json!({"owner":owner,"public_key":crypto::encode(&public),"nonce":crypto::encode(&nonce),"token_id":crypto::token_id(&public),"client_id":crypto::initial_client_id(&public),"transcript_base64":crypto::encode(&crypto::auth_bytes(&nonce,&public,owner))})).collect();
    let aad = crypto::rotation_aad(
        "owner",
        "client:fixture",
        "rotation:1",
        "ed25519:old",
        1893456000123,
    );
    let rotation = json!({"owner":"owner","client_id":"client:fixture","rotation_id":"rotation:1","old_token":"ed25519:old","until_ms":1893456000123u64,"public_key":crypto::encode(&public),"challenge":crypto::encode(&nonce),"aad_base64":crypto::encode(&aad),"proof_base64":crypto::encode(&crypto::rotation_proof_bytes(&aad,&public,&nonce))});
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"version":1,"frames":frames,"auth":auth,"rotation":rotation})
        )
        .unwrap()
    );
}
