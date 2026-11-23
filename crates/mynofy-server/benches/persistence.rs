//! Serial persistence timings; input preparation is outside measured operations.
#![forbid(unsafe_code)]
mod support;
use mynofy_server::{
    BoxError,
    admin::{Command, ResultBody},
    config::{
        Action, Client, Config, ContentEncryption, ContentStorage, PublicKey, Rule,
        SecurityStorage, TopicStorage,
    },
    content::{ContentSecret, offline::OfflineStorage},
    crypto,
    engine::Engine,
    protocol::{self, payload::Payload, pb},
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
use support::*;
use zeroize::Zeroizing;

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn secret(mode: &str, replacement: bool) -> Option<ContentSecret> {
    match mode {
        "none" => None,
        "key" => Some(ContentSecret::Key(Zeroizing::new(
            [if replacement { 9 } else { 7 }; 32],
        ))),
        "password" => Some(ContentSecret::Password(Zeroizing::new(if replacement {
            b"benchmark-new".to_vec()
        } else {
            b"benchmark-old".to_vec()
        }))),
        _ => unreachable!(),
    }
}
fn data(engine: &mut Engine, command: Command) -> serde_json::Value {
    let ResultBody::Data(value) = engine.admin(command, NOW).unwrap() else {
        panic!("Expected data")
    };
    value
}
fn timed<T>(phase: &str, operation: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = operation();
    let elapsed = start.elapsed();
    println!(
        "{}",
        json!({"phase":phase,"elapsed_ms":elapsed.as_secs_f64()*1000.0})
    );
    result
}
async fn run(messages: usize, size: usize, mode: &str) -> Result<(), BoxError> {
    let directory = Directory(std::env::temp_dir().join(format!(
        "mynofy-persistence-{}-{}",
        std::process::id(),
        crypto::encode(&crypto::random::<12>()?).replace(['/', '+', '='], "")
    )));
    fs::create_dir(&directory.0)?;
    let key = crypto::generate_key()?;
    let public = crypto::public_key(&key)?;
    let client_id = crypto::initial_client_id(&public);
    let mut config: Config =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json"))?;
    config.clients = vec![Client {
        disabled: false,
        client_id: client_id.clone(),
        display_name: None,
        keys: vec![PublicKey {
            algorithm: "ed25519".into(),
            public_key: crypto::encode(&public),
            valid_until: protocol::timestamp(NOW + 86_400_000)?,
        }],
    }];
    config.topics.truncate(1);
    config.topics[0].acl = Some(vec![Rule {
        client_id,
        allow: vec![Action::Publish, Action::Subscribe, Action::InspectOwn],
    }]);
    config.topics[0].storage = TopicStorage::Manual {
        encrypted: mode != "none",
    };
    config.security_storage = Some(SecurityStorage {
        directory: directory.0.join("security"),
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    config.content_storage = Some(ContentStorage {
        directory: directory.0.join("content"),
        encryption: match mode {
            "none" => ContentEncryption::None,
            "key" => ContentEncryption::KeyStdin,
            _ => ContentEncryption::PasswordStdin,
        },
        max_snapshot_bytes: 256 * 1024 * 1024,
    });
    config.limits.client_burst = 1_000_000;
    config.limits.client_requests_per_second = 1_000_000;
    config.limits.max_active_messages = messages;
    config.limits.max_active_message_bytes = messages * (size + 4096);
    let mut engine = Engine::new_with_content_secret(config.clone(), NOW, secret(mode, false))?;
    engine.start_id_allocator();
    let session = authenticate(&mut engine, &key);
    let raw = encode_request(
        frame(pb::Body::Publish(pb::Publish {
            id: "p".into(),
            topic: "local/events".into(),
            payload: Payload::bytes(vec![b'x'; size], None),
            expires: None,
        })),
        "protobuf",
    );
    let mut samples = Vec::with_capacity(messages);
    for i in 0..messages {
        request(&mut engine, session, &raw, &mut samples, "protobuf", None).check_result();
        if i % 256 == 255 {
            tokio::task::yield_now().await;
        }
    }
    assert_eq!(engine.storage_stats().active_records, messages);
    println!(
        "{}",
        json!({"phase":"configuration","messages":messages,"payload_bytes":size,"mode":mode,"rss_active_bytes":rss_bytes(),"storage":engine.storage_stats()})
    );
    // The synchronous capture includes the public admin command. Writer I/O overlaps it.
    let start = Instant::now();
    let queued = data(&mut engine, Command::ContentSave { topics: None });
    let capture_ms = start.elapsed().as_secs_f64() * 1000.0;
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        let status = data(&mut engine, Command::Status {});
        let content = &status["content_storage"];
        assert!(content["last_error"].is_null(), "{status}");
        if content["save_in_progress"] == false {
            break status;
        }
        assert!(Instant::now() < deadline, "Save timeout");
        std::thread::sleep(Duration::from_micros(100));
    };
    let save_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(
        status["content_storage"]["saved_revision"],
        queued["revision"]
    );
    assert_eq!(status["content_storage"]["dirty"], false);
    println!(
        "{}",
        json!({"phase":"save","capture_ms":capture_ms,"durable_observed_ms":save_ms,"rss_after_save_bytes":rss_bytes()})
    );
    drop(engine);
    let mut engine = timed("restart", || {
        Engine::new_with_content_secret(config.clone(), NOW, secret(mode, false))
    })?;
    assert_eq!(engine.storage_stats().active_records, messages);
    assert_eq!(
        data(&mut engine, Command::Status {})["content_storage"]["loaded"],
        true
    );
    drop(engine);
    let mut storage = timed("offline_open", || {
        OfflineStorage::open(config.clone(), secret(mode, false))
    })?;
    assert_eq!(storage.check()["active_messages"], messages);
    let inspection = timed("inspect", || storage.inspect());
    println!(
        "{}",
        json!({"phase":"stored_layout","inspection":inspection})
    );
    timed("backup", || storage.backup(&directory.0.join("backup")))?;
    timed("restore", || storage.restore(&directory.0.join("backup")))?;
    if mode != "none" {
        timed("rekey", || storage.rekey(secret(mode, true).unwrap()))?;
    }
    timed("cleanup", || storage.cleanup())?;
    drop(storage);
    let engine = Engine::new_with_content_secret(config, NOW, secret(mode, mode != "none"))?;
    assert_eq!(engine.storage_stats().active_records, messages);
    drop(engine);
    Ok(())
}
fn main() -> Result<(), BoxError> {
    let args: Vec<_> = std::env::args().collect();
    let messages = args.get(1).map_or(Ok(10000), |v| v.parse::<usize>())?;
    let size = args.get(2).map_or(Ok(256), |v| v.parse::<usize>())?;
    let mode = args.get(3).map_or("none", String::as_str);
    assert!(
        (1..=100000).contains(&messages)
            && (1..=4096).contains(&size)
            && ["none", "key", "password"].contains(&mode)
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(messages, size, mode))
}
