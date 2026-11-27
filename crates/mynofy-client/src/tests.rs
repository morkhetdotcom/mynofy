use super::*;
use protocol::ProtobufMessage as _;
use tokio::io::{DuplexStream, duplex};
use tokio_tungstenite::tungstenite::protocol::Role;

async fn pair() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (a, b) = duplex(4096);
    let client = WebSocketStream::from_raw_socket(a, Role::Client, None).await;
    let peer = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
    (client, peer)
}

#[test]
fn exchange_sends_request_and_handles_ping_without_server_dependency() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async{
        let(mut client,mut peer)=pair().await;
        let responder=tokio::spawn(async move{
            let Message::Binary(raw)=peer.next().await.unwrap().unwrap() else{panic!("Expected binary frame")};
            let request=protocol::pb::Frame::decode(raw).unwrap().to_json().unwrap();
            assert_eq!(request["topic"],"events");
            peer.send(Message::Ping(Vec::new().into())).await.unwrap();
            peer.send(Message::Binary(protocol::pb::Frame::from_json(json!({"version":1,"type":"messages","request_id":request["id"],"items":[],"next_cursor":"00000000","has_more":false})).unwrap().encode_to_vec().into())).await.unwrap();
            assert!(matches!(peer.next().await.unwrap().unwrap(),Message::Pong(_)));
        });
        let answer=exchange(&mut client,&json!({"version":1,"type":"poll","id":"request-1","topic":"events"})).await.unwrap();
        assert_eq!(answer["request_id"],"request-1");assert_eq!(answer["items"],json!([]));
        responder.await.unwrap();
    });
}

#[test]
fn exchange_returns_error_when_peer_closes_before_response() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (mut client, mut peer) = pair().await;
            let responder = tokio::spawn(async move {
                let _ = peer.next().await;
                peer.close(None).await.unwrap();
            });
            let answer = exchange(
                &mut client,
                &json!({"version":1,"type":"poll","id":"request-1","topic":"events"}),
            )
            .await;
            assert!(answer.is_err());
            responder.await.unwrap();
        });
}

#[test]
fn catalog_helper_rejects_errors_and_mismatched_definitions() {
    use protocol::ProtobufMessage as _;
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        for answer in [
            json!({"version":1,"type":"error","request_id":"get-catalog","code":"not_found","detail":"unavailable"}),
            json!({"version":1,"type":"catalog","request_id":"get-catalog","catalog":{"name":"mynofy","version":2}}),
            json!({"version":1,"type":"catalog","request_id":"wrong","catalog":{"name":"mynofy","version":1}}),
        ] {
            let (mut client,mut peer) = pair().await;
            let responder=tokio::spawn(async move {
                let Message::Binary(raw)=peer.next().await.unwrap().unwrap() else {panic!("Expected catalog request")};
                let query=protocol::pb::Frame::decode(raw).unwrap().to_json().unwrap();
                assert_eq!(query["type"],"get_catalog");assert_eq!(query["catalog_version"],1);
                let response=protocol::pb::Frame::from_json(answer).unwrap();
                peer.send(Message::Binary(response.encode_to_vec().into())).await.unwrap();
            });
            assert!(get_catalog(&mut client,"mynofy",1).await.is_err());
            responder.await.unwrap();
        }
    });
}

#[tokio::test]
async fn authentication_rejects_welcome_without_restart_counter() {
    use protocol::{ProtobufMessage, pb};
    let key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&key).unwrap();
    let client_id = crypto::initial_client_id(&public);
    let (socket, mut peer) = pair().await;
    let server = tokio::spawn(async move {
        assert!(matches!(
            peer.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));
        peer.send(Message::Binary(
            pb::Frame {
                version: 1,
                body: Some(pb::Body::AuthChallenge(pb::AuthChallenge {
                    request_id: "auth-hello".into(),
                    service_id: "owner".into(),
                    nonce: crypto::encode(&[7; 32]),
                })),
            }
            .encode_to_vec()
            .into(),
        ))
        .await
        .unwrap();
        assert!(matches!(
            peer.next().await.unwrap().unwrap(),
            Message::Binary(_)
        ));
        peer.send(Message::Binary(
            pb::Frame {
                version: 1,
                body: Some(pb::Body::AuthWelcome(pb::AuthWelcome {
                    request_id: "auth-proof".into(),
                    service_id: "owner".into(),
                    client_id,
                    restart_counter: None,
                    boot_id: None,
                })),
            }
            .encode_to_vec()
            .into(),
        ))
        .await
        .unwrap();
    });
    assert!(authenticate(socket, &key, "owner", None).await.is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn cancelled_or_timed_out_exchange_poison_socket_before_late_reply() {
    use protocol::{ProtobufMessage, pb};
    for timed_out in [false, true] {
        let (mut client, mut peer) = pair().await;
        let request = pb::Frame::from_json(
            json!({"version":1,"type":"publish","id":"first","topic":"t","payload":1}),
        )
        .unwrap();
        let (sent, seen) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            assert!(matches!(
                peer.next().await.unwrap().unwrap(),
                Message::Binary(_)
            ));
            sent.send(()).unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            let result = pb::Frame::from_json(json!({"version":1,"type":"result","request_id":"first","message_id":"00000001","expires":{"time":"2030-01-01T00:00:00Z"}})).unwrap();
            let _ = peer
                .send(Message::Binary(result.encode_to_vec().into()))
                .await;
            // Any follow-up frame may only be Close, never another mutation.
            if let Some(Ok(message)) = peer.next().await {
                assert!(matches!(message, Message::Close(_)));
            }
        });
        if timed_out {
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(10),
                    exchange_frame(&mut client, &request)
                )
                .await
                .is_err()
            );
        } else {
            tokio::select! { biased; _ = seen => {}, result = exchange_frame(&mut client, &request) => panic!("unexpected {result:?}") }
        }
        // Reusing the SAME request ID must also fail; correlation alone is insufficient.
        assert!(exchange_frame(&mut client, &request).await.is_err());
        drop(client);
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn mismatched_response_id_or_type_poison_session() {
    use protocol::{ProtobufMessage, pb};
    for text_response in [false, true] {
        for wrong_type in [false, true] {
            let (mut client, mut peer) = pair().await;
            let request = json!({"version":1,"type":"poll","id":"current","topic":"t"});
            let worker = tokio::spawn(async move {
                peer.next().await.unwrap().unwrap();
                let value = if wrong_type {
                    json!({"version":1,"type":"result","request_id":"current"})
                } else {
                    json!({"version":1,"type":"messages","request_id":"old","items":[],"next_cursor":"00000000","has_more":false})
                };
                let message = if text_response {
                    Message::Text(value.to_string().into())
                } else {
                    Message::Binary(pb::Frame::from_json(value).unwrap().encode_to_vec().into())
                };
                peer.send(message).await.unwrap();
                peer.next().await
            });
            let answer = exchange(&mut client, &request).await;
            assert!(answer.is_err());
            assert!(exchange(&mut client, &request).await.is_err());
            drop(client);
            let _ = worker.await.unwrap();
        }
    }
}

#[tokio::test]
async fn uncorrelated_rate_limit_is_regular_error_and_session_remains_usable() {
    use protocol::{ProtobufMessage, pb};
    let (mut client, mut peer) = pair().await;
    let worker = tokio::spawn(async move {
        for rate in [true, false] {
            peer.next().await.unwrap().unwrap();
            let response = if rate {
                json!({"version":1,"type":"error","code":"rate_limited","detail":"wait","retry_after_ms":10})
            } else {
                json!({"version":1,"type":"messages","request_id":"poll","items":[],"next_cursor":"00000000","has_more":false})
            };
            peer.send(Message::Binary(
                pb::Frame::from_json(response)
                    .unwrap()
                    .encode_to_vec()
                    .into(),
            ))
            .await
            .unwrap();
        }
    });
    let q = json!({"version":1,"type":"poll","id":"poll","topic":"t"});
    assert_eq!(
        exchange(&mut client, &q).await.unwrap()["code"],
        "rate_limited"
    );
    assert_eq!(exchange(&mut client, &q).await.unwrap()["type"], "messages");
    worker.await.unwrap();
}

#[test]
fn rotation_rate_rejection_is_typed_and_never_advances_to_confirmation() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let (mut socket, mut peer) = pair().await;
        let task = tokio::spawn(async move {
            let Message::Binary(raw) = peer.next().await.unwrap().unwrap() else { panic!("binary") };
            assert_eq!(protocol::pb::Frame::decode(raw).unwrap().kind(), "rotation_start");
            let error = protocol::pb::Frame::from_json(json!({"version":1,"type":"error","code":"rate_limited","detail":"wait","retry_after_ms":321})).unwrap();
            peer.send(Message::Binary(error.encode_to_vec().into())).await.unwrap();
            // A second rotation phase would be an implicit mutation retry.
            assert!(timeout(Duration::from_millis(20), peer.next()).await.is_err());
        });
        let old = crypto::generate_key().unwrap();
        let new = crypto::generate_key().unwrap();
        let error = rotate_client(&mut socket, &old, &new, "local", "client").await.unwrap_err();
        let error = error.downcast_ref::<RotationError>().unwrap();
        assert_eq!(error.phase, RotationPhase::Start);
        assert!(!error.outcome_unknown());
        let broker = error.broker.as_ref().unwrap();
        assert_eq!(broker.code, "rate_limited");
        assert_eq!(broker.detail, "wait");
        assert_eq!(broker.retry_after_ms, Some(321));
        task.await.unwrap();
    });
}

#[test]
fn json_commands_reject_explicit_nulls_but_allow_absent_expiry_and_null_data() {
    let base = json!({"version":1,"type":"publish","id":"p","topic":"events","payload":null});
    assert!(protocol::pb::Frame::from_json(base.clone()).is_ok());
    assert!(protocol::parse_value(base.clone()).is_ok());
    for field in ["expires", "topic", "id"] {
        let mut invalid = base.clone();
        invalid[field] = Value::Null;
        assert!(protocol::parse_value(invalid.clone()).is_err());
        assert!(protocol::pb::Frame::from_json(invalid).is_err());
    }
}

#[tokio::test]
async fn shared_reply_vectors_validate_command_semantics_and_poison_invalid_sessions() {
    let vectors: Value =
        serde_json::from_str(include_str!("../../../protocol/conformance/sdk-v1.json")).unwrap();
    for case in vectors["reply_validation"].as_array().unwrap() {
        let request = protocol::pb::Frame::from_json(case["request"].clone()).unwrap();
        let response = protocol::pb::Frame::from_json(case["response"].clone()).unwrap();
        assert_eq!(
            crypto::encode(&request.encode_to_vec()),
            case["request_base64"]
        );
        assert_eq!(
            crypto::encode(&response.encode_to_vec()),
            case["response_base64"]
        );
        let valid = case["valid"].as_bool().unwrap();
        let (mut socket, mut peer) = pair().await;
        let expected = request.clone();
        let task = tokio::spawn(async move {
            for _ in 0..if valid { 2 } else { 1 } {
                let Message::Binary(bytes) = peer.next().await.unwrap().unwrap() else {
                    panic!("binary request")
                };
                assert_eq!(protocol::pb::Frame::decode(bytes).unwrap(), expected);
                peer.send(Message::Binary(response.encode_to_vec().into()))
                    .await
                    .unwrap();
            }
            // A broken session must not send another application command.
            if !valid && let Some(Ok(message)) = peer.next().await {
                assert!(!matches!(message, Message::Binary(_)));
            }
        });
        let result = exchange_frame(&mut socket, &request).await;
        assert_eq!(result.is_ok(), valid, "{}", case["name"]);
        assert_eq!(exchange_frame(&mut socket, &request).await.is_ok(), valid);
        drop(socket);
        task.await.unwrap();
    }
}

#[test]
fn topic_scoped_replies_reject_mismatched_items_and_permissions() {
    use protocol::pb;
    let frame = |body| pb::Frame {
        version: 1,
        body: Some(body),
    };
    for (request, response) in [
        (
            pb::Body::Poll(pb::Poll {
                id: "r".into(),
                topic: "expected".into(),
                ..Default::default()
            }),
            pb::Body::Messages(pb::Messages {
                request_id: "r".into(),
                items: vec![pb::PolledMessage {
                    message: pb::StoredMessage {
                        topic: "unexpected".into(),
                        ..Default::default()
                    },
                }],
                ..Default::default()
            }),
        ),
        (
            pb::Body::ListMessages(pb::ListMessages {
                id: "r".into(),
                topic: Some("expected".into()),
                ..Default::default()
            }),
            pb::Body::MessagesPage(pb::MessagesPage {
                request_id: "r".into(),
                items: vec![pb::MessageSummary {
                    topic: "unexpected".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        ),
        (
            pb::Body::GetPermissions(pb::GetPermissions {
                id: "r".into(),
                topic: "expected".into(),
            }),
            pb::Body::Permissions(pb::Permissions {
                request_id: "r".into(),
                topic: "unexpected".into(),
                ..Default::default()
            }),
        ),
    ] {
        let error = validate_response(&frame(request), &frame(response)).unwrap_err();
        assert!(error.to_string().contains("topic mismatch"));
    }
}

#[test]
fn boot_identity_matches_shared_conformance_vectors() {
    let vectors: Value =
        serde_json::from_str(include_str!("../../../protocol/conformance/sdk-v1.json")).unwrap();
    for row in vectors["boot_identity"].as_array().unwrap() {
        assert_eq!(
            validate_boot_id(row["value"].as_str()).is_ok(),
            row["valid"].as_bool().unwrap(),
            "{}",
            row["name"],
        );
    }
}
