//! Shared benchmark helpers; request construction and client checking are outside operation timers.
#![allow(dead_code)]
use mynofy_server::{
    crypto,
    engine::Engine,
    protocol::{self, pb},
};
use protocol::ProtobufMessage as _;
use serde_json::{Value, json};
use std::{hint::black_box, time::Instant};
pub(crate) const NOW: u64 = 1_800_000_000_000;

pub(crate) fn checked(value: Value, expected: &str) -> Value {
    assert_eq!(value["type"], expected, "Unexpected response: {value}");
    value
}

// Measure parsing, authentication/ACL checks, core work, and response serialization.
pub(crate) fn encode_request(frame: pb::Frame, encoding: &str) -> Vec<u8> {
    if encoding == "protobuf" {
        frame.encode_to_vec()
    } else {
        serde_json::to_vec(&frame.to_json().unwrap()).unwrap()
    }
}
pub(crate) fn frame(body: pb::Body) -> pb::Frame {
    pb::Frame {
        version: 1,
        body: Some(body),
    }
}
// Keep typed responses in the binary benchmark: no driver-only JSON conversion.
// A stack value avoids introducing a benchmark-only heap allocation per binary response.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Response {
    Binary(pb::Frame),
    Json(Value),
}
impl Response {
    pub(crate) fn check_result(self) {
        match self {
            Self::Binary(frame) => {
                assert!(matches!(frame.body, Some(pb::Body::Result(_))), "{frame:?}")
            }
            Self::Json(value) => {
                checked(value, "result");
            }
        }
    }
    pub(crate) fn page(self) -> (Vec<String>, String, bool) {
        match self {
            Self::Binary(frame) => {
                let Some(pb::Body::Messages(page)) = frame.body else {
                    panic!("{frame:?}")
                };
                (
                    page.items.into_iter().map(|item| item.message.id).collect(),
                    page.next_cursor,
                    page.has_more,
                )
            }
            Self::Json(value) => {
                let page = checked(value, "messages");
                (
                    page["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|item| item["message"]["id"].as_str().unwrap().to_owned())
                        .collect(),
                    page["next_cursor"].as_str().unwrap().to_owned(),
                    page["has_more"].as_bool().unwrap(),
                )
            }
        }
    }
}
pub(crate) fn request(
    engine: &mut Engine,
    session: u64,
    raw: &[u8],
    samples: &mut Vec<u64>,
    encoding: &str,
    output: Option<&mut mynofy_server::output::ResponseBuffer>,
) -> Response {
    request_at(engine, session, raw, samples, encoding, output, NOW)
}
pub(crate) fn request_at(
    engine: &mut Engine,
    session: u64,
    raw: &[u8],
    samples: &mut Vec<u64>,
    encoding: &str,
    output: Option<&mut mynofy_server::output::ResponseBuffer>,
    now: u64,
) -> Response {
    // Input generation/encoding, response inspection and sample recording are outside this timer.
    let start = Instant::now();
    if encoding == "protobuf"
        && let Some(output) = output
    {
        engine.handle_protobuf_into(session, black_box(raw), now, output.prepare());
        let bytes = output.take();
        black_box(&bytes);
        let nanos = start.elapsed().as_nanos() as u64;
        // Client decoding and buffer reclamation happen outside the server operation timer.
        let response = pb::Frame::decode(bytes.as_ref()).unwrap();
        drop(bytes);
        output.sent();
        samples.push(nanos);
        return Response::Binary(response);
    }
    let response = if encoding == "protobuf" {
        let response = engine.handle_protobuf(session, black_box(raw), now);
        black_box(bytes::Bytes::from(response.encode_to_vec()));
        Response::Binary(response)
    } else {
        let response = engine.handle(session, black_box(std::str::from_utf8(raw).unwrap()), now);
        black_box(serde_json::to_vec(&response).unwrap());
        Response::Json(response)
    };
    let nanos = start.elapsed().as_nanos() as u64;
    samples.push(nanos);
    response
}
pub(crate) fn report(name: &str, samples: &mut [u64], messages: usize) {
    samples.sort_unstable();
    let percentile = |p: usize| samples[(samples.len() - 1) * p / 100] as f64 / 1000.0;
    let measured_ns = samples.iter().map(|&n| u128::from(n)).sum::<u128>();
    println!(
        "{}",
        json!({
            "phase":name, "requests":samples.len(), "messages":messages,
            "measured_requests_per_second":samples.len() as f64/(measured_ns as f64/1e9),
            "measured_messages_per_second":messages as f64/(measured_ns as f64/1e9),
            "measured_operation_ms":measured_ns as f64/1e6,
            "latency_us":{"p50":percentile(50),"p95":percentile(95),"p99":percentile(99),"max":samples[samples.len()-1] as f64/1000.0}
        })
    );
}
pub(crate) fn rss_bytes() -> Option<u64> {
    // Linux current process RSS, not virtual size or the owner's accounting budget.
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    })
}
pub(crate) fn authenticate(engine: &mut Engine, key: &[u8]) -> u64 {
    let session = engine.connect(NOW).unwrap();
    let public = crypto::public_key(key).unwrap();
    let challenge = checked(
        engine.handle(
            session,
            &json!({"version":1,"type":"auth_hello","id":"h","token_id":crypto::token_id(&public)})
                .to_string(),
            NOW,
        ),
        "auth_challenge",
    );
    let nonce = crypto::decode(challenge["nonce"].as_str().unwrap()).unwrap();
    let signature = crypto::sign(
        key,
        &crypto::auth_bytes(&nonce, &public, &engine.config().service.id),
    )
    .unwrap();
    checked(
        engine.handle(
            session,
            &json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":signature})
                .to_string(),
            NOW,
        ),
        "auth_welcome",
    );
    session
}
