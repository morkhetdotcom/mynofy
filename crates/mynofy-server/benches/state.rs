//! Serial multi-identity/replay/lifecycle workloads. No network; prepared protobuf input.
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

async fn run(
    messages: usize,
    clients: usize,
    topics: usize,
    rounds: usize,
    cycles: usize,
    unique: bool,
) -> Result<(), BoxError> {
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
    let base = cfg.topics[0].clone();
    cfg.topics = (0..topics)
        .map(|i| {
            let mut topic = base.clone();
            topic.name = format!("state/{i}");
            topic.acl = Some(
                cfg.clients
                    .iter()
                    .map(|client| Rule {
                        client_id: client.client_id.clone(),
                        allow: vec![Action::Publish, Action::Subscribe, Action::InspectOwn],
                    })
                    .collect(),
            );
            let policy = topic.policy.as_mut().unwrap();
            policy.default_ttl_seconds = 60;
            policy.max_ttl_seconds = 300;
            topic
        })
        .collect();
    cfg.limits.client_burst = 1_000_000;
    cfg.limits.client_requests_per_second = 1_000_000;
    cfg.limits.max_active_messages = messages;
    cfg.limits.max_active_message_bytes = messages * 4096;
    let mut engine = Engine::new(cfg, NOW)?;
    engine.start_id_allocator();
    let sessions: Vec<_> = keys
        .iter()
        .map(|key| authenticate(&mut engine, key))
        .collect();
    let mut outputs: Vec<_> = (0..clients)
        .map(|_| ResponseBuffer::new(engine.config.limits.max_response_bytes))
        .collect();
    // All publish messages for all cycles are generated and encoded before any operation timer.
    let prepared: Vec<Vec<Vec<u8>>> = (0..cycles)
        .map(|cycle| {
            (0..messages)
                .map(|i| {
                    let payload = Payload::event(Event {
                        catalog: 1,
                        catalog_version: 1,
                        category: 12,
                        action: 10,
                        status: 7,
                        parameters: vec![
                            Parameter {
                                value: Some(ParameterValue::U64(i as u64)),
                            },
                            Parameter {
                                value: Some(ParameterValue::U32(cycle as u32)),
                            },
                            Parameter {
                                value: Some(ParameterValue::Bool(true)),
                            },
                        ],
                    });
                    let deadline =
                        NOW + cycle as u64 * 1_000_000 + 60_000 + if unique { i as u64 } else { 0 };
                    encode_request(
                        frame(pb::Body::Publish(pb::Publish {
                            id: "p".into(),
                            topic: format!("state/{}", i % topics),
                            payload,
                            expires: Some(pb::Expires {
                                time: protocol::timestamp(deadline).unwrap(),
                            }),
                        })),
                        "protobuf",
                    )
                })
                .collect()
        })
        .collect();
    println!(
        "{}",
        json!({"phase":"configuration","messages":messages,"clients":clients,"topics":topics,"rounds":rounds,"cycles":cycles,"unique_deadlines":unique,"serial":true,"transport":"none","request_generation_timed":false,"prepared_input_bytes":prepared.iter().flatten().map(Vec::len).sum::<usize>()})
    );
    let rss_baseline = rss_bytes();
    for (cycle, publish_inputs) in prepared.iter().enumerate() {
        let now = NOW + cycle as u64 * 1_000_000;
        let mut publish_samples = Vec::with_capacity(messages);
        let mut first_poll = Vec::with_capacity(messages * clients);
        let mut repeat_poll = Vec::with_capacity(messages * clients * rounds.saturating_sub(1));
        let mut first_message = None;
        for (i, raw) in publish_inputs.iter().enumerate() {
            let response = request_at(
                &mut engine,
                sessions[0],
                raw,
                &mut publish_samples,
                "protobuf",
                Some(&mut outputs[0]),
                now,
            );
            if i == 0 {
                let Response::Binary(value) = response else {
                    panic!("binary expected")
                };
                let Some(pb::Body::Result(result)) = value.body else {
                    panic!("publish failed")
                };
                first_message = result.message_id;
            } else {
                response.check_result();
            }
            if i % 256 == 255 {
                tokio::task::yield_now().await;
            }
        }
        let after_publish = engine.storage_stats();
        let mut known: Vec<Vec<String>> =
            (0..clients).map(|_| Vec::with_capacity(messages)).collect();
        let mut head_requests: Vec<Vec<Vec<u8>>> =
            (0..clients).map(|_| Vec::with_capacity(topics)).collect();
        for round in 0..rounds {
            for client in 0..clients {
                let mut offset = 0;
                for topic in 0..topics {
                    let mut cursor = None;
                    loop {
                        let raw = encode_request(
                            frame(pb::Body::Poll(pb::Poll {
                                id: "poll".into(),
                                topic: format!("state/{topic}"),
                                after: cursor,
                                limit: Some(100),
                            })),
                            "protobuf",
                        );
                        let samples = if round == 0 {
                            &mut first_poll
                        } else {
                            &mut repeat_poll
                        };
                        let (references, next_cursor, more) = request_at(
                            &mut engine,
                            sessions[client],
                            &raw,
                            samples,
                            "protobuf",
                            Some(&mut outputs[client]),
                            now,
                        )
                        .page();
                        if round == 0 {
                            known[client].extend(references.iter().cloned());
                        } else {
                            assert_eq!(
                                &known[client][offset..offset + references.len()],
                                &references
                            );
                        }
                        offset += references.len();

                        if !more {
                            if round == 0 {
                                head_requests[client].push(encode_request(
                                    frame(pb::Body::Poll(pb::Poll {
                                        id: "head".into(),
                                        topic: format!("state/{topic}"),
                                        after: Some(next_cursor),
                                        limit: Some(100),
                                    })),
                                    "protobuf",
                                ));
                            }
                            break;
                        }
                        cursor = Some(next_cursor);
                    }
                }
                assert_eq!(offset, messages);
            }
        }
        let mut head_samples = Vec::with_capacity(1000);
        for i in 0..1000 {
            let client = i % clients;
            let topic = i % topics;
            assert!(
                request_at(
                    &mut engine,
                    sessions[client],
                    &head_requests[client][topic],
                    &mut head_samples,
                    "protobuf",
                    Some(&mut outputs[client]),
                    now
                )
                .page()
                .0
                .is_empty()
            );
        }
        let _status=engine.handle(sessions[0],&json!({"version":1,"type":"get_message","id":"check","message_id":first_message.unwrap()}).to_string(),now);

        report("publish", &mut publish_samples, messages);
        report("first_poll", &mut first_poll, messages * clients);
        if rounds > 1 {
            report(
                "repeat_poll",
                &mut repeat_poll,
                messages * clients * (rounds - 1),
            );
        }
        report("poll_at_head", &mut head_samples, 0);
        let active = engine.storage_stats();
        let rss_active = rss_bytes();
        let start = Instant::now();
        engine.sweep(now + 60_000 + if unique { messages as u64 } else { 0 });
        let sweep_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut reclaim_ms = 0.;
        for _ in 0..10_000 {
            let start = Instant::now();
            let progress = engine.maintenance(now + 60_000 + messages as u64);
            reclaim_ms += start.elapsed().as_secs_f64() * 1000.;
            if !progress.more && engine.storage_stats().record_capacity == 0 {
                break;
            }
        }
        let reclaimed = engine.storage_stats();
        assert_eq!(reclaimed.active_records, 0);
        assert_eq!(reclaimed.record_capacity, 0);
        println!(
            "{}",
            json!({"phase":"lifecycle","cycle":cycle,"after_publish":after_publish,"after_poll":active,"after_expiry":reclaimed,"rss_baseline_bytes":rss_baseline,"rss_active_bytes":rss_active,"rss_after_reclaim_bytes":rss_bytes(),"expiry_sweep_ms":sweep_ms,"expiry_scope":"first_bounded_request_step","reclaim_ms":reclaim_ms})
        );
    }
    Ok(())
}
fn main() -> Result<(), BoxError> {
    let mut messages = 10000;
    let mut clients = 1;
    let mut topics = 1;
    let mut rounds = 2;
    let mut cycles = 3;
    let mut unique = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--messages" => messages = args.next().ok_or("messages")?.parse()?,
            "--clients" => clients = args.next().ok_or("clients")?.parse()?,
            "--topics" => topics = args.next().ok_or("topics")?.parse()?,
            "--rounds" => rounds = args.next().ok_or("rounds")?.parse()?,
            "--cycles" => cycles = args.next().ok_or("cycles")?.parse()?,
            "--unique-deadlines" => unique = true,
            "--bench" => {}
            _ => return Err(format!("Unknown argument {arg}").into()),
        }
    }
    if !(1..=100000).contains(&messages)
        || !(1..=16).contains(&clients)
        || !(1..=16).contains(&topics)
        || messages < topics
        || !(1..=10).contains(&rounds)
        || !(1..=5).contains(&cycles)
        || messages * clients * rounds > 2_000_000
    {
        return Err("Invalid workload bounds".into());
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(messages, clients, topics, rounds, cycles, unique))
}
