use super::*;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use std::{
    sync::{Arc, mpsc},
    thread,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{Role, WebSocketConfig},
    },
};

#[derive(Clone, Copy)]
enum Behavior {
    Drop,
    Wait,
    WrongId,
    Version,
    Text,
    Malformed,
    RateLimited,
    Good,
}
struct Peer(tokio::task::JoinHandle<()>);
impl Drop for Peer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn mock(behavior: Behavior, timeout: Duration) -> (Arc<Client>, mpsc::Receiver<pb::Frame>, Peer) {
    let rt = runtime().unwrap();
    let (c, mut s) = rt.block_on(async {
        let (a, b) = tokio::io::duplex(65536);
        let cfg = WebSocketConfig::default()
            .max_message_size(Some(4 * 1024 * 1024))
            .max_frame_size(Some(4 * 1024 * 1024));
        let c = WebSocketStream::from_raw_socket(a, Role::Client, Some(cfg)).await;
        let s = WebSocketStream::from_raw_socket(b, Role::Server, Some(cfg)).await;
        (c, s)
    });
    let (tx, rx) = mpsc::sync_channel(1);
    let task = rt.spawn(async move {
        let Some(Ok(Message::Binary(bytes))) = s.next().await else {
            return;
        };
        let frame = pb::Frame::decode(bytes).unwrap();
        let id = match &frame.body {
            Some(pb::Body::Publish(v)) => v.id.clone(),
            _ => panic!("test request kind"),
        };
        tx.send(frame).unwrap();
        let mut reply = pb::Frame {
            version: 1,
            body: Some(pb::Body::Result(pb::Result {
                request_id: id.clone(),
                message_id: Some("00000001".into()),
                expires: Some(pb::Expires {
                    time: "2030-01-01T00:00:00Z".into(),
                }),
            })),
        };
        match behavior {
            Behavior::Drop => return,
            Behavior::Wait => {
                std::future::pending::<()>().await;
                return;
            }
            Behavior::WrongId => {
                if let Some(pb::Body::Result(v)) = &mut reply.body {
                    v.request_id = "wrong".into();
                }
            }
            Behavior::Version => reply.version = 2,
            Behavior::Text => {
                s.send(Message::Text("{}".into())).await.unwrap();
                return;
            }
            Behavior::Malformed => {
                s.send(Message::Binary(vec![255].into())).await.unwrap();
                return;
            }
            Behavior::RateLimited => {
                reply.body = Some(pb::Body::Error(pb::Error {
                    request_id: None,
                    code: "rate_limited".into(),
                    detail: "wait".into(),
                    retry_after_ms: Some(100),
                }))
            }
            Behavior::Good => {}
        }
        let _ = s.send(Message::Binary(reply.encode_to_vec().into())).await;
        // Do not turn a successful reply into an unrelated EOF race.
        std::future::pending::<()>().await;
    });
    let key = Key::parse(&client::crypto::generate_key().unwrap()).unwrap();
    let id = key.id.clone();
    let (cancel, _) = watch::channel(false);
    let client = Arc::new(Client {
        state: Mutex::new(State {
            socket: Some(Socket::Memory(Box::new(c))),
            sequence: 0,
        }),
        closed: AtomicBool::new(false),
        cancel,
        key,
        owner: "local".into(),
        welcome: pb::AuthWelcome {
            request_id: "auth".into(),
            service_id: "local".into(),
            client_id: id,
            restart_counter: Some(1),
        },
        timeout,
        runtime: rt.clone(),
    });
    (client, rx, Peer(task))
}
fn publish(c: &Client) -> (i32, abi::Error, *mut Response) {
    let mut e = abi::Error::default();
    let mut out = std::ptr::null_mut();
    // SAFETY: all input strings/buffers/handles and output storage live through the call.
    let status = unsafe {
        mynofy_publish_bytes(
            c,
            c"local/events".as_ptr(),
            b"once".as_ptr(),
            4,
            std::ptr::null(),
            std::ptr::null(),
            &mut out,
            &mut e,
        )
    };
    (status, e, out)
}
#[test]
fn lost_publish_closes_without_retry() {
    let (c, rx, _peer) = mock(Behavior::Drop, Duration::from_secs(1));
    let (status, e, out) = publish(&c);
    assert_eq!(status, 4);
    assert_eq!(e.outcome_unknown, 1);
    assert!(out.is_null());
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap().body,
        Some(pb::Body::Publish(_))
    ));
    assert_eq!(publish(&c).0, 3);
    assert!(rx.try_recv().is_err());
}
#[test]
fn invalid_responses_discard_connection() {
    for behavior in [
        Behavior::WrongId,
        Behavior::Version,
        Behavior::Text,
        Behavior::Malformed,
    ] {
        let (c, _rx, _peer) = mock(behavior, Duration::from_secs(1));
        let (status, e, out) = publish(&c);
        assert_eq!(status, 4);
        assert_eq!(e.outcome_unknown, 1);
        assert!(out.is_null());
        assert_eq!(publish(&c).0, 3);
    }
}
#[test]
fn rate_limit_is_structured_and_does_not_discard_connection() {
    let (c, _rx, _peer) = mock(Behavior::RateLimited, Duration::from_secs(1));
    let (status, e, out) = publish(&c);
    assert_eq!(status, 5);
    assert_eq!(e.outcome_unknown, 0);
    assert_eq!(e.has_retry_after_ms, 1);
    assert_eq!(e.retry_after_ms, 100);
    assert!(out.is_null());
    assert!(!c.closed.load(Ordering::Acquire));
    c.close();
}
#[test]
fn busy_and_concurrent_close_cancel_active_operation() {
    let (c, rx, _peer) = mock(Behavior::Wait, Duration::from_secs(2));
    let worker = c.clone();
    let task = thread::spawn(move || {
        let (s, e, out) = publish(&worker);
        assert!(out.is_null());
        (s, e.outcome_unknown)
    });
    rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(publish(&c).0, 2);
    // SAFETY: shared client remains alive until worker has joined.
    unsafe { mynofy_client_close(Arc::as_ptr(&c)) };
    assert_eq!(task.join().unwrap(), (7, 1));
    assert_eq!(publish(&c).0, 3);
}
#[test]
fn timeout_is_bounded_and_connection_is_closed() {
    let elapsed = runtime().unwrap().block_on(async {
        tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err()
    });
    let mapped = Failure::network(Box::new(elapsed), true);
    assert_eq!(mapped.code, 8);
    assert!(mapped.unknown);
    let (c, _rx, _peer) = mock(Behavior::Wait, Duration::from_millis(30));
    let (status, e, out) = publish(&c);
    assert_eq!(status, 8);
    assert_eq!(e.outcome_unknown, 1);
    assert!(out.is_null());
    assert_eq!(publish(&c).0, 3);
}
#[test]
fn c_event_and_integer_values_match_shared_wire_vectors() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../protocol/conformance/sdk-v1.json")).unwrap();
    let vector = vectors["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["json"]["id"] == "event")
        .unwrap();
    let expected = pb::Frame::from_json(vector["json"].clone()).unwrap();
    let Some(pb::Body::Publish(p)) = &expected.body else {
        panic!()
    };
    let Some(payload::Body::Event(event)) = &p.payload.body else {
        panic!()
    };
    let c_event = abi::event_view(event);
    let (c, rx, _peer) = mock(Behavior::Good, Duration::from_secs(1));
    let mut out = std::ptr::null_mut();
    let mut e = abi::Error::default();
    // SAFETY: fully initialized event, live client and valid outputs.
    let status = unsafe {
        mynofy_publish_event(
            Arc::as_ptr(&c),
            c"sdk/events".as_ptr(),
            &c_event,
            std::ptr::null(),
            &mut out,
            &mut e,
        )
    };
    assert_eq!(status, 0);
    let mut actual = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    set_id(&mut actual, "event");
    assert_eq!(
        client::crypto::encode(&actual.encode_to_vec()),
        vector["protobuf_base64"].as_str().unwrap()
    );
    // SAFETY: owned response, freed once.
    unsafe { response::mynofy_response_free(out) };
    c.close();
}
#[test]
fn invalid_ffi_inputs_and_key_lifetimes() {
    let mut out = std::ptr::null_mut();
    let mut e = abi::Error::default();
    // SAFETY: NULL/oversize inputs are intentionally supported validation cases; valid output.
    unsafe {
        assert_eq!(mynofy_key_parse(std::ptr::null(), 1, &mut out, &mut e), 1);
        assert!(out.is_null());
        assert_eq!(mynofy_key_generate(std::ptr::null_mut(), &mut e), 1);
        assert_eq!(mynofy_key_generate(&mut out, &mut e), 0);
        let exported = mynofy_key_pkcs8(out);
        assert_eq!(exported.len, 83);
        let mut imported = std::ptr::null_mut();
        assert_eq!(
            mynofy_key_parse(exported.data, exported.len, &mut imported, &mut e),
            0
        );
        assert_eq!(mynofy_key_public(imported).len, 32);
        mynofy_key_free(out);
        assert!(!mynofy_key_token_id(imported).data.is_null());
        mynofy_key_free(imported);
    }
}
#[test]
fn panic_is_contained_and_error_is_reinitialized() {
    let mut error = abi::Error::default();
    assert_eq!(boundary(&mut error, || panic!("injected test panic")), 6);
    assert_eq!(error.code, 6);
    assert_eq!(error.outcome_unknown, 1);
    assert_eq!(boundary(&mut error, || Ok(())), 0);
    assert_eq!(error.code, 0);
    assert_eq!(error.outcome_unknown, 0);
}

#[test]
fn rotation_rejections_preserve_broker_fields_and_lost_commit_stays_unknown() {
    use serde_json::json;
    for mode in [
        "start",
        "confirm",
        "commit",
        "lost",
        "lost-start",
        "lost-confirm",
        "wrong-id",
        "wrong-commit",
    ] {
        let (c, _rx, original_peer) = mock(Behavior::Good, Duration::from_secs(2));
        drop(original_peer);
        let (socket, mut peer) = c.runtime.block_on(async {
            let (a, b) = tokio::io::duplex(65536);
            (
                WebSocketStream::from_raw_socket(a, Role::Client, None).await,
                WebSocketStream::from_raw_socket(b, Role::Server, None).await,
            )
        });
        c.state.lock().unwrap().socket = Some(Socket::Memory(Box::new(socket)));
        let public = c.key.public;
        let id = c.welcome.client_id.clone();
        let task = c.runtime.spawn(async move {
            for phase in ["start", "confirm", "commit"] {
                let Some(Ok(Message::Binary(raw))) = peer.next().await else { panic!("missing rotation command") };
                let request = pb::Frame::decode(raw).unwrap();
                assert_eq!(request.kind(), format!("rotation_{phase}"));
                if (phase == "commit" && mode == "lost") || mode == format!("lost-{phase}") { return; }
                let wrong = mode == "wrong-id" || (phase == "commit" && mode == "wrong-commit");
                let reject = phase == mode || wrong;
                let answer = if reject {
                    json!({"version":1,"type":"error","request_id":if wrong {Some("wrong")} else if phase == "start" {None} else {request.request_id()},
                        "code":if phase == "start" {"rate_limited"} else {"rotation_expired"},"detail":"explicit rejection","retry_after_ms":123})
                } else if phase == "start" {
                    json!({"version":1,"type":"rotation_offer","request_id":request.request_id(),"rotation_id":"rotation","expires_at":"2030-01-01T00:00:00Z","new_valid_until":"2031-01-01T00:00:00Z"})
                } else {
                    let until = protocol::parse_time("2031-01-01T00:00:00Z").unwrap();
                    let aad = client::crypto::rotation_aad("local", &id, "rotation", &client::crypto::token_id(&public), until);
                    json!({"version":1,"type":"rotation_ready","request_id":request.request_id(),"rotation_id":"rotation","expires_at":"2030-01-01T00:00:00Z","new_valid_until":"2031-01-01T00:00:00Z","aad":client::crypto::encode(&aad),"nonce":client::crypto::encode(&[3;12]),"challenge":client::crypto::encode(&[4;32])})
                };
                peer.send(Message::Binary(pb::Frame::from_json(answer).unwrap().encode_to_vec().into())).await.unwrap();
                if reject {
                    if wrong { return; }
                    // The next command must be an explicit caller operation, not a rotation retry.
                    let Some(Ok(Message::Binary(raw))) = peer.next().await else { panic!("missing follow-up") };
                    let request = pb::Frame::decode(raw).unwrap();
                    assert_eq!(request.kind(), "publish");
                    let response = pb::Frame::from_json(json!({"version":1,"type":"result","request_id":request.request_id(),"message_id":"00000001","expires":{"time":"2030-01-01T00:00:00Z"}})).unwrap();
                    peer.send(Message::Binary(response.encode_to_vec().into())).await.unwrap();
                    std::future::pending::<()>().await;
                    return;
                }
            }
        });
        let peer = Peer(task);
        let new = Key::parse(&client::crypto::generate_key().unwrap()).unwrap();
        let mut error = abi::Error::default();
        let mut out = std::ptr::null_mut();
        // SAFETY: keys, client and output/error storage live through this call.
        let code = unsafe { mynofy_rotate(&*c, &new, &mut out, &mut error) };
        assert!(out.is_null());
        if ["start", "confirm", "commit"].contains(&mode) {
            assert_eq!(code, 5, "{mode}");
            assert_eq!(error.outcome_unknown, 0);
            assert_eq!(error.has_retry_after_ms, 1);
            assert_eq!(error.retry_after_ms, 123);
            let text = |v: &[std::ffi::c_char]| {
                String::from_utf8(v.iter().map(|v| *v as u8).take_while(|v| *v != 0).collect())
                    .unwrap()
            };
            assert_eq!(
                text(&error.broker_code),
                if mode == "start" {
                    "rate_limited"
                } else {
                    "rotation_expired"
                }
            );
            assert_eq!(text(&error.message), "explicit rejection");
            assert!(!c.closed.load(Ordering::Acquire));
            let (code, _, response) = publish(&c);
            assert_eq!(code, 0);
            // SAFETY: response is an owned live handle returned above.
            unsafe { response::mynofy_response_free(response) };
        } else {
            assert_eq!(code, 4, "{mode}");
            assert_eq!(
                error.outcome_unknown,
                u32::from(mode == "lost" || mode == "wrong-commit")
            );
            assert!(c.closed.load(Ordering::Acquire));
        }
        drop(peer);
    }
}

#[test]
fn shared_reply_vectors_preserve_the_c_semantic_countercheck() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../../../protocol/conformance/sdk-v1.json")).unwrap();
    for case in vectors["reply_validation"].as_array().unwrap() {
        let request = pb::Frame::from_json(case["request"].clone()).unwrap();
        let response = pb::Frame::from_json(case["response"].clone()).unwrap();
        let expected = if case["mutation"].as_bool().unwrap() {
            1
        } else {
            4
        };
        assert_eq!(
            response::validate(&response, "semantic-1", expected, Some(&request)).is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        let valid = case["valid"].as_bool().unwrap();
        let mutation = case["mutation"].as_bool().unwrap();
        let (client, _requests, old_peer) = mock(Behavior::Good, Duration::from_secs(2));
        drop(old_peer);
        let (socket, mut peer) = client.runtime.block_on(async {
            let (a, b) = tokio::io::duplex(65536);
            (
                WebSocketStream::from_raw_socket(a, Role::Client, None).await,
                WebSocketStream::from_raw_socket(b, Role::Server, None).await,
            )
        });
        client.state.lock().unwrap().socket = Some(Socket::Memory(Box::new(socket)));
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received = count.clone();
        let worker = client.runtime.spawn(async move {
            for _ in 0..if valid { 2 } else { 1 } {
                let Some(Ok(Message::Binary(raw))) = peer.next().await else {
                    panic!("missing request")
                };
                let request = pb::Frame::decode(raw).unwrap();
                received.fetch_add(1, Ordering::AcqRel);
                let mut reply = response.clone();
                match reply.body.as_mut().unwrap() {
                    pb::Body::Result(v) => v.request_id = request.request_id().unwrap().into(),
                    pb::Body::MessageStatus(v) => {
                        v.request_id = request.request_id().unwrap().into()
                    }
                    _ => panic!("vector response kind"),
                }
                peer.send(Message::Binary(reply.encode_to_vec().into()))
                    .await
                    .unwrap();
            }
            if !valid && let Some(Ok(message)) = peer.next().await {
                assert!(!matches!(message, Message::Binary(_)), "implicit retry");
            }
        });
        let _peer = Peer(worker);
        let operation = || {
            if mutation {
                publish(&client)
            } else {
                let mut error = abi::Error::default();
                let mut out = std::ptr::null_mut();
                // SAFETY: client, constant string and writable outputs live through the call.
                let code = unsafe {
                    mynofy_get_message(&*client, c"00000001".as_ptr(), &mut out, &mut error)
                };
                (code, error, out)
            }
        };
        for attempt in 0..2 {
            let (code, error, out) = operation();
            if valid {
                assert_eq!(code, 0);
                assert!(!out.is_null());
                // SAFETY: this is an owned response returned by the successful call.
                unsafe { response::mynofy_response_free(out) };
            } else {
                assert!(out.is_null());
                assert_eq!(code, if attempt == 0 { 4 } else { 3 });
                if attempt == 0 {
                    assert_eq!(error.outcome_unknown, u32::from(mutation));
                }
            }
        }
        assert_eq!(count.load(Ordering::Acquire), if valid { 2 } else { 1 });
    }
}
