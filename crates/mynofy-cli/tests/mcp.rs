#![cfg(unix)]
mod support;
use mynofy_protocol::crypto;
use mynofy_server::{config::Config, transport};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};
const MODERN: &str = "2026-07-28";
struct Fixture {
    dir: PathBuf,
    socket: PathBuf,
    key: PathBuf,
    config: Config,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "mynofy-mcp-{}",
        u64::from_le_bytes(crypto::random().unwrap())
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&key).unwrap();
    let id = crypto::initial_client_id(&public);
    let socket = dir.join("data.sock");
    let key_path = dir.join("agent.pk8");
    std::fs::write(&key_path, key).unwrap();
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut cfg: Value =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
    cfg["service"] = json!({"id":"local","unix_socket":socket});
    cfg["clients"] = json!([{"client_id":id,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":"2030-01-01T00:00:00Z"}]}]);
    cfg["topics"][0]["acl"] =
        json!([{"client_id":id,"allow":["publish","subscribe","inspect_own"]}]);
    cfg["topics"].as_array_mut().unwrap().push(json!({"name":"human/messages","owner_id":"local","policy":{"default_ttl_seconds":300,"max_ttl_seconds":3600},"acl":[{"client_id":id,"allow":["subscribe"]}]}));
    cfg["limits"]["source_auth_burst"] = 100.into();
    cfg["limits"]["client_burst"] = 1000.into();
    Fixture {
        dir,
        socket,
        key: key_path,
        config: serde_json::from_value(cfg).unwrap(),
    }
}
struct Mcp {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
}
impl Mcp {
    fn start(f: &Fixture) -> Self {
        Self::start_with_checkpoint(f, false)
    }
    fn start_with_checkpoint(f: &Fixture, checkpoint: bool) -> Self {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"));
        command.args([
            "mcp",
            "--socket",
            f.socket.to_str().unwrap(),
            "--key",
            f.key.to_str().unwrap(),
            "--owner",
            "local",
        ]);
        if checkpoint {
            command
                .arg("--checkpoint")
                .arg(f.dir.join("checkpoint.json"));
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            input,
            output,
        }
    }
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        let input = self.input.as_mut().unwrap();
        input.write_all(&bytes).await.unwrap();
        input.flush().await.unwrap();
    }
    async fn read(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.output.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("MCP response, not EOF");
        serde_json::from_str(&line).unwrap()
    }
    async fn request(&mut self, id: u32, method: &str, params: Value, modern: bool) -> Value {
        let mut params = params;
        if modern {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":MODERN,"io.modelcontextprotocol/clientCapabilities":{}});
        }
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        let value = self.read().await;
        assert_eq!(value["id"], id, "{value}");
        value
    }
    async fn tool(&mut self, id: u32, name: &str, args: Value, modern: bool) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name":name,"arguments":args}),
            modern,
        )
        .await
    }
    async fn initialize(&mut self) {
        let hello=self.request(1,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"integration","version":"1"}}),false).await;
        assert_eq!(hello["result"]["protocolVersion"], "2025-11-25");
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
    }
    async fn finish(mut self) {
        drop(self.input.take());
        let out = tokio::time::timeout(Duration::from_secs(3), self.child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
#[tokio::test]
async fn mcp_cli_exposes_existing_client_operations_and_preserves_poll_and_acl_semantics() {
    if support::under_unprivileged_identity(
        "mcp_cli_exposes_existing_client_operations_and_preserves_poll_and_acl_semantics",
    ) {
        return;
    }
    let f = fixture();
    let bound = transport::bind_unix(&f.socket).unwrap();
    let cfg = f.config.clone();
    let server = tokio::spawn(async move { support::run_unix_fixture(bound, cfg).await });
    let mut m = Mcp::start(&f);
    m.initialize().await;
    let tools = m.request(2, "tools/list", json!({}), false).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 10);
    let info = m.tool(3, "mynofy_connection_info", json!({}), false).await;
    assert_eq!(info["result"]["structuredContent"]["owner_id"], "local");
    assert_eq!(info["result"]["structuredContent"]["restart_counter"], 0);
    let published = m
        .tool(
            4,
            "mynofy_publish",
            json!({"topic":"local/events","payload":"review requested"}),
            false,
        )
        .await;
    assert_eq!(published["result"]["isError"], false);
    let mid = published["result"]["structuredContent"]["response"]["message_id"].clone();
    let page = m
        .tool(5, "mynofy_poll", json!({"topic":"local/events"}), false)
        .await;
    let page = &page["result"]["structuredContent"]["response"];

    let cursor = page["next_cursor"].clone();
    let _before = m
        .tool(6, "mynofy_get_message", json!({"message_id":mid}), false)
        .await;

    let _after = m
        .tool(8, "mynofy_get_message", json!({"message_id":mid}), false)
        .await;

    let forbidden = m
        .tool(
            9,
            "mynofy_publish",
            json!({"topic":"human/messages","payload":"pretend human"}),
            false,
        )
        .await;
    assert_eq!(forbidden["result"]["isError"], true);
    assert_eq!(
        forbidden["result"]["structuredContent"]["response"]["code"],
        "forbidden"
    );
    assert_eq!(
        m.tool(
            10,
            "mynofy_publish",
            json!({"topic":"local/events","payload":"reject this"}),
            false
        )
        .await["result"]["isError"],
        false
    );
    let _page = m
        .tool(
            11,
            "mynofy_poll",
            json!({"topic":"local/events","after":cursor}),
            false,
        )
        .await;

    let own = m
        .tool(13, "mynofy_list_messages", json!({"limit":100}), false)
        .await;
    assert_eq!(
        own["result"]["structuredContent"]["response"]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        m.tool(
            14,
            "mynofy_get_permissions",
            json!({"topic":"human/messages"}),
            false
        )
        .await["result"]["structuredContent"]["response"]["actions"],
        json!(["subscribe"])
    );
    assert_eq!(
        m.tool(15, "mynofy_get_catalog", json!({}), false).await["result"]["structuredContent"]["response"]
            ["type"],
        "catalog"
    );
    let invalid = m
        .tool(
            16,
            "mynofy_publish",
            json!({"topic":"local/events","payload":"x".repeat(4097)}),
            false,
        )
        .await;
    assert_eq!(invalid["error"]["code"], -32602);
    assert_eq!(
        m.tool(
            17,
            "mynofy_topic_create",
            json!({"topic":"backdoor"}),
            false
        )
        .await["error"]["code"],
        -32602
    );
    m.finish().await;
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn modern_mcp_is_discoverable_without_a_live_broker_and_does_not_inherit_metadata() {
    if support::under_unprivileged_identity(
        "modern_mcp_is_discoverable_without_a_live_broker_and_does_not_inherit_metadata",
    ) {
        return;
    }
    let f = fixture();
    let mut m = Mcp::start(&f);
    for version in [Value::Null, json!(17), json!(true), json!([]), json!({})] {
        let invalid = m
            .request(
                90,
                "server/discover",
                json!({"_meta":{
                    "io.modelcontextprotocol/protocolVersion": version,
                    "io.modelcontextprotocol/clientCapabilities": {}
                }}),
                false,
            )
            .await;
        assert_eq!(invalid["error"]["code"], -32602);
        assert!(invalid["error"].get("data").is_none());
    }
    let unsupported = m
        .request(
            91,
            "server/discover",
            json!({"_meta":{
                "io.modelcontextprotocol/protocolVersion": "1900-01-01",
                "io.modelcontextprotocol/clientCapabilities": {}
            }}),
            false,
        )
        .await;
    assert_eq!(unsupported["error"]["code"], -32022);
    assert_eq!(unsupported["error"]["data"]["requested"], "1900-01-01");
    let discover = m.request(1, "server/discover", json!({}), true).await;
    assert_eq!(discover["result"]["resultType"], "complete");
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!([MODERN, "2025-11-25"])
    );
    let tools = m.request(2, "tools/list", json!({}), true).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 10);
    let failed = m.tool(3, "mynofy_connection_info", json!({}), true).await;
    assert_eq!(failed["result"]["isError"], true);
    assert_eq!(
        failed["result"]["structuredContent"]["outcome_unknown"],
        false
    );
    assert_eq!(
        m.request(4, "tools/list", json!({}), false).await["error"]["code"],
        -32602
    );
    assert_eq!(
        m.request(5, "tools/list", json!({}), true).await["result"]["resultType"],
        "complete"
    );
    m.finish().await;
}
#[tokio::test]
async fn mcp_cancellation_and_eof_close_pending_owner_work_without_blocking_metadata_requests() {
    if support::under_unprivileged_identity(
        "mcp_cancellation_and_eof_close_pending_owner_work_without_blocking_metadata_requests",
    ) {
        return;
    }
    use futures_util::StreamExt;
    let f = fixture();
    let listener = tokio::net::UnixListener::bind(&f.socket).unwrap();
    std::fs::set_permissions(&f.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (started_tx, started) = tokio::sync::oneshot::channel();
    let (closed_tx, closed) = tokio::sync::oneshot::channel();
    let owner = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert!(ws.next().await.is_some());
        let _ = started_tx.send(());
        while let Some(Ok(_)) = ws.next().await {}
        let _ = closed_tx.send(());
        std::future::pending::<()>().await;
    });
    let mut m = Mcp::start(&f);
    m.initialize().await;
    m.send(json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"mynofy_connection_info","arguments":{}}})).await;
    tokio::time::timeout(Duration::from_secs(3), started)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        m.request(11, "ping", json!({}), false).await["result"],
        json!({})
    );
    assert_eq!(
        m.tool(12, "mynofy_poll", json!({"topic":"local/events"}), false)
            .await["result"]["structuredContent"]["code"],
        "busy"
    );
    m.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10}}))
        .await;
    tokio::time::timeout(Duration::from_secs(3), closed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.request(13, "ping", json!({}), false).await["id"], 13);
    m.send(json!({"jsonrpc":"2.0","id":14,"method":"tools/call","params":{"name":"mynofy_connection_info","arguments":{}}})).await;
    m.finish().await;
    owner.abort();
    let _ = owner.await;
}

#[tokio::test]
async fn lost_publish_response_is_reported_uncertain_and_never_replayed_on_reconnect() {
    if support::under_unprivileged_identity(
        "lost_publish_response_is_reported_uncertain_and_never_replayed_on_reconnect",
    ) {
        return;
    }
    use futures_util::{SinkExt, StreamExt};
    use mynofy_protocol::ProtobufMessage;
    let f = fixture();
    let listener = tokio::net::UnixListener::bind(&f.socket).unwrap();
    std::fs::set_permissions(&f.socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let engine = mynofy_server::engine::Engine::new(f.config.clone(), transport::now_ms()).unwrap();
    let engine = std::sync::Arc::new(std::sync::Mutex::new(engine));
    let state = engine.clone();
    let publications = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = publications.clone();
    let owner = tokio::spawn(async move {
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let session = state.lock().unwrap().connect(transport::now_ms()).unwrap();
            while let Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(raw))) =
                ws.next().await
            {
                let frame = mynofy_protocol::pb::Frame::decode(raw.as_ref()).unwrap();
                let publish = matches!(frame.body, Some(mynofy_protocol::pb::Body::Publish(_)));
                let reply =
                    state
                        .lock()
                        .unwrap()
                        .handle_protobuf(session, &raw, transport::now_ms());
                if publish {
                    assert!(matches!(
                        reply.body,
                        Some(mynofy_protocol::pb::Body::Result(_))
                    ));
                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    ws.close(None).await.unwrap();
                    break;
                }
                ws.send(tokio_tungstenite::tungstenite::Message::Binary(
                    reply.encode_to_vec().into(),
                ))
                .await
                .unwrap();
            }
            state.lock().unwrap().disconnect(session);
        }
    });
    let mut m = Mcp::start(&f);
    m.initialize().await;
    let result = m
        .tool(
            2,
            "mynofy_publish",
            json!({"topic":"local/events","payload":"accepted, response lost"}),
            false,
        )
        .await;
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["structuredContent"]["outcome_unknown"],
        true
    );
    assert_eq!(publications.load(std::sync::atomic::Ordering::Relaxed), 1);
    let own = m.tool(3, "mynofy_list_messages", json!({}), false).await;
    assert_eq!(own["result"]["isError"], false);
    assert_eq!(
        own["result"]["structuredContent"]["response"]["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(publications.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(engine.lock().unwrap().storage_stats().active_records, 1);
    m.finish().await;
    tokio::time::timeout(Duration::from_secs(3), owner)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn mcp_stops_on_sigterm_with_open_stdin() {
    if support::under_unprivileged_identity("mcp_stops_on_sigterm_with_open_stdin") {
        return;
    }
    let f = fixture();
    let mut mcp = Mcp::start(&f);
    mcp.send(json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
        .await;
    assert_eq!(mcp.read().await["result"], json!({}));
    assert!(
        std::process::Command::new("kill")
            .args(["-TERM", &mcp.child.id().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let status = tokio::time::timeout(Duration::from_secs(3), mcp.child.wait())
        .await
        .expect("SIGTERM must stop MCP even with open stdin")
        .unwrap();
    assert!(status.success());
    assert!(
        mcp.input.is_some(),
        "stdin was kept open throughout shutdown"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rotation_sync_failures_preserve_old_remote_credential() {
    if support::under_unprivileged_identity("rotation_sync_failures_preserve_old_remote_credential")
    {
        return;
    }
    let f = fixture();
    let source = f.dir.join("sync_failure.c");
    std::fs::write(
        &source,
        r#"
#include <errno.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>
int fsync(int fd) {
    struct stat state;
    const char *kind = getenv("MYNOFY_TEST_SYNC_FAILURE");
    if (kind && fstat(fd, &state) == 0 &&
        ((kind[0] == 'd' && S_ISDIR(state.st_mode)) ||
         (kind[0] == 'f' && S_ISREG(state.st_mode)))) { errno = EIO; return -1; }
    return syscall(SYS_fsync, fd);
}
"#,
    )
    .unwrap();
    let library = f.dir.join("sync_failure.so");
    let compiled = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let bound = transport::bind_unix(&f.socket).unwrap();
    let server = tokio::spawn(support::run_unix_fixture(bound, f.config.clone()));
    let old = std::fs::read(&f.key).unwrap();
    let client_id = crypto::initial_client_id(&crypto::public_key(&old).unwrap());
    for failure in ["file", "directory"] {
        let name = format!("new-{failure}.pk8");
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .current_dir(&f.dir)
            .args(["rotate", "--owner", "local", "--socket"])
            .arg(&f.socket)
            .arg("--key")
            .arg(&f.key)
            .args(["--new-key", &name])
            .env("LD_PRELOAD", &library)
            .env("MYNOFY_TEST_SYNC_FAILURE", failure)
            .output()
            .await
            .unwrap();
        assert!(
            !output.status.success(),
            "{failure} sync failure must prevent commit"
        );
        let path = f.dir.join(&name);
        if failure == "file" {
            assert!(
                !path.exists(),
                "failed file sync must not publish the candidate key"
            );
        }

        let mut old_session =
            mynofy_client::connect_unix_client(&f.socket, &old, "local", Some(&client_id))
                .await
                .unwrap();
        old_session.close(None).await.unwrap();
        if failure == "directory" {
            let prepared = std::fs::read(path).unwrap();
            assert!(
                mynofy_client::connect_unix_client(&f.socket, &prepared, "local", Some(&client_id))
                    .await
                    .is_err()
            );
        }
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn mcp_preserves_event_shaped_legacy_bytes() {
    if support::under_unprivileged_identity("mcp_preserves_event_shaped_legacy_bytes") {
        return;
    }
    use mynofy_protocol::{payload::Payload, pb};
    let f = fixture();
    let bound = transport::bind_unix(&f.socket).unwrap();
    let server = tokio::spawn(support::run_unix_fixture(bound, f.config.clone()));
    let opaque =
        json!({"kind":"event","catalog":999,"parameters":[{"u64":7}],"note":"opaque legacy JSON"});
    let key = std::fs::read(&f.key).unwrap();
    let mut publisher = mynofy_client::connect_unix_client(&f.socket, &key, "local", None)
        .await
        .unwrap();
    let published = mynofy_client::exchange_frame(
        &mut publisher,
        &pb::Frame {
            version: 1,
            body: Some(pb::Body::Publish(pb::Publish {
                id: "opaque".into(),
                topic: "local/events".into(),
                expires: None,
                payload: Payload::bytes(
                    serde_json::to_vec(&opaque).unwrap(),
                    Some("application/vnd.mynofy.legacy+json".into()),
                ),
            })),
        },
    )
    .await
    .unwrap();
    assert_eq!(published.kind(), "result");
    let mut m = Mcp::start(&f);
    m.initialize().await;
    let response = m
        .tool(2, "mynofy_poll", json!({"topic":"local/events"}), false)
        .await;
    assert_eq!(
        response["result"]["structuredContent"]["response"]["items"][0]["message"]["payload"],
        opaque
    );
    m.input.take();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), m.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn managed_poll_commits_workflow_state_and_resumes_across_client_restarts() {
    if support::under_unprivileged_identity(
        "managed_poll_commits_workflow_state_and_resumes_across_client_restarts",
    ) {
        return;
    }
    let f = fixture();
    let bound = transport::bind_unix(&f.socket).unwrap();
    let cfg = f.config.clone();
    let server = tokio::spawn(async move { support::run_unix_fixture(bound, cfg).await });
    let topic = "local/events";
    let mut plain = Mcp::start(&f);
    plain.initialize().await;
    let disabled = plain
        .tool(2, "mynofy_poll_next", json!({"topic":topic}), false)
        .await;
    assert_eq!(
        disabled["result"]["structuredContent"]["code"],
        "checkpoint_disabled"
    );
    plain.finish().await;
    let mut m = Mcp::start_with_checkpoint(&f, true);
    m.initialize().await;
    for id in [2, 3] {
        let sent = m
            .tool(
                id,
                "mynofy_publish",
                json!({"topic":topic,"payload":"review"}),
                false,
            )
            .await;
        assert_eq!(sent["result"]["isError"], false, "{sent}");
    }
    let first = m
        .tool(
            4,
            "mynofy_poll_next",
            json!({"topic":topic,"limit":1}),
            false,
        )
        .await;
    let page = &first["result"]["structuredContent"];
    assert_eq!(page["response"]["has_more"], true, "{first}");
    let receipt = page["checkpoint_receipt"].clone();
    let cursor = page["response"]["next_cursor"].clone();
    let deferred = m
        .tool(5, "mynofy_poll_next", json!({"topic":topic}), false)
        .await;
    let data = &deferred["result"]["structuredContent"];
    assert_eq!(data["type"], "poll_deferred", "{deferred}");
    assert!(
        data["retry_after_ms"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= 30_000)
    );
    let wrong_topic = m
        .tool(
            6,
            "mynofy_checkpoint_commit",
            json!({"topic":"human/messages","receipt":receipt}),
            false,
        )
        .await;
    assert_eq!(
        wrong_topic["result"]["structuredContent"]["code"],
        "stale_checkpoint_receipt"
    );
    assert!(!f.dir.join("checkpoint.json").exists());

    let commit = m
        .tool(
            7,
            "mynofy_checkpoint_commit",
            json!({"topic":topic,"receipt":receipt,"state":{"ticket":"TASK-001"}}),
            false,
        )
        .await;
    assert_eq!(
        commit["result"]["structuredContent"]["cursor"], cursor,
        "{commit}"
    );
    let next = m
        .tool(8, "mynofy_poll_next", json!({"topic":topic}), false)
        .await;
    let next = &next["result"]["structuredContent"];
    assert_eq!(next["response"]["items"].as_array().unwrap().len(), 1);
    assert_ne!(next["response"]["next_cursor"], cursor);
    let discarded_receipt = next["checkpoint_receipt"].clone();
    m.finish().await;

    let mut m = Mcp::start_with_checkpoint(&f, true);
    m.initialize().await;
    let loaded = m
        .tool(2, "mynofy_checkpoint_status", json!({"topic":topic}), false)
        .await;
    let saved = &loaded["result"]["structuredContent"];
    assert_eq!(saved["cursor"], cursor, "{loaded}");
    assert_eq!(saved["state"], json!({"ticket":"TASK-001"}));
    let stale = m
        .tool(
            3,
            "mynofy_checkpoint_commit",
            json!({"topic":topic,"receipt":discarded_receipt}),
            false,
        )
        .await;
    assert_eq!(
        stale["result"]["structuredContent"]["code"],
        "stale_checkpoint_receipt"
    );
    let replay = m
        .tool(4, "mynofy_poll_next", json!({"topic":topic}), false)
        .await;
    let replay = &replay["result"]["structuredContent"];
    let receipt_from_old_boot = replay["checkpoint_receipt"].clone();
    assert_eq!(replay["response"]["items"], next["response"]["items"]);
    assert_eq!(
        replay["response"]["next_cursor"],
        next["response"]["next_cursor"]
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());

    let blocked = m
        .tool(
            5,
            "mynofy_checkpoint_commit",
            json!({"topic":topic,"receipt":receipt_from_old_boot,"state":{"ticket":"wrong"}}),
            false,
        )
        .await;
    assert_eq!(
        blocked["result"]["structuredContent"]["code"],
        "transport_error"
    );
    m.finish().await;

    let mut m = Mcp::start_with_checkpoint(&f, true);
    m.initialize().await;
    let offline = m
        .tool(2, "mynofy_checkpoint_status", json!({"topic":topic}), false)
        .await;
    let offline = &offline["result"]["structuredContent"];
    assert_eq!(offline["owner_context"], "unchecked");
    assert_eq!(offline["committed_cursor"], cursor);
    assert_eq!(offline["state"], json!({"ticket":"TASK-001"}));

    let bound = transport::bind_unix(&f.socket).unwrap();
    let cfg = f.config.clone();
    let server = tokio::spawn(async move { support::run_unix_fixture(bound, cfg).await });
    let info = m.tool(3, "mynofy_connection_info", json!({}), false).await;
    let info = &info["result"]["structuredContent"];
    assert_eq!(info["response"]["owner_context"], "authenticated_now");
    let reset = m
        .tool(4, "mynofy_checkpoint_status", json!({"topic":topic}), false)
        .await;
    let reset = &reset["result"]["structuredContent"];
    assert!(reset["cursor"].is_null());
    assert_eq!(reset["committed_cursor"], cursor);
    assert_ne!(
        reset["committed_owner"]["boot_id"],
        reset["observed_owner"]["boot_id"]
    );
    assert_eq!(reset["observed_owner"]["boot_id"], info["boot_id"]);
    assert_eq!(reset["state"], json!({"ticket":"TASK-001"}));
    m.finish().await;
    server.abort();
}
