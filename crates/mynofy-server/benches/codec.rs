//! Codec-only aggregate costs, not request latency or network throughput.
#![forbid(unsafe_code)]
use mynofy_protocol::{
    ProtobufMessage as _,
    payload::{Event, Parameter, ParameterValue, Payload},
    pb,
};
use serde_json::json;
use std::{hint::black_box, time::Instant};
fn main() {
    let iterations = 20_000;
    for (kind, size) in [
        ("event-empty", 0),
        ("event", 0),
        ("bytes", 32),
        ("bytes", 256),
        ("bytes", 4096),
    ] {
        let payload = if size == 0 {
            Payload::event(Event {
                catalog: 1,
                catalog_version: 1,
                category: 12,
                action: 10,
                status: 7,
                parameters: if kind == "event-empty" {
                    Vec::new()
                } else {
                    vec![
                        Parameter {
                            value: Some(ParameterValue::U64(4711)),
                        },
                        Parameter {
                            value: Some(ParameterValue::U32(1)),
                        },
                        Parameter {
                            value: Some(ParameterValue::Bool(true)),
                        },
                    ]
                },
            })
        } else {
            Payload::bytes(vec![0xff; size], None)
        };
        let frame = pb::Frame {
            version: 1,
            body: Some(pb::Body::Publish(pb::Publish {
                id: "pub".into(),
                topic: "local/events".into(),
                payload,
                expires: None,
            })),
        };
        let json_view = frame.to_json().unwrap();
        for codec in ["json", "protobuf"] {
            let bytes = if codec == "json" {
                serde_json::to_vec(&json_view).unwrap()
            } else {
                frame.encode_to_vec()
            };
            let start = Instant::now();
            for _ in 0..iterations {
                if codec == "json" {
                    black_box(serde_json::to_vec(black_box(&json_view)).unwrap());
                } else {
                    black_box(black_box(&frame).encode_to_vec());
                }
            }
            let encode_ns = start.elapsed().as_nanos() as f64 / iterations as f64;
            let start = Instant::now();
            for _ in 0..iterations {
                let decoded = if codec == "json" {
                    pb::Frame::from_json(
                        mynofy_protocol::strict_json(black_box(
                            std::str::from_utf8(&bytes).unwrap(),
                        ))
                        .unwrap(),
                    )
                    .unwrap()
                } else {
                    let decoded = pb::Frame::decode(black_box(bytes.as_slice())).unwrap();
                    let Some(pb::Body::Publish(ref p)) = decoded.body else {
                        panic!("Expected publish")
                    };
                    p.payload.validate().unwrap();
                    decoded
                };
                black_box(decoded);
            }
            let decode_ns = start.elapsed().as_nanos() as f64 / iterations as f64;
            println!(
                "{}",
                json!({"phase":"codec","codec":codec,"payload_kind":kind,"payload_bytes":size,"iterations":iterations,"wire_bytes":bytes.len(),"encode_ns_per_record":encode_ns,"decode_validate_ns_per_record":decode_ns})
            );
        }
    }
}
