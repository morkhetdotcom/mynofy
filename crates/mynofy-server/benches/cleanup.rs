//! Cleanup-step and request latency during a backlog. All input construction is outside timers.
#![forbid(unsafe_code)]
mod support;
use mynofy_server::{
    BoxError,
    config::{Action, Client, Config, PublicKey, Rule},
    crypto,
    engine::Engine,
    output::ResponseBuffer,
    protocol::{
        self,
        payload::{Event, Parameter, ParameterValue, Payload},
        pb,
    },
};
use serde_json::json;
use std::time::Instant;
use support::*;
fn latency(samples: &[u64]) -> serde_json::Value {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let p = |n: usize| sorted[(sorted.len() - 1) * n / 100] as f64 / 1000.;
    json!({"p50":p(50),"p95":p(95),"p99":p(99),"max":p(100)})
}
async fn run(messages: usize, clients: usize) -> Result<(), BoxError> {
    let keys: Vec<_> = (0..clients)
        .map(|_| crypto::generate_key())
        .collect::<Result<_, _>>()?;
    let mut cfg: Config =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json"))?;
    cfg.clients = keys
        .iter()
        .map(|key| {
            let public = crypto::public_key(key)?;
            Ok(Client {
                disabled: false,
                client_id: crypto::initial_client_id(&public),
                display_name: None,
                keys: vec![PublicKey {
                    algorithm: "ed25519".into(),
                    public_key: crypto::encode(&public),
                    valid_until: protocol::timestamp(NOW + 86_400_000)?,
                }],
            })
        })
        .collect::<Result<_, BoxError>>()?;
    cfg.topics[0].policy.as_mut().unwrap().default_ttl_seconds = 1;
    cfg.topics[0].acl = Some(
        cfg.clients
            .iter()
            .map(|client| Rule {
                client_id: client.client_id.clone(),
                allow: vec![Action::Publish, Action::Subscribe, Action::InspectOwn],
            })
            .collect(),
    );
    cfg.limits.client_burst = 1_000_000;
    cfg.limits.client_requests_per_second = 1_000_000;
    cfg.limits.max_active_messages = messages + 1;
    cfg.limits.max_active_message_bytes = (messages + 1) * 4096;
    let mut engine = Engine::new(cfg, NOW)?;
    engine.start_id_allocator();
    let sessions: Vec<_> = keys
        .iter()
        .map(|key| authenticate(&mut engine, key))
        .collect();
    let mut output = ResponseBuffer::new(engine.config.limits.max_response_bytes);
    let payload = Payload::event(Event {
        catalog: 1,
        catalog_version: 1,
        category: 12,
        action: 10,
        status: 7,
        parameters: vec![
            Parameter {
                value: Some(ParameterValue::U64(4711)),
            },
            Parameter {
                value: Some(ParameterValue::U32(1)),
            },
            Parameter {
                value: Some(ParameterValue::Bool(true)),
            },
        ],
    });
    let raw = encode_request(
        frame(pb::Body::Publish(pb::Publish {
            id: "p".into(),
            topic: "local/events".into(),
            payload: payload.clone(),
            expires: None,
        })),
        "protobuf",
    );
    let live = encode_request(
        frame(pb::Body::Publish(pb::Publish {
            id: "live".into(),
            topic: "local/events".into(),
            payload,
            expires: Some(pb::Expires {
                time: protocol::timestamp(NOW + 60_000)?,
            }),
        })),
        "protobuf",
    );
    let mut setup = Vec::with_capacity(messages + 1);
    for i in 0..messages {
        request_at(
            &mut engine,
            sessions[0],
            &raw,
            &mut setup,
            "protobuf",
            Some(&mut output),
            NOW,
        )
        .check_result();
        if i % 256 == 255 {
            tokio::task::yield_now().await;
        }
    }
    let mut last_cursor = String::new();
    for session in &sessions {
        let mut cursor = None;
        let mut seen = 0;
        loop {
            let raw = encode_request(
                frame(pb::Body::Poll(pb::Poll {
                    id: "poll".into(),
                    topic: "local/events".into(),
                    after: cursor,
                    limit: Some(100),
                })),
                "protobuf",
            );
            let (references, next_cursor, more) = request_at(
                &mut engine,
                *session,
                &raw,
                &mut setup,
                "protobuf",
                Some(&mut output),
                NOW,
            )
            .page();

            seen += references.len();
            cursor = Some(next_cursor.clone());
            last_cursor = next_cursor;
            if !more {
                break;
            }
        }
        assert_eq!(seen, messages);
    }
    request_at(
        &mut engine,
        sessions[0],
        &live,
        &mut setup,
        "protobuf",
        Some(&mut output),
        NOW,
    )
    .check_result();
    let raw = encode_request(
        frame(pb::Body::Poll(pb::Poll {
            id: "live-poll".into(),
            topic: "local/events".into(),
            after: Some(last_cursor),
            limit: Some(100),
        })),
        "protobuf",
    );
    let (items, head, _) = request_at(
        &mut engine,
        sessions[0],
        &raw,
        &mut setup,
        "protobuf",
        Some(&mut output),
        NOW,
    )
    .page();
    assert_eq!(items.len(), 1);
    let probe = encode_request(
        frame(pb::Body::Poll(pb::Poll {
            id: "head".into(),
            topic: "local/events".into(),
            after: Some(head),
            limit: Some(100),
        })),
        "protobuf",
    );
    let old_get = encode_request(
        frame(pb::Body::GetMessage(pb::GetMessage {
            id: "expired".into(),
            message_id: "00000000".into(),
        })),
        "protobuf",
    );
    let now = NOW + 1000;
    {
        let (raw, code) = (&old_get, "not_found");
        let Response::Binary(response) = request_at(
            &mut engine,
            sessions[0],
            raw,
            &mut setup,
            "protobuf",
            Some(&mut output),
            now,
        ) else {
            panic!("binary")
        };
        let Some(pb::Body::Error(error)) = response.body else {
            panic!("expired access accepted")
        };
        assert_eq!(error.code, code);
    }
    let before = engine.storage_stats();
    assert_eq!(before.active_records, 1);
    let mut steps = Vec::new();
    let mut probes = Vec::new();
    let mut work = 0;
    loop {
        let start = Instant::now();
        let progress = engine.cleanup_step(now, 512);
        steps.push(start.elapsed().as_nanos() as u64);
        assert!(progress.work <= 512);
        work += progress.work;
        assert!(
            request_at(
                &mut engine,
                sessions[0],
                &probe,
                &mut probes,
                "protobuf",
                Some(&mut output),
                now
            )
            .page()
            .0
            .is_empty()
        );
        if !progress.more {
            break;
        }
        assert!(steps.len() < messages * clients + 10);
    }
    let mut maintenance = Vec::new();
    loop {
        let start = Instant::now();
        let progress = engine.maintenance(now);
        maintenance.push(start.elapsed().as_nanos() as u64);
        if !progress.more {
            break;
        }
        assert!(maintenance.len() < 10000);
    }
    let after = engine.storage_stats();
    assert_eq!(after.active_records, 1);
    assert_eq!(after.resident_records, 1);
    println!(
        "{}",
        json!({"phase":"cleanup_backlog","messages":messages,"clients":clients,"request_generation_timed":false,"transport":"none","before":before,"after":after,"step_budget":512,"step_calls":steps.len(),"explicit_step_work":work,"step_us":latency(&steps),"step_compute_ms":steps.iter().sum::<u64>() as f64/1e6,"probe_us":latency(&probes),"probe_compute_ms":probes.iter().sum::<u64>() as f64/1e6,"maintenance_us":latency(&maintenance),"maintenance_compute_ms":maintenance.iter().sum::<u64>() as f64/1e6})
    );
    loop {
        let progress = engine.maintenance(NOW + 60_000);
        if !progress.more {
            break;
        }
    }
    assert_eq!(engine.storage_stats().resident_records, 0);
    Ok(())
}
fn main() -> Result<(), BoxError> {
    let mut messages = 100000;
    let mut clients = 1;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--messages" => messages = args.next().ok_or("messages")?.parse()?,
            "--clients" => clients = args.next().ok_or("clients")?.parse()?,
            "--bench" => {}
            _ => return Err(format!("Unknown argument {arg}").into()),
        }
    }
    if !(1..=100000).contains(&messages)
        || !(1..=512).contains(&clients)
        || messages * clients > 200000
    {
        return Err("Invalid workload bounds".into());
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(messages, clients))
}
