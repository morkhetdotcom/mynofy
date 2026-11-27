//! Core lock contention under concurrent prepared Publish/Poll and administrative work.
#![forbid(unsafe_code)]
mod support;
use mynofy_server::{
    BoxError,
    admin::{Command, ResultBody},
    config::{
        Action, Client, Config, ContentEncryption, ContentStorage, PublicKey, Rule,
        SecurityStorage, TopicStorage,
    },
    crypto,
    engine::Engine,
    output::ResponseBuffer,
    protocol::{self, payload::Payload, pb},
};
use protocol::ProtobufMessage;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use support::{NOW, authenticate, frame};
struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Sample {
    phase: usize,
    start: u64,
    end: u64,
    wait: u64,
    hold: u64,
}
fn ns(start: Instant) -> u64 {
    start.elapsed().as_nanos() as u64
}
fn quantiles(v: &[u64]) -> serde_json::Value {
    if v.is_empty() {
        return json!({"samples":0});
    }
    let mut sorted = v.to_vec();
    sorted.sort_unstable();
    let q = |p: usize| sorted[(sorted.len() - 1) * p / 1000] as f64 / 1000.;
    json!({"samples":v.len(),"p50_us":q(500),"p95_us":q(950),"p99_us":q(990),"p999_us":q(999),"max_us":sorted[sorted.len()-1] as f64/1000.})
}
fn data(e: &mut Engine, c: Command, now: u64) -> serde_json::Value {
    let ResultBody::Data(v) = e.admin(c, now).unwrap() else {
        panic!("Expected data")
    };
    v
}
async fn run(n: usize, size: usize, operation: String) -> Result<(), BoxError> {
    let directory = Directory(std::env::temp_dir().join(format!(
        "mynofy-m1-{}-{}",
        std::process::id(),
        crypto::encode(&crypto::random::<8>()?).replace(['+', '/', '='], "")
    )));
    fs::create_dir(&directory.0)?;
    let keys: Vec<_> = (1..=2)
        .map(|b| {
            aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(&[b; 32])
                .unwrap()
                .to_pkcs8()
                .unwrap()
                .as_ref()
                .to_vec()
        })
        .collect();
    let mut cfg: Config =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json"))?;
    cfg.clients = keys
        .iter()
        .map(|key| {
            let p = crypto::public_key(key).unwrap();
            Client {
                client_id: crypto::initial_client_id(&p),
                disabled: false,
                display_name: None,
                keys: vec![PublicKey {
                    algorithm: "ed25519".into(),
                    public_key: crypto::encode(&p),
                    valid_until: protocol::timestamp(NOW + 86_400_000).unwrap(),
                }],
            }
        })
        .collect();
    let base = cfg.topics[0].clone();
    cfg.topics = ["load/bulk", "load/live"]
        .into_iter()
        .map(|name| {
            let mut t = base.clone();
            t.name = name.into();
            t.storage = if name.ends_with("bulk")
                || matches!(operation.as_str(), "save-selective" | "save-broad")
            {
                TopicStorage::Manual { encrypted: false }
            } else {
                TopicStorage::Ram {}
            };
            t.acl = Some(
                cfg.clients
                    .iter()
                    .map(|c| Rule {
                        client_id: c.client_id.clone(),
                        allow: vec![Action::Publish, Action::Subscribe],
                    })
                    .collect(),
            );
            t
        })
        .collect();
    if operation == "save-many" {
        for index in 0..1000 {
            let mut topic = base.clone();
            topic.name = format!("load/empty-{index}");
            topic.acl = Some(Vec::new());
            topic.storage = TopicStorage::Manual { encrypted: false };
            cfg.topics.push(topic);
        }
    }
    let many_topics: Vec<_> = cfg
        .topics
        .iter()
        .filter(|topic| topic.storage.persistent())
        .map(|topic| topic.name.clone())
        .collect();
    cfg.security_storage = Some(SecurityStorage {
        minimum_sequence: None,
        sync: false,
        directory: directory.0.join("security"),
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    cfg.content_storage = Some(ContentStorage {
        store_id: None,
        directory: directory.0.join("content"),
        encryption: ContentEncryption::None,
        max_snapshot_bytes: 512 * 1024 * 1024,
        autosave_retry: Default::default(),
    });
    cfg.limits.max_content_working_bytes = 1024 * 1024 * 1024;
    cfg.limits.client_burst = 1_000_000;
    cfg.limits.client_requests_per_second = 1_000_000;
    cfg.limits.max_active_messages = 3 * n;
    cfg.limits.max_topic_active_messages = Some(cfg.limits.max_active_messages);
    cfg.limits.max_publisher_active_messages = Some(cfg.limits.max_active_messages);
    cfg.limits.max_active_message_bytes = 3 * n * (size + 4096);
    cfg.limits.max_topic_active_bytes = Some(cfg.limits.max_active_message_bytes);
    cfg.limits.max_publisher_active_bytes = Some(cfg.limits.max_active_message_bytes);
    cfg.limits.max_response_bytes = 512 * 1024;
    let mut e = Engine::new(cfg, NOW)?;
    let sessions: Vec<_> = keys.iter().map(|k| authenticate(&mut e, k)).collect();
    let raw = |topic: &str, expires: Option<pb::Expires>| {
        frame(pb::Body::Publish(pb::Publish {
            id: "p".into(),
            topic: topic.into(),
            payload: Payload::bytes(vec![42; size], None),
            expires,
        }))
        .encode_to_vec()
    };
    let bulk = raw(
        "load/bulk",
        Some(pb::Expires {
            time: protocol::timestamp(NOW + 1000)?,
        }),
    );
    let live = raw("load/live", None);
    let mut out = ResponseBuffer::new(e.config().limits.max_response_bytes);
    for i in 0..n {
        let input = if i < 1000 { &live } else { &bulk };
        e.handle_protobuf_into(sessions[0], input, NOW, out.prepare());
        let bytes = out.take();
        assert!(matches!(
            pb::Frame::decode(bytes.clone())?.body,
            Some(pb::Body::Result(_))
        ));
        drop(bytes);
        out.sent();
        if i % 256 == 255 {
            tokio::task::yield_now().await;
        }
    }
    let e = Arc::new(Mutex::new(e));
    let done = Arc::new(AtomicBool::new(false));
    let phase = Arc::new(AtomicUsize::new(0));
    let produced = Arc::new(AtomicUsize::new(0));
    let epoch = Instant::now();
    let mut workers = Vec::new();
    for (i, &session) in sessions.iter().enumerate() {
        let engine = e.clone();
        let done = done.clone();
        let phase = phase.clone();
        let produced = produced.clone();
        let reclaim = operation == "reclaim";
        let input = if i == 0 {
            live.clone()
        } else {
            frame(pb::Body::Poll(pb::Poll {
                id: "poll".into(),
                topic: "load/live".into(),
                after: None,
                limit: Some(20),
            }))
            .encode_to_vec()
        };
        workers.push(thread::spawn(move || {
            let mut output = ResponseBuffer::new(512 * 1024);
            let mut samples = Vec::with_capacity(10000);
            while !done.load(Ordering::Acquire) {
                let p = phase.load(Ordering::Acquire);
                let now = NOW + if reclaim && p > 0 { 1000 } else { 0 };
                let start = ns(epoch);
                let mut guard = engine.lock().unwrap();
                let locked = ns(epoch);
                guard.handle_protobuf_into(session, &input, now, output.prepare());
                let bytes = output.take();
                let end = ns(epoch);
                if i == 0 {
                    produced.fetch_add(1, Ordering::Relaxed);
                }
                drop(guard);
                let response = pb::Frame::decode(bytes.clone()).unwrap();
                assert!(if i == 0 {
                    matches!(response.body, Some(pb::Body::Result(_)))
                } else {
                    matches!(response.body, Some(pb::Body::Messages(_)))
                });
                drop(bytes);
                output.sent();
                samples.push(Sample {
                    phase: p,
                    start,
                    end,
                    wait: locked - start,
                    hold: end - locked,
                });
                thread::sleep(Duration::from_micros(250));
            }
            samples
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    phase.store(1, Ordering::Release);
    let mut admin = Vec::new();
    let mut observations = Vec::new();
    for iteration in 0..5 {
        let requested = (operation == "save-many").then(|| many_topics.clone());
        let ready = ns(epoch);
        let now = NOW + if operation == "reclaim" { 1000 } else { 0 };
        let (start, end, records, result) = {
            let mut guard = e.lock().unwrap();
            let start = ns(epoch);
            let records = n + produced.load(Ordering::Relaxed);
            let result = match operation.as_str() {
                "control" => json!({}),
                "status" => data(&mut guard, Command::Status {}, now),
                "storage" => data(&mut guard, Command::StorageStats {}, now),
                "topic" => data(
                    &mut guard,
                    Command::TopicGet {
                        topic: "load/live".into(),
                    },
                    now,
                ),
                "topics" => data(
                    &mut guard,
                    Command::TopicsList {
                        after: Some("load/bulk".into()),
                        limit: Some(1),
                    },
                    now,
                ),
                "save-many" => data(&mut guard, Command::ContentSave { topics: requested }, now),
                "save-selective" => data(
                    &mut guard,
                    Command::ContentSave {
                        topics: Some(vec!["load/live".into()]),
                    },
                    now,
                ),
                "save" | "save-broad" => {
                    data(&mut guard, Command::ContentSave { topics: None }, now)
                }
                "reclaim" => serde_json::to_value(guard.maintenance(now))?,
                _ => return Err("Unknown operation".into()),
            };
            let end = ns(epoch);
            (start, end, records, result)
        };
        admin.push(json!({"iteration":iteration,"start_ns":start,"end_ns":end,"wait_ns":start-ready,"hold_us":(end-start) as f64/1000.,"records_before_expiry_cleanup":records}));
        if matches!(
            operation.as_str(),
            "save" | "save-selective" | "save-broad" | "save-many"
        ) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let (start, end, status) = {
                    let mut guard = e.lock().unwrap();
                    let start = ns(epoch);
                    let status = data(&mut guard, Command::Status {}, now);
                    (start, ns(epoch), status)
                };
                observations.push(
                    json!({"start_ns":start,"end_ns":end,"hold_us":(end-start) as f64/1000.}),
                );
                if status["content_storage"]["save_in_progress"] == false {
                    admin.last_mut().unwrap()["writer_duration_us"] =
                        status["content_storage"]["last_writer_duration_us"].clone();
                    assert!(
                        status["content_storage"]["last_error"].is_null(),
                        "{status}"
                    );
                    break;
                }
                assert!(Instant::now() < deadline, "Writer timed out");
                tokio::time::sleep(Duration::from_millis(125)).await;
            }
        }
        drop(result);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    phase.store(2, Ordering::Release);
    tokio::time::sleep(Duration::from_millis(200)).await;
    done.store(true, Ordering::Release);
    for (i, worker) in workers.into_iter().enumerate() {
        let samples = worker.join().unwrap();
        for p in 0..3 {
            let selected: Vec<_> = samples.iter().filter(|s| s.phase == p).collect();
            let durations: Vec<_> = selected.iter().map(|s| s.end - s.start).collect();
            let wait: Vec<_> = selected.iter().map(|s| s.wait).collect();
            let hold: Vec<_> = selected.iter().map(|s| s.hold).collect();
            println!(
                "{}",
                json!({"phase":"requests","worker":if i==0{"publish"}else{"poll"},"window":p,"latency":quantiles(&durations),"lock_wait":quantiles(&wait),"lock_hold":quantiles(&hold)})
            );
        }
        let overlap: Vec<_> = samples
            .iter()
            .filter(|s| {
                admin.iter().chain(&observations).any(|a| {
                    s.start < a["end_ns"].as_u64().unwrap()
                        && s.end > a["start_ns"].as_u64().unwrap()
                })
            })
            .collect();
        let times: Vec<_> = overlap.iter().map(|s| s.end - s.start).collect();
        let wait: Vec<_> = overlap.iter().map(|s| s.wait).collect();
        println!(
            "{}",
            json!({"phase":"overlap","worker":if i==0{"publish"}else{"poll"},"latency":quantiles(&times),"lock_wait":quantiles(&wait)})
        );
    }
    println!(
        "{}",
        json!({"phase":"admin","operation":operation,"messages_prefilled":n,"payload_bytes":size,"samples":admin,"completion_status_samples":observations,"transport":"none","parallel_workers":2,"request_generation_timed":false,"writer":"real unencrypted temporary topic files; capture is synchronous","completion_status_interval_ms":125})
    );
    drop(e);
    Ok(())
}
fn main() -> Result<(), BoxError> {
    let args: Vec<_> = std::env::args().collect();
    let op = args.get(1).ok_or("operation")?.clone();
    let n = args.get(2).map_or(Ok(100000), |s| s.parse::<usize>())?;
    let size = args.get(3).map_or(Ok(256), |s| s.parse::<usize>())?;
    if !(2000..=100000).contains(&n) || !(1..=4096).contains(&size) {
        return Err("Invalid workload".into());
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(n, size, op))
}
