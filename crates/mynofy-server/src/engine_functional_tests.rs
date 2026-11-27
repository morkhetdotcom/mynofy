//! Independent small reference model: public command results plus resident-index invariants.
use super::*;
use crate::test_support::{self, Rng};
use std::collections::HashMap;
const TOPICS: [&str; 2] = ["local/events", "other"];
// Fixed test identities keep the fuzz corpus reproducible; never use these keys in a service.
fn functional_fixture() -> Fixture {
    let mut fixture = super::fixture();
    fixture.keys = (1..=2)
        .map(|byte| {
            aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(&[byte; 32])
                .unwrap()
                .to_pkcs8()
                .unwrap()
                .as_ref()
                .to_vec()
        })
        .collect();
    fixture.ids = fixture
        .keys
        .iter()
        .map(|key| crypto::initial_client_id(&crypto::public_key(key).unwrap()))
        .collect();
    for (index, client) in fixture.cfg.clients.iter_mut().enumerate() {
        client.client_id = fixture.ids[index].clone();
        client.keys[0].public_key =
            crypto::encode(&crypto::public_key(&fixture.keys[index]).unwrap());
    }
    for topic in &mut fixture.cfg.topics {
        for (index, rule) in topic.acl.as_mut().unwrap().iter_mut().enumerate() {
            rule.client_id = fixture.ids[index].clone();
        }
    }
    fixture
}
struct Message {
    id: String,
    producer: usize,
    topic: usize,
    value: String,
    until: u64,
    seq: u64,
}
struct Model {
    messages: Vec<Message>,
    rights: [[u8; 2]; 2],
    heads: [u64; 2],
    produced: [u64; 2],
    now: u64,
}
impl Model {
    fn check(&self, engine: &Engine, ids: &[String]) {
        assert_eq!(engine.index.len(), engine.records.len());
        assert_eq!(
            engine.active_bytes,
            engine
                .records
                .iter()
                .map(|r| r.charge(&engine.content_types, &engine.payloads))
                .sum::<usize>()
        );
        assert_eq!(
            engine
                .topics
                .values()
                .map(|t| t.messages.len())
                .sum::<usize>(),
            engine.records.len()
        );
        assert_eq!(
            engine.own.values().map(OrderedIndex::len).sum::<usize>(),
            engine.records.len()
        );
        let modeled: HashMap<_, _> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id.as_str(), i))
            .collect();
        for (slot, record) in engine.records.iter_slots() {
            let text = record.id().to_string();
            let message = &self.messages[modeled[text.as_str()]];
            assert_eq!(engine.index.get(&record.id()), Some(&slot));
            assert_eq!(record.until(), message.until);
            assert_eq!(record.seq(), message.seq);
            assert_eq!(
                engine.topics[TOPICS[message.topic]]
                    .messages
                    .get(record.seq()),
                Some(&slot)
            );
            assert_eq!(
                engine.own[&ids[message.producer]].get(record.own_pos()),
                Some(&slot)
            );
            assert_eq!(
                engine.client_names[record.producer_slot()],
                ids[message.producer]
            );
            assert_eq!(
                engine.topic_names[record.topic() as usize],
                TOPICS[message.topic]
            );
            assert_eq!(
                record
                    .payload()
                    .payload(&engine.content_types, &engine.payloads)
                    .to_json()
                    .unwrap(),
                message.value
            );
        }
        for (_i, message) in self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.until > self.now)
        {
            let id = Id::parse(&message.id).unwrap();
            assert!(
                engine.index.contains_key(&id),
                "active message disappeared: {}",
                message.id
            );
        }
        for (client, id) in ids.iter().enumerate() {
            assert_eq!(engine.clients[id].produced, self.produced[client]);
        }
        for (topic, name) in TOPICS.iter().enumerate() {
            assert_eq!(engine.topics[*name].head, self.heads[topic]);
        }

        assert!(engine.records.len() <= engine.config.limits.max_active_messages);
    }
}
fn decode_cursor(text: &str) -> u64 {
    text.bytes().fold(0, |value, byte| {
        value * 62
            + u64::from(match byte {
                b'0'..=b'9' => byte - b'0',
                b'A'..=b'Z' => byte - b'A' + 10,
                b'a'..=b'z' => byte - b'a' + 36,
                _ => panic!("invalid cursor"),
            })
    })
}
fn list_all(engine: &mut Engine, session: u64, client: usize, topic: Option<usize>, model: &Model) {
    let expected: Vec<_> = model
        .messages
        .iter()
        .filter(|m| {
            m.producer == client && m.until > model.now && topic.is_none_or(|t| m.topic == t)
        })
        .map(|m| m.id.clone())
        .collect();
    let mut cursor = None;
    let mut actual = Vec::new();
    for _ in 0..=model.messages.len() {
        let mut request = json!({"version":1,"type":"list_messages","id":"list","limit":3});
        if let Some(topic) = topic {
            request["topic"] = TOPICS[topic].into();
        }
        if let Some(cursor) = cursor.take() {
            request["cursor"] = cursor;
        }
        let reply = send(engine, session, request, model.now);
        assert_eq!(reply["type"], "messages_page", "{reply}");
        for item in reply["items"].as_array().unwrap() {
            let id = item["id"].as_str().unwrap();
            let (_index, _message) = model
                .messages
                .iter()
                .enumerate()
                .find(|(_, m)| m.id == id)
                .unwrap();
            actual.push(id.to_owned());
        }
        cursor = reply.get("next_cursor").cloned();
        if cursor.is_none() {
            assert_eq!(actual, expected);
            return;
        }
    }
    panic!("producer pagination failed to terminate");
}
fn run_case(seed: u64, steps: usize) {
    let mut rng = Rng(seed);
    let mut fixture = functional_fixture();
    fixture.cfg.limits.client_burst = 1_000_000;
    fixture.cfg.limits.client_requests_per_second = 1_000_000;
    fixture.cfg.limits.max_active_messages = 8 * (1 + seed as usize % 6);
    fixture.cfg.limits.max_topic_active_messages = Some(fixture.cfg.limits.max_active_messages);
    fixture.cfg.limits.max_publisher_active_messages = Some(fixture.cfg.limits.max_active_messages);
    fixture.cfg.limits.max_response_bytes = 65536;
    for topic in &mut fixture.cfg.topics {
        topic.policy = Some(Policy {
            default_ttl_seconds: 2,
            max_ttl_seconds: 5,
        });
    }
    let mut engine = Engine::new(fixture.cfg, NOW).unwrap();
    let mut sessions = [
        auth(&mut engine, &fixture.keys[0], NOW),
        auth(&mut engine, &fixture.keys[1], NOW),
    ];
    let mut model = Model {
        messages: Vec::new(),
        rights: [[7; 2]; 2],
        heads: [0; 2],
        produced: [0; 2],
        now: NOW,
    };
    let mut cursors: [[Option<String>; 2]; 2] = Default::default();
    let mut trace = std::collections::VecDeque::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for step in 0..steps {
            let operation = rng.pick(16);
            let client = rng.pick(2);
            let topic = rng.pick(2);
            trace.push_back(format!(
                "{step}: op={operation} client={client} topic={topic} time={}",
                model.now
            ));
            if trace.len() > 24 {
                trace.pop_front();
            }
            match operation {
                0..=3 => {
                    let value = format!("value-{step}-{}", rng.next());
                    let valid = rng.pick(8) != 0;
                    let ttl = 1 + rng.pick(5) as u64;
                    let until = if valid {
                        model.now + ttl * 1000
                    } else {
                        model.now
                    };
                    let reply = send(
                        &mut engine,
                        sessions[client],
                        json!({"version":1,"type":"publish","id":"pub","topic":TOPICS[topic],"payload":value,"expires":{"time":protocol::timestamp(until).unwrap()}}),
                        model.now,
                    );
                    if model.rights[client][topic] & 1 == 0 {
                        assert_eq!(reply["code"], "forbidden");
                    } else if !valid {
                        assert_eq!(reply["code"], "invalid_frame");
                    } else if reply["type"] == "error" {
                        assert_eq!(reply["code"], "resource_exhausted", "{reply}");
                        assert_eq!(
                            engine.records.len(),
                            engine.config.limits.max_active_messages,
                            "unexpected rejection"
                        );
                    } else {
                        assert_eq!(reply["type"], "result", "{reply}");
                        let id = reply["message_id"].as_str().unwrap().to_owned();
                        assert!(Id::parse(&id).is_some());
                        assert!(model.messages.iter().all(|m| m.id != id));
                        assert_eq!(
                            protocol::parse_time(reply["expires"]["time"].as_str().unwrap())
                                .unwrap(),
                            until
                        );
                        model.heads[topic] += 1;
                        model.produced[client] += 1;
                        model.messages.push(Message {
                            id,
                            producer: client,
                            topic,
                            value,
                            until,
                            seq: model.heads[topic],
                        });
                    }
                }
                4..=6 => {
                    let choice = rng.pick(4);
                    let after = match choice {
                        0 => None,
                        1 => cursors[client][topic].clone(),
                        2 => Some("invalid".into()),
                        _ => Some("zzzzzzzz".into()),
                    };
                    let start = if choice == 1 {
                        after.as_deref().map_or(0, decode_cursor)
                    } else {
                        0
                    };
                    let limit = 1 + rng.pick(5);
                    let expected: Vec<_> = model
                        .messages
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.topic == topic && m.until > model.now && m.seq > start)
                        .take(limit)
                        .map(|(i, _)| i)
                        .collect();
                    let mut request = json!({"version":1,"type":"poll","id":"poll","topic":TOPICS[topic],"limit":limit});
                    if let Some(after) = after {
                        request["after"] = after.into();
                    }
                    let reply = send(&mut engine, sessions[client], request, model.now);
                    if model.rights[client][topic] & 2 == 0 {
                        assert_eq!(reply["code"], "forbidden");
                    } else {
                        assert_eq!(reply["type"], "messages", "{reply}");
                        let items = reply["items"].as_array().unwrap();
                        assert_eq!(items.len(), expected.len());
                        for (item, index) in items.iter().zip(expected) {
                            assert_eq!(item["message"]["id"], model.messages[index].id);
                            assert_eq!(item["message"]["payload"], model.messages[index].value);
                        }
                        let cursor = reply["next_cursor"].as_str().unwrap();
                        assert!(
                            decode_cursor(cursor) >= start
                                && decode_cursor(cursor) <= model.heads[topic]
                        );
                        if reply["has_more"] == false {
                            assert_eq!(decode_cursor(cursor), model.heads[topic]);
                        }
                        cursors[client][topic] = Some(cursor.to_owned());
                    }
                }

                8 if !model.messages.is_empty() => {
                    let index = rng.pick(model.messages.len());
                    let message = &model.messages[index];
                    let reply = get(&mut engine, sessions[client], &message.id, model.now);
                    if message.producer != client || message.until <= model.now {
                        assert_eq!(reply["code"], "not_found");
                    } else {
                        assert_eq!(reply["message"]["payload"], message.value);
                    }
                }
                9 => {
                    let filter = if rng.pick(2) == 0 { None } else { Some(topic) };
                    list_all(&mut engine, sessions[client], client, filter, &model);
                }
                10 => {
                    model.now += rng.pick(3001) as u64;
                    engine.sweep(model.now);
                }
                11 => {
                    let bits = rng.pick(8) as u8;
                    let actions = [Action::Publish, Action::Subscribe, Action::InspectOwn]
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| bits & (1 << i) != 0)
                        .map(|(_, action)| action)
                        .collect();
                    let old = sessions[client];
                    engine
                        .set_acl(&fixture.ids[client], TOPICS[topic], actions)
                        .unwrap();
                    model.rights[client][topic] = bits;
                    assert!(!engine.authenticated(old, model.now));
                    let denied = publish(&mut engine, old, json!("stale session"), model.now);
                    assert_eq!(denied["code"], "unauthorized");
                    engine.disconnect(old);
                    sessions[client] = auth(&mut engine, &fixture.keys[client], model.now);
                }
                12 => {
                    engine.disconnect(sessions[client]);
                    sessions[client] = auth(&mut engine, &fixture.keys[client], model.now);
                }
                13 => {
                    let raw = if rng.pick(2) == 0 {
                        r#"{"version":1,"type":"publish","id":"bad","topic":"local/events","payload":"x","reply":"forbidden field"}"#
                    } else {
                        r#"{"version":1,"version":1,"type":"poll","id":"bad","topic":"local/events"}"#
                    };
                    assert_eq!(
                        engine.handle(sessions[client], raw, model.now)["code"],
                        "invalid_frame"
                    );
                }
                14 => {
                    let budget = rng.pick(9);
                    let progress = engine.cleanup_step(model.now, budget);
                    assert!(progress.work <= budget);
                }
                _ => {
                    let backwards = model.now.saturating_sub(60_000);
                    engine.sweep(backwards);
                    assert_eq!(engine.last_cleanup_now, model.now);
                }
            }
            model.check(&engine, &fixture.ids);
        }
    }));
    if let Err(error) = result {
        eprintln!(
            "Replay MYNOFY_TEST_SEED=0x{seed:016x}; steps={steps}; last operations:\n{}",
            trace.into_iter().collect::<Vec<_>>().join("\n")
        );
        std::panic::resume_unwind(error);
    }
}
#[test]
fn seeded_reference_model_checks_core_state_transitions() {
    let seeds = test_support::setting("MYNOFY_MODEL_SEEDS", 16, 4096);
    let steps = test_support::setting("MYNOFY_MODEL_STEPS", 600, 1500);
    let first = test_support::seed();
    for case in 0..seeds {
        run_case(first.wrapping_add(case as u64), steps);
    }
    eprintln!("Core model: {seeds} seeds x {steps} steps, starting seed 0x{first:016x}");
}

#[test]
fn functional_seeded_checkpoint_mutations_validate_controls_and_embedded_images() {
    use crate::content::{Checkpoint, checkpoint::TopicState};
    let f = functional_fixture();
    let mut engine = Engine::new(f.cfg.clone(), NOW).unwrap();
    let session = auth(&mut engine, &f.keys[0], NOW);
    for index in 0..12 {
        super::topic_publish(
            &mut engine,
            session,
            TOPICS[index % 2],
            &format!("checkpoint-{index}"),
            NOW,
        );
    }
    for topic in TOPICS {
        super::topic_poll(&mut engine, session, topic, NOW);
    }
    let checkpoint = Checkpoint {
        image: engine.capture_message_image(Default::default()).unwrap(),
        revision: 7,
        next_message: engine.next_message,
        reserved_ids: engine.message_ids.reserved_position(),
        cutoff: NOW,
        produced: engine
            .client_names
            .iter()
            .map(|name| engine.clients[name].produced)
            .collect(),
        topics: engine
            .topic_names
            .iter()
            .map(|name| TopicState {
                head: engine.topics[name].head,
                policy: engine.topics[name].policy.clone(),
            })
            .collect(),
    };
    let original = checkpoint.write(256 * 1024).unwrap();
    let mut rng = Rng(test_support::seed());
    let cases = test_support::fuzz_cases();
    let mut accepted = 0;
    for case in 0..cases {
        let mut bytes = if case % 16 == 0 {
            original.to_vec()
        } else {
            rng.mutate(&original, 256 * 1024)
        };
        if case % 2 == 0 && bytes.len() >= 128 {
            let payload = crypto::sha256(&bytes[128..]);
            bytes[64..96].copy_from_slice(&payload);
            let header = crypto::sha256(&bytes[..96]);
            bytes[96..128].copy_from_slice(&header);
        }
        let outcome = std::panic::catch_unwind(|| Checkpoint::read(&bytes, &f.cfg, 256 * 1024));
        if let Ok(decoded) = outcome.unwrap_or_else(|_| {
            panic!(
                "Checkpoint panic: seed=0x{:016x}, case={case}",
                test_support::seed()
            )
        }) {
            accepted += 1;
            assert_eq!(decoded.write(256 * 1024).unwrap().as_slice(), bytes);
        }
    }
    assert!(accepted > 0);
    eprintln!(
        "Checkpoint mutations: {cases} cases, {accepted} accepted and roundtripped; outer checksums repaired on alternate cases"
    );
}

#[test]
fn functional_seeded_json_typed_and_direct_protobuf_have_the_same_effects() {
    use protocol::payload::{Event, Parameter, ParameterValue};
    let mut f = functional_fixture();
    f.cfg.limits.client_burst = 1_000_000;
    f.cfg.limits.client_requests_per_second = 1_000_000;
    f.cfg.limits.max_response_bytes = 4 * 1024 * 1024;
    let mut engines = [
        Engine::new(f.cfg.clone(), NOW).unwrap(),
        Engine::new(f.cfg.clone(), NOW).unwrap(),
        Engine::new(f.cfg, NOW).unwrap(),
    ];
    for engine in &mut engines {
        engine.generation = "functional-comparison".into();
    }
    let sessions: Vec<_> = engines
        .iter_mut()
        .map(|engine| [auth(engine, &f.keys[0], NOW), auth(engine, &f.keys[1], NOW)])
        .collect();
    let mut rng = Rng(test_support::seed());
    let cases = test_support::fuzz_cases().min(1500);
    let mut now = NOW;
    let mut ids: Vec<String> = Vec::new();
    let mut cursor = None;
    for case in 0..cases {
        let client = rng.pick(2);
        let topic = TOPICS[rng.pick(2)];
        let body = match rng.pick(8) {
            0..=2 => {
                let payload = if rng.pick(2) == 0 {
                    let size = [0, 1, 15, 16, 255, 4096, 4097][rng.pick(7)];
                    Payload::bytes(
                        vec![rng.next() as u8; size],
                        Some("application/octet-stream".into()),
                    )
                } else {
                    let values = [
                        ParameterValue::I64(i64::MIN),
                        ParameterValue::U64(u64::MAX),
                        ParameterValue::F64(-0.0),
                        ParameterValue::Bool(true),
                        ParameterValue::F32(f32::MAX),
                        ParameterValue::I32(i32::MAX),
                    ];
                    Payload::event(Event {
                        catalog: 1,
                        catalog_version: 1,
                        category: 12,
                        action: 10,
                        status: 7,
                        parameters: (0..rng.pick(5))
                            .map(|_| Parameter {
                                value: Some(values[rng.pick(values.len())].clone()),
                            })
                            .collect(),
                    })
                };
                pb::Body::Publish(pb::Publish {
                    id: "unicode-ä-🤖".into(),
                    topic: topic.into(),
                    payload,
                    expires: Some(pb::Expires {
                        time: protocol::timestamp(now + 1000).unwrap(),
                    }),
                })
            }
            3 => pb::Body::Poll(pb::Poll {
                id: "poll".into(),
                topic: topic.into(),
                after: if rng.pick(3) == 0 {
                    cursor.clone()
                } else {
                    None
                },
                limit: Some(1 + rng.pick(20) as u32),
            }),
            4..=6 => pb::Body::GetMessage(pb::GetMessage {
                id: "get".into(),
                message_id: if ids.is_empty() {
                    "00000000".into()
                } else {
                    ids[rng.pick(ids.len())].clone()
                },
            }),
            _ => pb::Body::GetPermissions(pb::GetPermissions {
                id: "permissions".into(),
                topic: topic.into(),
            }),
        };
        let frame = pb::Frame {
            version: 1,
            body: Some(body),
        };
        let raw = frame.encode_to_vec();
        let json = engines[0].handle(
            sessions[0][client],
            &frame.to_json().unwrap().to_string(),
            now,
        );
        let typed = engines[1].handle_protobuf(sessions[1][client], &raw, now);
        let mut buffer = bytes::BytesMut::new();
        engines[2].handle_protobuf_into(sessions[2][client], &raw, now, &mut buffer);
        assert_eq!(
            typed.encode_to_vec(),
            buffer.as_ref(),
            "direct encoder case {case}"
        );
        assert_eq!(
            typed.to_json().unwrap(),
            json,
            "JSON/protobuf semantics at case {case}"
        );
        if let Some(id) = json["message_id"].as_str() {
            ids.push(id.to_owned());
        }

        if let Some(value) = json["next_cursor"].as_str() {
            cursor = Some(value.to_owned());
        }
        if case % 31 == 0 {
            now += 1001;
        }
        for engine in &mut engines {
            engine.maintenance(now);
        }
    }
    eprintln!(
        "JSON/typed/direct differential checks: {cases} requests including bytes/events, payload boundaries, expiry and confirmations"
    );
}

#[test]
fn functional_seeded_protobuf_and_json_bytes_cannot_publish_without_authentication() {
    let mut f = functional_fixture();
    f.cfg.limits.client_burst = 1_000_000;
    let mut engine = Engine::new(f.cfg, NOW).unwrap();
    let mut rng = Rng(test_support::seed());
    let cases = test_support::fuzz_cases();
    let corpus = [
        json!({"version":1,"type":"publish","id":"pub","topic":"local/events","payload":"not authorized"}),
        json!({"version":1,"type":"poll","id":"poll","topic":"local/events"}),
    ];
    for case in 0..cases {
        let command = &corpus[rng.pick(corpus.len())];
        let binary = pb::Frame::from_json(command.clone())
            .unwrap()
            .encode_to_vec();
        let binary = rng.mutate(&binary, 4096);
        let session = engine.connect(NOW).unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut output = bytes::BytesMut::new();
            engine.handle_protobuf_into(session, &binary, NOW, &mut output);
            let frame = pb::Frame::decode(output.as_ref()).unwrap();
            assert_eq!(frame.kind(), "error");
            let raw = rng.mutate(command.to_string().as_bytes(), 4096);
            if let Ok(text) = std::str::from_utf8(&raw) {
                assert_eq!(engine.handle(session, text, NOW)["type"], "error");
            }
        }));
        outcome.unwrap_or_else(|_| {
            panic!(
                "Unauthenticated frame panic: seed=0x{:016x}, case={case}, bytes={binary:?}",
                test_support::seed()
            )
        });
        engine.disconnect(session);
        assert_eq!(engine.records.len(), 0);

        assert_eq!(engine.next_message, 0);
    }
    eprintln!("Unauthenticated frame mutations: {cases} binary/JSON pairs, no content mutations");
}
