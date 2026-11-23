#![cfg(unix)]
//! Deterministic misbehaving clients against the real transport, not AI behavior.
mod support;
use futures_util::{SinkExt, StreamExt};
use mynofy_client as client;
use mynofy_protocol::{ProtobufMessage, crypto};
use mynofy_server::{admin, config::Config, transport};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use tokio::{net::UnixStream, time::timeout};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
type Socket = WebSocketStream<UnixStream>;
const TOPIC: &str = "local/events";

struct Service {
    dir: PathBuf,
    socket: PathBuf,
    admin: PathBuf,
    keys: [Vec<u8>; 2],
    ids: [String; 2],
    task: tokio::task::JoinHandle<Result<(), mynofy_server::BoxError>>,
}
impl Drop for Service {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Service {
    async fn start(short_timeouts: bool) -> Self {
        Self::start_with_capacity(short_timeouts, None).await
    }
    async fn start_with_capacity(short_timeouts: bool, capacity: Option<usize>) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mn-fail-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let socket = dir.join("d.sock");
        let admin = dir.join("a.sock");
        let keys = [
            crypto::generate_key().unwrap(),
            crypto::generate_key().unwrap(),
        ];
        let public = keys.each_ref().map(|key| crypto::public_key(key).unwrap());
        let ids = public.each_ref().map(|key| crypto::initial_client_id(key));
        let mut cfg: Value =
            serde_json::from_str(include_str!("../../../config/scenarios/unix.json")).unwrap();
        cfg["service"] = json!({"id":"local","unix_socket":socket,"admin_socket":admin});
        cfg["clients"] = json!(
            public
                .iter()
                .zip(&ids)
                .map(|(key, id)| json!({
            "client_id":id,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(key),
            "valid_until":"2035-01-01T00:00:00Z"}]}))
                .collect::<Vec<_>>()
        );
        cfg["topics"][0]["acl"] = json!(
            ids.iter()
                .map(|id| json!({
            "client_id":id,"allow":["publish","subscribe","inspect_own"]}))
                .collect::<Vec<_>>()
        );
        if let Some(capacity) = capacity {
            cfg["limits"]["max_parallel_requests_per_client"] = capacity.into();
            cfg["limits"]["max_response_bytes"] = (4 * 1024 * 1024).into();
            cfg["limits"]["client_burst"] = 1_000_000.into();
            cfg["limits"]["client_requests_per_second"] = 1_000_000.into();
        }
        if short_timeouts {
            for key in [
                "auth_timeout_seconds",
                "request_timeout_seconds",
                "idle_timeout_seconds",
            ] {
                cfg["limits"][key] = 1.into();
            }
        }
        let config: Config = serde_json::from_value(cfg).unwrap();
        config.validate().unwrap();
        let bound = transport::bind_unix(&socket).unwrap();
        let task = tokio::spawn(async move { transport::run_unix(bound, config).await });
        timeout(Duration::from_secs(3), async {
            while !admin.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("admin startup deadline");
        Self {
            dir,
            socket,
            admin,
            keys,
            ids,
            task,
        }
    }
    async fn connect(&self, who: usize) -> Socket {
        timeout(
            Duration::from_secs(3),
            client::connect_unix_client(&self.socket, &self.keys[who], "local", None),
        )
        .await
        .expect("connect deadline")
        .unwrap()
    }
}
async fn call(socket: &mut Socket, request: Value) -> Value {
    timeout(Duration::from_secs(3), client::exchange(socket, &request))
        .await
        .expect("response deadline")
        .unwrap()
}
fn publish(id: &str) -> Value {
    json!({"version":1,"type":"publish","id":id,"topic":TOPIC,"payload":{"text":"test"}})
}
fn poll() -> Value {
    json!({"version":1,"type":"poll","id":"poll","topic":TOPIC})
}
async fn closed(socket: &mut Socket) {
    let response = timeout(Duration::from_secs(4), socket.next())
        .await
        .expect("connection must close");
    assert!(
        matches!(response, None | Some(Err(_)) | Some(Ok(Message::Close(_)))),
        "{response:?}"
    );
}

#[tokio::test]
async fn reader_abort_preserves_message_on_reconnect() {
    if support::under_unprivileged_identity("reader_abort_preserves_message_on_reconnect") {
        return;
    }
    let service = Service::start(false).await;
    let mut producer = service.connect(0).await;
    let published = call(&mut producer, publish("publish")).await;
    let mut reader = service.connect(1).await;
    let first = call(&mut reader, poll()).await;
    drop(reader); // Abrupt transport loss after poll, no WebSocket close handshake.
    let mut reader = service.connect(1).await;
    let replay = call(&mut reader, poll()).await;
    assert_eq!(replay["items"], first["items"]);

    let _status = call(
        &mut producer,
        json!({"version":1,"type":"get_message","id":"own","message_id":published["message_id"]}),
    )
    .await;

    assert_eq!(
        call(&mut reader, poll()).await["items"],
        first["items"],
        "Polling does not delete content"
    );
    let mut next = poll();
    next["after"] = replay["next_cursor"].clone();
    assert_eq!(call(&mut reader, next).await["items"], json!([]));
}

#[tokio::test]
async fn repeated_publish_request_id_is_not_an_idempotency_key() {
    if support::under_unprivileged_identity("repeated_publish_request_id_is_not_an_idempotency_key")
    {
        return;
    }
    let service = Service::start(false).await;
    let mut producer = service.connect(0).await;
    let first = call(&mut producer, publish("same-request-id")).await;
    drop(producer);
    let mut producer = service.connect(0).await;
    let second = call(&mut producer, publish("same-request-id")).await;
    assert_ne!(first["message_id"], second["message_id"]);
    let mut reader = service.connect(1).await;
    let page = call(&mut reader, poll()).await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["message"]["id"], first["message_id"]);
    assert_eq!(items[1]["message"]["id"], second["message_id"]);
}

#[tokio::test]
async fn malformed_and_oversize_payloads_cannot_publish_or_break_other_clients() {
    if support::under_unprivileged_identity(
        "malformed_and_oversize_payloads_cannot_publish_or_break_other_clients",
    ) {
        return;
    }
    let service = Service::start(false).await;
    let mut bad = service.connect(0).await;
    bad.send(Message::Binary(vec![0xff, 0xff, 0xff].into()))
        .await
        .unwrap();
    let response = timeout(Duration::from_secs(3), bad.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Binary(raw) = response else {
        panic!("expected protobuf error")
    };
    let error = mynofy_protocol::pb::Frame::decode(raw.as_ref()).unwrap();
    assert!(matches!(
        error.body,
        Some(mynofy_protocol::pb::Body::Error(_))
    ));
    // Bypass SDK validation to exercise the server's payload limit.
    let oversize = mynofy_protocol::pb::Frame {
        version: 1,
        body: Some(mynofy_protocol::pb::Body::Publish(
            mynofy_protocol::pb::Publish {
                id: "oversize".into(),
                topic: TOPIC.into(),
                expires: None,
                payload: mynofy_protocol::payload::Payload::bytes(vec![42; 4097], None),
            },
        )),
    };
    bad.send(Message::Binary(oversize.encode_to_vec().into()))
        .await
        .unwrap();
    let reply = timeout(Duration::from_secs(3), bad.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Binary(raw) = reply else {
        panic!("expected protobuf error");
    };
    assert!(matches!(
        mynofy_protocol::pb::Frame::decode(raw).unwrap().body,
        Some(mynofy_protocol::pb::Body::Error(_))
    ));
    drop(bad);
    let mut healthy = service.connect(1).await;
    assert_eq!(call(&mut healthy, poll()).await["items"], json!([]));
    assert_eq!(
        call(&mut healthy, publish("healthy")).await["type"],
        "result"
    );
    assert_eq!(
        call(&mut healthy, poll()).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn acl_revocation_closes_live_session_but_preserves_own_content() {
    if support::under_unprivileged_identity(
        "acl_revocation_closes_live_session_but_preserves_own_content",
    ) {
        return;
    }
    let service = Service::start(false).await;
    let mut producer = service.connect(0).await;
    let published = call(&mut producer, publish("before-revoke")).await;
    let changed = timeout(
        Duration::from_secs(3),
        admin::request(
            &service.admin,
            admin::Command::AclSet {
                client: service.ids[0].clone(),
                topic: TOPIC.into(),
                allow: vec![],
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(changed["ok"], true);
    closed(&mut producer).await;
    let mut producer = service.connect(0).await;
    assert_eq!(
        call(&mut producer, publish("forbidden")).await["code"],
        "forbidden"
    );
    assert_eq!(call(&mut producer, poll()).await["code"], "forbidden");
    let own = call(
        &mut producer,
        json!({"version":1,"type":"get_message","id":"own","message_id":published["message_id"]}),
    )
    .await;
    assert_eq!(own["message"]["payload"]["text"], "test");
    let mut other = service.connect(1).await;
    assert_eq!(
        call(&mut other, poll()).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unauthenticated_and_idle_clients_expire_without_stopping_service() {
    if support::under_unprivileged_identity(
        "unauthenticated_and_idle_clients_expire_without_stopping_service",
    ) {
        return;
    }
    let service = Service::start(true).await;
    let stream = UnixStream::connect(&service.socket).await.unwrap();
    let (mut unauthenticated, _) = tokio_tungstenite::client_async("ws://localhost", stream)
        .await
        .unwrap();
    closed(&mut unauthenticated).await;
    let mut idle = service.connect(0).await;
    closed(&mut idle).await;
    let mut recovered = service.connect(1).await;
    assert_eq!(
        call(&mut recovered, publish("after-timeouts")).await["type"],
        "result"
    );
    assert_eq!(
        call(&mut recovered, poll()).await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unix_rejects_json_application_frames_before_and_after_authentication() {
    let service = Service::start(false).await;
    for authenticated in [false, true] {
        let mut socket = if authenticated {
            service.connect(0).await
        } else {
            tokio_tungstenite::client_async(
                "ws://localhost/",
                UnixStream::connect(&service.socket).await.unwrap(),
            )
            .await
            .unwrap()
            .0
        };
        let frame = if authenticated {
            publish("json-must-not-publish")
        } else {
            json!({"version":1,"type":"auth_hello","id":"json-hello","token_id":crypto::token_id(&crypto::public_key(&service.keys[0]).unwrap())})
        };
        socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        let reply = timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Close(Some(close)) = reply else {
            panic!("JSON client frame must close the connection");
        };
        assert_eq!(
            close.code,
            tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Unsupported
        );
    }
    let mut healthy = service.connect(1).await;
    assert_eq!(call(&mut healthy, poll()).await["items"], json!([]));
    assert_eq!(
        call(&mut healthy, publish("binary-still-works")).await["type"],
        "result"
    );
}

#[tokio::test]
async fn capacity_rejections_correlate_without_publish_and_preserve_the_session() {
    if support::under_unprivileged_identity(
        "capacity_rejections_correlate_without_publish_and_preserve_the_session",
    ) {
        return;
    }
    use mynofy_protocol::{payload::Payload, pb};
    let service = Service::start_with_capacity(false, Some(1)).await;
    let mut producer = service.connect(0).await;
    for index in 0..100 {
        let reply = client::exchange_frame(
            &mut producer,
            &pb::Frame {
                version: 1,
                body: Some(pb::Body::Publish(pb::Publish {
                    id: format!("p-{index}"),
                    topic: TOPIC.into(),
                    expires: None,
                    payload: Payload::bytes(vec![42; 4096], None),
                })),
            },
        )
        .await
        .unwrap();
        assert_eq!(reply.kind(), "result");
    }
    let mut slow = service.connect(0).await;
    slow.send(Message::Binary(
        pb::Frame::from_json(
            json!({"version":1,"type":"poll","id":"slow","topic":TOPIC,"limit":100}),
        )
        .unwrap()
        .encode_to_vec()
        .into(),
    ))
    .await
    .unwrap();
    let mut probe = service.connect(0).await;
    timeout(Duration::from_secs(3), async {
        loop {
            let reply = call(
                &mut probe,
                json!({"version":1,"type":"get_permissions","id":"capacity-ready","topic":TOPIC}),
            )
            .await;
            if reply["code"] == "resource_exhausted" {
                break;
            }
            assert_eq!(reply["type"], "permissions");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("first response must hold the shared permit");
    let rejected = call(&mut probe, publish("rejected-publish")).await;
    assert_eq!(rejected["code"], "resource_exhausted");
    assert_eq!(rejected["request_id"], "rejected-publish");
    // Invalid metadata is closed without executing or inventing a correlation ID.
    let mut malformed = service.connect(0).await;
    malformed
        .send(Message::Binary(
            pb::Frame::from_json(publish(&"x".repeat(129)))
                .unwrap()
                .encode_to_vec()
                .into(),
        ))
        .await
        .unwrap();
    let answer = timeout(Duration::from_secs(3), malformed.next())
        .await
        .unwrap();
    assert!(!matches!(
        answer,
        Some(Ok(Message::Binary(_))) | Some(Ok(Message::Text(_)))
    ));
    let status = admin::request(&service.admin, admin::Command::Status {})
        .await
        .unwrap();
    assert_eq!(status["result"]["resident_messages"], 100);
    let Message::Binary(page) = timeout(Duration::from_secs(3), slow.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    else {
        panic!("Expected poll response");
    };
    let page = pb::Frame::decode(page).unwrap();
    let Some(pb::Body::Messages(page)) = page.body else {
        panic!("Expected page");
    };
    assert_eq!(page.items.len(), 100);
    timeout(Duration::from_secs(3), async {
        loop {
            let reply = call(
                &mut probe,
                json!({"version":1,"type":"get_permissions","id":"after-release","topic":TOPIC}),
            )
            .await;
            if reply["type"] == "permissions" {
                break;
            }
            assert_eq!(reply["code"], "resource_exhausted");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the same connection must work after permit release");
    assert_eq!(
        call(&mut probe, publish("accepted-after-release")).await["type"],
        "result"
    );
    let status = admin::request(&service.admin, admin::Command::Status {})
        .await
        .unwrap();
    assert_eq!(status["result"]["resident_messages"], 101);
}
