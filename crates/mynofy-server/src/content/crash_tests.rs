//! Real child processes killed at deterministic storage boundaries. No production fault switches.
use super::*;
use crate::{
    config::{Client, Policy, PublicKey, Rule, SecurityStorage},
    crypto,
    engine::Engine,
    protocol::payload::Payload,
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
const WORKER: &str = "content::crash_tests::storage_fault_worker";
const ENV: &str = "MYNOFY_STORAGE_TEST_";
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mynofy-storage-fault-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn secret(mode: ContentEncryption, new: bool) -> Option<ContentSecret> {
    match mode {
        ContentEncryption::None => None,
        ContentEncryption::KeyStdin => Some(ContentSecret::Key(Zeroizing::new(
            [if new { 9 } else { 7 }; 32],
        ))),
        ContentEncryption::PasswordStdin => Some(ContentSecret::Password(Zeroizing::new(if new {
            b"storage-test-new".to_vec()
        } else {
            b"storage-test-old".to_vec()
        }))),
    }
}
fn key() -> Vec<u8> {
    aws_lc_rs::signature::Ed25519KeyPair::from_seed_unchecked(&[1; 32])
        .unwrap()
        .to_pkcs8()
        .unwrap()
        .as_ref()
        .to_vec()
}
fn request(e: &mut Engine, session: u64, value: Value) -> Value {
    e.handle(session, &value.to_string(), crate::transport::now_ms())
}
fn authenticate(e: &mut Engine) -> u64 {
    let key = key();
    let public = crypto::public_key(&key).unwrap();
    let now = crate::transport::now_ms();
    let session = e.connect(now).unwrap();
    let challenge = request(
        e,
        session,
        json!({"version":1,"type":"auth_hello","id":"hello","token_id":crypto::token_id(&public)}),
    );
    assert_eq!(challenge["type"], "auth_challenge");
    let nonce = crypto::decode(challenge["nonce"].as_str().unwrap()).unwrap();
    let signature = crypto::sign(
        &key,
        &crypto::auth_bytes(&nonce, &public, &e.config.service.id),
    )
    .unwrap();
    let welcome = request(
        e,
        session,
        json!({"version":1,"type":"auth_proof","id":"proof","auth_id":"hello","signature":signature}),
    );
    assert_eq!(welcome["type"], "auth_welcome");
    session
}
fn publish(e: &mut Engine, session: u64, topic: &str, value: &str) {
    let mut bytes = vec![0x5a; 4096];
    bytes[..value.len()].copy_from_slice(value.as_bytes());
    let reply = request(
        e,
        session,
        json!({"version":1,"type":"publish","id":"publish","topic":topic,"payload":Payload::bytes(bytes,Some("application/test".into()))}),
    );
    assert_eq!(reply["type"], "result", "{reply}");
}
fn poll(e: &mut Engine, session: u64, topic: &str) -> Value {
    let reply = request(
        e,
        session,
        json!({"version":1,"type":"poll","id":"poll","topic":topic,"limit":10}),
    );
    assert_eq!(reply["type"], "messages", "{reply}");
    reply
}
fn wait_saved(e: &Engine) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = e.content_status();
        if !state.save_in_progress {
            assert!(
                state.last_error.is_none(),
                "{}",
                state.last_error.unwrap_or_default()
            );
            return;
        }
        assert!(Instant::now() < deadline, "save did not complete");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn save(e: &mut Engine) {
    e.save_content().unwrap();
    wait_saved(e);
}
fn config(root: &Path, mode: ContentEncryption) -> Config {
    let mut config: Config =
        serde_json::from_str(include_str!("../../../../config/scenarios/local.json")).unwrap();
    let public = crypto::public_key(&key()).unwrap();
    let id = crypto::initial_client_id(&public);
    config.clients = vec![Client {
        client_id: id.clone(),
        display_name: None,
        disabled: false,
        keys: vec![PublicKey {
            algorithm: "ed25519".into(),
            public_key: crypto::encode(&public),
            valid_until: crate::protocol::timestamp(crate::transport::now_ms() + 86_400_000)
                .unwrap(),
        }],
    }];
    let mut topic = config.topics[0].clone();
    topic.name = "a".into();
    topic.policy = Some(Policy {
        default_ttl_seconds: 3600,
        max_ttl_seconds: 3600,
    });
    topic.acl = Some(vec![Rule {
        client_id: id,
        allow: vec![
            crate::config::Action::Publish,
            crate::config::Action::Subscribe,
            crate::config::Action::InspectOwn,
        ],
    }]);
    topic.storage = TopicStorage::Manual { encrypted: false };
    let mut other = topic.clone();
    other.name = "b".into();
    other.storage = TopicStorage::Manual {
        encrypted: mode != ContentEncryption::None,
    };
    config.topics = vec![topic, other];
    config.content_storage = Some(ContentStorage {
        directory: root.join("content"),
        encryption: mode,
        max_snapshot_bytes: 4 * 1024 * 1024,
    });
    config.security_storage = Some(SecurityStorage {
        directory: root.join("security"),
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    config
}
struct Fixture {
    directory: Directory,
    config: Config,
}
impl Fixture {
    fn new(mode: ContentEncryption, operation: &str) -> Self {
        let directory = Directory::new();
        let mut config = config(&directory.0, mode);
        let mut e = Engine::new_with_content_secret(
            config.clone(),
            crate::transport::now_ms(),
            secret(mode, false),
        )
        .unwrap();
        let s = authenticate(&mut e);
        for topic in ["a", "b"] {
            publish(&mut e, s, topic, &format!("old-{topic}"));
            let _page = poll(&mut e, s, topic);
        }
        save(&mut e);
        drop(e);
        if operation == "restore" || operation == "fresh-restore" {
            offline::OfflineStorage::open(config.clone(), secret(mode, false))
                .unwrap()
                .backup(&directory.0.join("import"))
                .unwrap();
            if operation == "restore" {
                let mut e = Engine::new_with_content_secret(
                    config.clone(),
                    crate::transport::now_ms(),
                    secret(mode, false),
                )
                .unwrap();
                let s = authenticate(&mut e);
                for topic in ["a", "b"] {
                    publish(&mut e, s, topic, &format!("new-{topic}"));
                }
                save(&mut e);
                drop(e);
            }
        }
        if operation == "topic-ram" {
            config.topics[0].storage = TopicStorage::Ram {};
        }
        if operation == "fresh-restore" {
            config.content_storage.as_mut().unwrap().directory = directory.0.join("empty");
        }
        Self { directory, config }
    }
    fn fork(&self) -> Self {
        let directory = Directory::new();
        copy_tree(&self.directory.0, &directory.0);
        let mut config = self.config.clone();
        config.content_storage.as_mut().unwrap().directory = directory.0.join(
            if self
                .config
                .content_storage
                .as_ref()
                .unwrap()
                .directory
                .ends_with("empty")
            {
                "empty"
            } else {
                "content"
            },
        );
        config.security_storage.as_mut().unwrap().directory = directory.0.join("security");
        Self { directory, config }
    }
    fn content(&self) -> &Path {
        &self.config.content_storage.as_ref().unwrap().directory
    }
}
fn copy_tree(source: &Path, target: &Path) {
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        assert!(!ty.is_symlink());
        if ty.is_dir() {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&destination)
                .unwrap();
            copy_tree(&entry.path(), &destination);
        } else {
            assert!(ty.is_file());
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}
fn snapshot(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().to_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Old,
    New,
}
#[derive(Clone, Copy, Debug)]
struct Case {
    point: &'static str,
    occurrence: usize,
    outcome: Outcome,
}
fn case(point: &'static str, occurrence: usize, outcome: Outcome) -> Case {
    Case {
        point,
        occurrence,
        outcome,
    }
}
fn content_boundaries() -> Vec<Case> {
    let mut cases = Vec::new();
    for point in [
        "topic.truncated",
        "topic.partial_written",
        "topic.written",
        "topic.file_synced",
        "topic.renamed",
        "topic.dir_synced",
    ] {
        for n in [1, 2] {
            cases.push(case(point, n, Outcome::Old));
        }
    }
    for point in [
        "manifest.truncated",
        "manifest.partial_written",
        "manifest.written",
        "manifest.file_synced",
        "manifest.rename",
    ] {
        cases.push(case(point, 1, Outcome::Old));
    }
    for point in [
        "manifest.renamed",
        "manifest.dir_sync",
        "manifest.dir_synced",
        "cleanup.before_unlink",
        "cleanup.after_unlink",
    ] {
        cases.push(case(point, 1, Outcome::New));
    }
    cases
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run_child(fixture: &Fixture, operation: &str, case: Case, action: &str) {
    let mode = match fixture.config.content_storage.as_ref().unwrap().encryption {
        ContentEncryption::None => "none",
        ContentEncryption::KeyStdin => "key",
        ContentEncryption::PasswordStdin => "password",
    };
    eprintln!("storage-fault {operation} {mode} {action} {case:?}");
    let path = fixture.directory.0.join(format!(
        "case-{:016x}.json",
        u64::from_le_bytes(crypto::random().unwrap())
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&serde_json::to_vec(&fixture.config).unwrap())
        .unwrap();
    drop(file);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            WORKER,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(format!("{ENV}CONFIG"), &path)
        .env(format!("{ENV}ROOT"), &fixture.directory.0)
        .env(format!("{ENV}OP"), operation)
        .env(format!("{ENV}POINT"), case.point)
        .env(format!("{ENV}N"), case.occurrence.to_string())
        .env(format!("{ENV}ACTION"), action)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = ChildGuard(command.spawn().unwrap());
    let stderr = child.0.stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut diagnostic = String::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            if line.starts_with("MYNOFY_STORAGE_POINT ") {
                let _ = send.send(true);
            }
            if diagnostic.len() < 65536 {
                diagnostic.push_str(&line);
                diagnostic.push('\n');
            }
        }
        let _ = send.send(false);
        diagnostic
    });
    let reached = receive.recv_timeout(Duration::from_secs(30));
    if !matches!(reached, Ok(true)) {
        let _ = child.0.kill();
        let status = child.0.wait().unwrap();
        panic!(
            "{operation} {case:?} {action}: fault not reached ({status}): {}",
            reader.join().unwrap()
        );
    }
    let status = if action == "stop" {
        child.0.kill().unwrap();
        let status = child.0.wait().unwrap();
        assert_eq!(status.signal(), Some(9));
        status
    } else {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "worker failed to exit");
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    let diagnostic = reader.join().unwrap();
    if action != "stop" {
        assert!(
            status.success(),
            "{operation} {case:?} {action}: {diagnostic}"
        );
    }
}
fn verify_generation(config: &Config, operation: &str, outcome: Outcome) {
    let mode = config.content_storage.as_ref().unwrap().encryption;
    let use_new_secret = operation == "rekey" && outcome == Outcome::New;
    if operation == "rekey" {
        assert!(
            offline::OfflineStorage::open(config.clone(), secret(mode, !use_new_secret)).is_err(),
            "opposite secret must fail"
        );
    }
    let mut e = Engine::new_with_content_secret(
        config.clone(),
        crate::transport::now_ms(),
        secret(mode, use_new_secret),
    )
    .unwrap();
    let s = authenticate(&mut e);
    let want_new_messages = (operation == "save" && outcome == Outcome::New)
        || (operation == "restore" && outcome == Outcome::Old);
    for topic in ["a", "b"] {
        let page = poll(&mut e, s, topic);
        let items = page["items"].as_array().unwrap();
        if operation == "topic-ram" && outcome == Outcome::New && topic == "a" {
            assert!(items.is_empty());
            continue;
        }
        assert_eq!(
            items.len(),
            if want_new_messages { 2 } else { 1 },
            "{operation} {outcome:?} topic {topic}"
        );
        let payload = Payload::from_json(items[0]["message"]["payload"].clone()).unwrap();
        let Some(crate::protocol::payload::Body::Data(bytes)) = payload.body else {
            panic!()
        };
        assert!(bytes.starts_with(format!("old-{topic}").as_bytes()));
        assert_eq!(bytes.len(), 4096);

        let old_id = items[0]["message"]["id"].as_str().unwrap();
        let _status = request(
            &mut e,
            s,
            json!({"version":1,"type":"get_message","id":"status","message_id":old_id}),
        );

        if want_new_messages {
            let payload = Payload::from_json(items[1]["message"]["payload"].clone()).unwrap();
            let Some(crate::protocol::payload::Body::Data(bytes)) = payload.body else {
                panic!()
            };
            assert!(bytes.starts_with(format!("new-{topic}").as_bytes()));
        }
    }
    drop(e);
    let mut cfg = config.clone();
    if operation == "topic-ram" && outcome == Outcome::New {
        cfg.topics[0].storage = TopicStorage::Ram {};
    }
    let store = offline::OfflineStorage::open(cfg, secret(mode, use_new_secret)).unwrap();
    store.cleanup().unwrap();
}
fn matrix(operation: &str, mode: ContentEncryption, cases: Vec<Case>, action: &str) {
    let base = Fixture::new(mode, operation);
    for selected in cases {
        let fixture = base.fork();
        let sec = snapshot(&fixture.directory.0.join("security"));
        let source = if operation == "backup" {
            Some(snapshot(fixture.content()))
        } else {
            None
        };

        run_child(&fixture, operation, selected, action);

        if operation != "save" {
            assert_eq!(
                snapshot(&fixture.directory.0.join("security")),
                sec,
                "offline operation changed security"
            );
        }
        if operation == "backup" {
            assert_eq!(
                snapshot(fixture.content()),
                source.unwrap(),
                "backup changed source"
            );
            let mut cfg = fixture.config.clone();
            cfg.content_storage.as_mut().unwrap().directory = fixture.directory.0.join("output");
            if selected.outcome == Outcome::Old {
                assert!(
                    !cfg.content_storage
                        .as_ref()
                        .unwrap()
                        .directory
                        .join("state.bin")
                        .exists()
                );
                assert!(offline::OfflineStorage::open(cfg, secret(mode, false)).is_err());
            } else {
                verify_generation(&cfg, "backup", Outcome::New);
            }
        } else if operation == "fresh-restore" {
            if !fixture.content().join("state.bin").exists() {
                assert!(
                    offline::OfflineStorage::open(fixture.config.clone(), secret(mode, false))
                        .is_err()
                );
            }
            let mut store = offline::OfflineStorage::open_for_restore(
                fixture.config.clone(),
                secret(mode, false),
            )
            .unwrap();
            store.restore(&fixture.directory.0.join("import")).unwrap();
            drop(store);
            verify_generation(&fixture.config, "restore", Outcome::New);
        } else {
            verify_generation(&fixture.config, operation, selected.outcome);
        }
    }
}
#[test]
fn storage_fault_process_kills_preserve_complete_save_restore_and_rekey_generations() {
    for operation in ["save", "restore", "rekey"] {
        matrix(
            operation,
            ContentEncryption::KeyStdin,
            content_boundaries(),
            "stop",
        );
    }
}
#[test]
fn storage_fault_manifest_and_topic_io_failures_keep_old_or_committed_generations() {
    let cases = vec![
        case("topic.open", 1, Outcome::Old),
        case("topic.partial_written", 2, Outcome::Old),
        case("topic.file_sync", 1, Outcome::Old),
        case("topic.rename", 2, Outcome::Old),
        case("manifest.partial_written", 1, Outcome::Old),
        case("manifest.file_sync", 1, Outcome::Old),
        case("manifest.rename", 1, Outcome::Old),
        case("manifest.dir_sync", 1, Outcome::New),
    ];
    for action in ["no-space", "io"] {
        matrix("save", ContentEncryption::KeyStdin, cases.clone(), action);
    }
    for operation in ["restore", "rekey", "backup"] {
        for action in ["no-space", "io"] {
            matrix(
                operation,
                ContentEncryption::KeyStdin,
                cases.clone(),
                action,
            );
        }
    }
}
#[test]
fn storage_fault_backup_and_ram_deletion_are_interruptible_without_security_rollback() {
    let backup = content_boundaries()
        .into_iter()
        .filter(|case| !case.point.starts_with("cleanup."))
        .collect();
    matrix("backup", ContentEncryption::KeyStdin, backup, "stop");
    let ram = content_boundaries()
        .into_iter()
        .filter(|case| !case.point.starts_with("topic."))
        .collect();
    matrix("topic-ram", ContentEncryption::KeyStdin, ram, "stop");
    matrix(
        "save",
        ContentEncryption::None,
        vec![
            case("topic.partial_written", 2, Outcome::Old),
            case("manifest.renamed", 1, Outcome::New),
        ],
        "stop",
    );
    matrix(
        "rekey",
        ContentEncryption::PasswordStdin,
        vec![
            case("topic.partial_written", 2, Outcome::Old),
            case("manifest.renamed", 1, Outcome::New),
        ],
        "stop",
    );
}
#[test]
fn storage_fault_empty_restore_resumes_after_allocator_initialization_interruptions() {
    let cases = vec![case("manifest.renamed", 1, Outcome::New)];
    matrix("fresh-restore", ContentEncryption::KeyStdin, cases, "stop");
}
#[test]
#[ignore = "subprocess worker; invoked only by the storage_fault matrix"]
fn storage_fault_worker() {
    let Ok(path) = std::env::var(format!("{ENV}CONFIG")) else {
        return;
    };
    let cfg: Config = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let mode = cfg.content_storage.as_ref().unwrap().encryption;
    let root = PathBuf::from(std::env::var(format!("{ENV}ROOT")).unwrap());
    let op = std::env::var(format!("{ENV}OP")).unwrap();
    let point = std::env::var(format!("{ENV}POINT")).unwrap();
    let n = std::env::var(format!("{ENV}N")).unwrap().parse().unwrap();
    let action = match std::env::var(format!("{ENV}ACTION")).unwrap().as_str() {
        "stop" => faults::Action::Stop,
        "no-space" => faults::Action::NoSpace,
        "io" => faults::Action::Io,
        _ => panic!(),
    };
    let result = if op == "reopen" {
        faults::arm(point, n, action);
        offline::OfflineStorage::open(cfg, secret(mode, true)).map(|_| ())
    } else if op == "save" {
        let mut cfg = cfg;
        let mut ram = cfg.topics[0].clone();
        ram.name = "ram-transition".into();
        ram.storage = TopicStorage::Ram {};
        cfg.topics.push(ram);
        let mut e =
            Engine::new_with_content_secret(cfg, crate::transport::now_ms(), secret(mode, false))
                .unwrap();
        let s = authenticate(&mut e);
        for topic in ["a", "b"] {
            publish(&mut e, s, topic, &format!("new-{topic}"));
        }
        let previous = e.content_status().saved_revision;
        faults::arm(point, n, action);
        e.save_content().unwrap();
        if !matches!(action, faults::Action::Stop) {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let status = e.content_status();
                if !status.save_in_progress {
                    let code = if matches!(action, faults::Action::NoSpace) {
                        rustix::io::Errno::NOSPC.raw_os_error()
                    } else {
                        rustix::io::Errno::IO.raw_os_error()
                    };
                    assert!(
                        status
                            .last_error
                            .unwrap()
                            .contains(&format!("os error {code}"))
                    );
                    assert!(status.dirty);
                    assert_eq!(status.saved_revision, previous);
                    if status.requires_restart {
                        let before_config = serde_json::to_value(&e.config).unwrap();
                        let before_status = serde_json::to_value(e.content_status()).unwrap();
                        for storage in [
                            TopicStorage::Manual { encrypted: false },
                            TopicStorage::Interval {
                                seconds: 1,
                                encrypted: false,
                            },
                        ] {
                            assert_eq!(
                                e.enable_topic_storage("ram-transition", storage)
                                    .unwrap_err()
                                    .code,
                                "content_storage_requires_restart"
                            );
                        }
                        assert_eq!(serde_json::to_value(&e.config).unwrap(), before_config);
                        assert_eq!(
                            serde_json::to_value(e.content_status()).unwrap(),
                            before_status
                        );
                        assert_eq!(
                            e.save_content().unwrap_err().code,
                            "content_storage_requires_restart"
                        );
                    }
                    assert!(faults::fired());
                    return;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        wait_saved(&e);
        Ok(())
    } else {
        let mut store = if op == "fresh-restore" {
            offline::OfflineStorage::open_for_restore(cfg, secret(mode, false)).unwrap()
        } else {
            offline::OfflineStorage::open(cfg, secret(mode, false)).unwrap()
        };
        faults::arm(point, n, action);
        let result = match op.as_str() {
            "restore" | "fresh-restore" => store.restore(&root.join("import")),
            "rekey" => store.rekey(secret(mode, true).unwrap()),
            "backup" => store.backup(&root.join("output")),
            "topic-ram" => store.topic_to_ram("a"),
            "cleanup" => store.cleanup(),
            _ => panic!(),
        };
        if result.is_err() && store.needs_reopen() {
            assert!(
                store.cleanup().is_err(),
                "uncertain store must reject cleanup"
            );
            assert!(
                store.backup(&root.join("unsafe-backup")).is_err(),
                "uncertain store must reject backup"
            );
        }
        result
    };
    assert!(
        faults::fired(),
        "operation ended without reaching checkpoint"
    );
    if matches!(action, faults::Action::Stop) {
        panic!("stop checkpoint returned");
    }
    let error = result.expect_err("injected I/O error must fail the operation");
    let expected = if matches!(action, faults::Action::NoSpace) {
        rustix::io::Errno::NOSPC.raw_os_error()
    } else {
        rustix::io::Errno::IO.raw_os_error()
    };
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error),
        Some(expected)
    );
}

#[test]
fn storage_fault_cleanup_is_retryable_and_preserves_committed_files_and_external_targets() {
    let base = Fixture::new(ContentEncryption::KeyStdin, "cleanup");
    for (point, occurrence) in [
        ("cleanup.before_unlink", 1),
        ("cleanup.after_unlink", 1),
        ("cleanup.before_unlink", 2),
        ("cleanup.dir_sync", 1),
    ] {
        let fixture = base.fork();
        let outside = fixture.directory.0.join("user-file");
        fs::write(&outside, b"keep outside target").unwrap();
        fs::write(fixture.content().join("user-file"), b"keep unknown file").unwrap();
        let orphan = topics::filename("unreferenced/topic", 0);
        fs::write(fixture.content().join(&orphan), b"orphan").unwrap();
        let link = topics::filename("unreferenced/link", 1);
        std::os::unix::fs::symlink(&outside, fixture.content().join(&link)).unwrap();
        for name in ["topic.tmp", "state.tmp", "references.tmp"] {
            fs::write(fixture.content().join(name), b"unfinished").unwrap();
        }
        let security = snapshot(&fixture.directory.0.join("security"));
        run_child(
            &fixture,
            "cleanup",
            case(point, occurrence, Outcome::Old),
            "stop",
        );
        assert_eq!(snapshot(&fixture.directory.0.join("security")), security);
        let store = offline::OfflineStorage::open(
            fixture.config.clone(),
            secret(ContentEncryption::KeyStdin, false),
        )
        .unwrap();
        let before = store.inspect();
        assert_eq!(before["topic_details"].as_array().unwrap().len(), 2);
        store.cleanup().unwrap();
        store.cleanup().unwrap();
        assert_eq!(store.inspect(), before);
        drop(store);
        for name in [
            "topic.tmp",
            "state.tmp",
            "references.tmp",
            orphan.as_str(),
            link.as_str(),
        ] {
            assert!(!fixture.content().join(name).exists());
        }
        assert_eq!(fs::read(&outside).unwrap(), b"keep outside target");
        assert_eq!(
            fs::read(fixture.content().join("user-file")).unwrap(),
            b"keep unknown file"
        );
        assert_eq!(snapshot(&fixture.directory.0.join("security")), security);
        verify_generation(&fixture.config, "cleanup", Outcome::Old);
    }
}

#[test]
fn storage_fault_reopen_must_confirm_visible_manifest_before_reusing_slots() {
    let fixture = Fixture::new(ContentEncryption::KeyStdin, "rekey");
    let security = snapshot(&fixture.directory.0.join("security"));
    run_child(
        &fixture,
        "rekey",
        case("manifest.dir_sync", 1, Outcome::New),
        "no-space",
    );
    let after_commit = snapshot(fixture.content());
    for action in ["no-space", "io"] {
        run_child(
            &fixture,
            "reopen",
            case("open.confirm_dir_sync", 1, Outcome::New),
            action,
        );
        assert_eq!(
            snapshot(fixture.content()),
            after_commit,
            "failed reopen changed or removed a generation"
        );
        assert_eq!(snapshot(&fixture.directory.0.join("security")), security);
    }
    verify_generation(&fixture.config, "rekey", Outcome::New);
}
