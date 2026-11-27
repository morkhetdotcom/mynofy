#![cfg(unix)]
//! Local transport tests use Python only to re-execute CLI scenarios under a non-root test identity.
mod support;
use mynofy_client as client;
use mynofy_protocol::crypto;
use mynofy_server::{config::Config, transport};
use serde_json::json;
use std::{
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};
struct Fixture {
    dir: PathBuf,
    config: Config,
    key: Vec<u8>,
    socket: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn fixture() -> Fixture {
    let suffix = u64::from_be_bytes(crypto::random().unwrap());
    let dir = std::env::temp_dir().join(format!("mn-{suffix:016x}"));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let socket = dir.join("service.sock");
    let key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&key).unwrap();
    let client = crypto::initial_client_id(&public);
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
    let service = config["service"].as_object_mut().unwrap();
    service.remove("listen");
    service.remove("tls");
    service.insert("unix_socket".into(), socket.to_str().unwrap().into());
    config["clients"] = json!([{"client_id":client,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":"2030-01-01T00:00:00Z"}]}]);
    config["topics"][0]["acl"] =
        json!([{"client_id":client,"allow":["publish","subscribe","inspect_own"]}]);
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    Fixture {
        dir,
        config: serde_json::from_value(config).unwrap(),
        key,
        socket,
    }
}
#[tokio::test]
async fn unix_publish_poll_rotate_and_cleanup() {
    let mut f = fixture();
    f.config.security_storage = Some(mynofy_server::config::SecurityStorage {
        minimum_sequence: None,
        sync: true,
        directory: f.dir.join("security"),
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    f.config.validate().unwrap();
    let bound = transport::bind_unix(&f.socket).unwrap();
    let meta = std::fs::symlink_metadata(&f.socket).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.mode() & 0o777, 0o600);
    let config = f.config.clone();
    let server = tokio::spawn(async move { support::run_unix_fixture(bound, config).await });
    let mut client = client::connect_unix_client(&f.socket, &f.key, "local", None)
        .await
        .unwrap();
    let catalog = client::get_catalog(&mut client, "mynofy", 1).await.unwrap();
    assert_eq!(catalog["name"], "mynofy");
    assert_eq!(catalog["statuses"].as_array().unwrap().len(), 21);
    let published=client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"pub","topic":"local/events","payload":{"local":true}})).await.unwrap();
    assert_eq!(published["type"], "result");
    let page = client::exchange(
        &mut client,
        &json!({"version":1,"type":"poll","id":"poll","topic":"local/events"}),
    )
    .await
    .unwrap();
    assert_eq!(page["items"][0]["message"]["payload"]["local"], true);

    let _status = client::exchange(
        &mut client,
        &json!({"version":1,"type":"get_message","id":"get","message_id":published["message_id"]}),
    )
    .await
    .unwrap();

    let new = crypto::generate_key().unwrap();
    let id = crypto::initial_client_id(&crypto::public_key(&f.key).unwrap());
    let complete = client::rotate_client(&mut client, &f.key, &new, "local", &id)
        .await
        .unwrap();
    assert_eq!(complete["client_id"], id);
    client.close(None).await.unwrap();
    let mut client = client::connect_unix_client(&f.socket, &new, "local", Some(&id))
        .await
        .unwrap();
    let empty=client::exchange(&mut client,&json!({"version":1,"type":"poll","id":"next","topic":"local/events","after":page["next_cursor"]})).await.unwrap();
    assert_eq!(empty["items"], json!([]));
    client.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
    assert!(
        !f.socket.exists(),
        "Stopping the listener must remove its own socket"
    );
}
#[tokio::test]
async fn unix_rejects_unknown_key_and_wrong_owner() {
    let f = fixture();
    let bound = transport::bind_unix(&f.socket).unwrap();
    let config = f.config.clone();
    let server = tokio::spawn(async move { support::run_unix_fixture(bound, config).await });
    assert!(
        client::connect_unix_client(&f.socket, &crypto::generate_key().unwrap(), "local", None)
            .await
            .is_err()
    );
    assert!(
        client::connect_unix_client(&f.socket, &f.key, "wrong-owner", None)
            .await
            .is_err()
    );
    server.abort();
    let _ = server.await;
    assert!(!f.socket.exists());
}
#[tokio::test]
async fn binding_never_overwrites_an_existing_file_symlink_or_socket() {
    let f = fixture();
    std::fs::write(&f.socket, b"keep").unwrap();
    assert!(transport::bind_unix(&f.socket).is_err());
    assert_eq!(std::fs::read(&f.socket).unwrap(), b"keep");
    std::fs::remove_file(&f.socket).unwrap();
    let target = f.dir.join("target");
    std::fs::write(&target, b"target").unwrap();
    std::os::unix::fs::symlink(&target, &f.socket).unwrap();
    assert!(transport::bind_unix(&f.socket).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"target");
    std::fs::remove_file(&f.socket).unwrap();
    let bound = transport::bind_unix(&f.socket).unwrap();
    assert!(transport::bind_unix(&f.socket).is_err());
    assert!(f.socket.exists());
    drop(bound);
    assert!(!f.socket.exists());
}
#[tokio::test]
async fn cleanup_keeps_a_replacement_file_or_socket() {
    let f = fixture();
    let first = transport::bind_unix(&f.socket).unwrap();
    std::fs::remove_file(&f.socket).unwrap();
    std::fs::write(&f.socket, b"replacement").unwrap();
    drop(first);
    assert_eq!(std::fs::read(&f.socket).unwrap(), b"replacement");
    std::fs::remove_file(&f.socket).unwrap();
    let first = transport::bind_unix(&f.socket).unwrap();
    std::fs::remove_file(&f.socket).unwrap();
    let replacement = transport::bind_unix(&f.socket).unwrap();
    drop(first);
    assert!(f.socket.exists());
    drop(replacement);
    assert!(!f.socket.exists());
}
#[tokio::test]
async fn rejects_writable_parent_and_overly_permissive_socket() {
    let f = fixture();
    std::fs::set_permissions(&f.dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(transport::bind_unix(&f.socket).is_err());
    assert!(!f.socket.exists());
    std::fs::set_permissions(&f.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bound = transport::bind_unix(&f.socket).unwrap();
    std::fs::set_permissions(&f.socket, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        client::connect_unix_client(&f.socket, &f.key, "local", None)
            .await
            .is_err()
    );
    drop(bound);
}
#[tokio::test]
async fn stale_socket_is_not_automatically_removed() {
    let f = fixture();
    let stale = std::os::unix::net::UnixListener::bind(&f.socket).unwrap();
    drop(stale);
    assert!(transport::bind_unix(&f.socket).is_err());
    assert!(
        std::fs::symlink_metadata(&f.socket)
            .unwrap()
            .file_type()
            .is_socket()
    );
}
#[test]
fn validates_exclusive_transport_and_resolves_relative_socket_path() {
    let f = fixture();
    let mut cfg = f.config.clone();
    cfg.service.listen = Some("127.0.0.1:0".parse().unwrap());
    assert!(cfg.validate().is_err());
    let mut cfg = f.config.clone();
    cfg.service.unix_socket = None;
    assert!(cfg.validate().is_err());
    let mut raw: serde_json::Value =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
    let service = raw["service"].as_object_mut().unwrap();
    service.remove("listen");
    service.remove("tls");
    service.insert("unix_socket".into(), "run/local.sock".into());
    let config = f.dir.join("config.json");
    std::fs::write(&config, raw.to_string()).unwrap();
    let loaded = Config::load(&config).unwrap();
    assert_eq!(
        loaded.service.unix_socket,
        Some(f.dir.canonicalize().unwrap().join("run/local.sock"))
    );
    assert!(loaded.service.tls.is_none());
}

struct ServerChild(std::process::Child);
impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn cli_uses_unix_socket_and_sigterm_removes_socket() {
    if support::under_unprivileged_identity("cli_uses_unix_socket_and_sigterm_removes_socket") {
        return;
    }
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let f = fixture();
    let binary = env!("CARGO_BIN_EXE_mynofy");
    let mut config = f.config.clone();
    config.security_storage = Some(mynofy_server::config::SecurityStorage {
        minimum_sequence: None,
        directory: f.dir.join("security"),
        sync: true,
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    std::fs::write(
        f.dir.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    mynofy_server::initialize_security(&config).unwrap();
    let mut server = ServerChild(
        Command::new(binary)
            .args(["serve", "--config"])
            .arg(f.dir.join("config.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !support::unix_socket_ready(&f.socket) {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "Server exited before socket was bound"
        );
        assert!(Instant::now() < deadline, "Socket startup timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    let key = f.dir.join("client.pk8");
    std::fs::write(&key, &f.key).unwrap();
    let mut client = Command::new(binary)
        .args(["client", "--socket"])
        .arg(&f.socket)
        .arg("--key")
        .arg(&key)
        .args(["--owner", "local"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = client.stdin.take().unwrap();
    writeln!(input,"{}",json!({"version":1,"type":"publish","id":"cli-pub","topic":"local/events","payload":"unix-cli"})).unwrap();
    writeln!(
        input,
        "{}",
        json!({"version":1,"type":"poll","id":"cli-poll","topic":"local/events"})
    )
    .unwrap();
    drop(input);
    let output = client.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frames: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["type"], "result");
    assert_eq!(frames[1]["items"][0]["message"]["payload"], "unix-cli");
    assert!(
        Command::new("kill")
            .args(["-TERM", &server.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(server.0.wait().unwrap().success());
    assert!(
        !f.socket.exists(),
        "SIGTERM should remove the original socket"
    );
}

#[tokio::test]
async fn cli_adapter_executes_only_exact_trigger_and_announces_interpretation() {
    if support::under_unprivileged_identity(
        "cli_adapter_executes_only_exact_trigger_and_announces_interpretation",
    ) {
        return;
    }
    let mut f = fixture();
    #[allow(unused_mut)]
    let mut rule: client::adapter::AdapterConfig =
        serde_json::from_str(include_str!("../../../config/adapters/echo.json")).unwrap();
    #[cfg(target_os = "linux")]
    let child_status = f.dir.join("child-status.json");
    #[cfg(target_os = "linux")]
    {
        rule.program = "python3".into();
        rule.args = vec![
            "-c".into(),
            r#"import os,sys,json
status=open('/proc/self/status').read()
regained=[]
for setter in [os.setuid,os.setgid]:
 try:
  setter(0); regained.append(True)
 except PermissionError:
  regained.append(False)
with open(sys.argv[1],'w') as f: json.dump({'status':status,'regained':regained},f)
sys.stdout.buffer.write(sys.stdin.buffer.read())
"#
            .into(),
            child_status.to_str().unwrap().into(),
        ];
    }
    let template = f.config.topics[0].clone();
    f.config.topics = [
        &rule.communication_topic,
        &rule.status_topic,
        &rule.interpretation_topic,
    ]
    .into_iter()
    .map(|name| {
        let mut t = template.clone();
        t.name = name.clone();
        t
    })
    .collect();
    let bound = transport::bind_unix(&f.socket).unwrap();
    let config = f.config.clone();
    let server = tokio::spawn(async move { support::run_unix_fixture(bound, config).await });
    let key = f.dir.join("adapter.pk8");
    std::fs::write(&key, &f.key).unwrap();
    let rules = f.dir.join("rules.json");
    std::fs::write(&rules, serde_json::to_vec(&rule).unwrap()).unwrap();
    let mut adapter = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .arg("adapter")
        .arg("--socket")
        .arg(&f.socket)
        .arg("--key")
        .arg(&key)
        .args(["--owner", "local", "--rules"])
        .arg(&rules)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut observer = client::connect_unix_client(&f.socket, &f.key, "local", None)
        .await
        .unwrap();
    let mut ignored = rule.trigger.clone();
    ignored.parameters[0].value = Some(mynofy_protocol::payload::ParameterValue::U64(4712));
    for e in [ignored, rule.trigger.clone()] {
        let p=client::exchange(&mut observer,&json!({"version":1,"type":"publish","id":"trigger","topic":rule.trigger_topic,"payload":mynofy_protocol::payload::Payload::event(e)})).await.unwrap();
        assert_eq!(p["type"], "result");
    }
    let mut cursor: Option<String> = None;
    let mut starts = 0;
    let mut successes = 0;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while successes == 0 {
            let mut query =
                json!({"version":1,"type":"poll","id":"status","topic":rule.status_topic});
            if let Some(ref c) = cursor {
                query["after"] = serde_json::Value::String(c.clone());
            }
            let page = client::exchange(&mut observer, &query).await.unwrap();
            cursor = Some(page["next_cursor"].as_str().unwrap().to_owned());
            for item in page["items"].as_array().unwrap() {
                let payload = &item["message"]["payload"];
                if payload["kind"] == "event" && payload["action"] == 10 && payload["status"] == 2 {
                    starts += 1;
                }
                if payload["kind"] == "event" && payload["action"] == 40 && payload["status"] == 2 {
                    successes += 1;
                }
            }
            if successes == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(starts, 1);
    assert_eq!(successes, 1);
    #[cfg(target_os = "linux")]
    {
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(child_status).unwrap()).unwrap();
        assert_eq!(report["regained"], json!([false, false]));
        let status = report["status"].as_str().unwrap();
        for key in ["CapEff:", "CapPrm:", "CapInh:", "CapAmb:"] {
            let value = status
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .unwrap()
                .trim();
            assert_eq!(u64::from_str_radix(value, 16).unwrap(), 0);
        }
        assert!(status.lines().any(|line| {
            line.strip_prefix("NoNewPrivs:")
                .is_some_and(|value| value.trim() == "1")
        }));
    }
    let page = client::exchange(
        &mut observer,
        &json!({"version":1,"type":"poll","id":"interpretation","topic":rule.interpretation_topic}),
    )
    .await
    .unwrap();
    let payload = mynofy_protocol::payload::Payload::from_json(
        page["items"][0]["message"]["payload"].clone(),
    )
    .unwrap();
    let Some(mynofy_protocol::payload::Body::Data(raw)) = payload.body else {
        panic!("Expected interpretation bytes")
    };
    let metadata = mynofy_protocol::strict_json(std::str::from_utf8(&raw).unwrap()).unwrap();
    assert_eq!(metadata["revision"], 1);
    assert_eq!(metadata["trigger"]["action"], 40);
    assert!(metadata.get("program").is_none());
    adapter.kill().await.unwrap();
    adapter.wait().await.unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn embedded_listener_requires_explicit_volatile_mode_or_an_initialized_store() {
    let mut f = fixture();
    assert!(transport::prepare(f.config.clone(), None).is_err());
    assert!(transport::prepare_volatile(f.config.clone()).is_ok());
    let path = f.dir.join("embedded-security");
    f.config.security_storage = Some(mynofy_server::config::SecurityStorage {
        minimum_sequence: None,
        directory: path.clone(),
        sync: true,
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    assert!(transport::prepare(f.config.clone(), None).is_err());
    assert!(!path.exists());
    assert!(transport::prepare_volatile(f.config.clone()).is_err());
    mynofy_server::initialize_security(&f.config).unwrap();
    assert!(transport::prepare(f.config.clone(), None).is_ok());
}
