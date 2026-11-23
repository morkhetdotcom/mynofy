#![cfg(unix)]
#[cfg(test)]
mod fixtures {
    use mynofy_server::{
        config::{Action, Client, Config, PublicKey, Rule},
        crypto,
        engine::Engine,
        protocol,
    };
    use serde_json::json;
    const NOW: u64 = 1_800_000_000_000;
    pub fn setup() -> (Engine, Vec<u8>, u64) {
        let key = crypto::generate_key().unwrap();
        let public = crypto::public_key(&key).unwrap();
        let id = crypto::initial_client_id(&public);
        let mut cfg: Config = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/scenarios/local.json"
        )))
        .unwrap();
        cfg.clients = vec![Client {
            client_id: id.clone(),
            display_name: None,
            disabled: false,
            keys: vec![PublicKey {
                algorithm: "ed25519".into(),
                public_key: crypto::encode(&public),
                valid_until: protocol::timestamp(NOW + 86400000).unwrap(),
            }],
        }];
        cfg.topics[0].acl = Some(vec![Rule {
            client_id: id,
            allow: vec![Action::Publish, Action::Subscribe],
        }]);
        cfg.limits.client_burst = 1;
        cfg.limits.client_requests_per_second = 1;
        let mut e = Engine::new(cfg, NOW).unwrap();
        let s = e.connect(NOW).unwrap();
        let h = e.handle(
            s,
            &json!({"version":1,"type":"auth_hello","id":"h","token_id":crypto::token_id(&public)})
                .to_string(),
            NOW,
        );
        let nonce = crypto::decode(h["nonce"].as_str().unwrap()).unwrap();
        let sig = crypto::sign(
            &key,
            &crypto::auth_bytes(&nonce, &public, &e.config.service.id),
        )
        .unwrap();
        assert_eq!(
            e.handle(
                s,
                &json!({"version":1,"type":"auth_proof","id":"p","auth_id":"h","signature":sig})
                    .to_string(),
                NOW
            )["type"],
            "auth_welcome"
        );
        (e, key, s)
    }
}

#[cfg(test)]
mod transport_tests {
    use super::fixtures::*;
    use futures_util::{SinkExt, StreamExt};
    use mynofy_server::{
        config::{Action, Client, PublicKey, Rule},
        crypto,
        protocol::{self, ProtobufMessage, pb},
        transport,
    };
    use tokio_tungstenite::tungstenite::Message;
    #[tokio::test]
    async fn rotation_cancels_other_old_key_session_output() {
        let (e, key, _) = setup();
        let mut cfg = e.config.clone();
        drop(e);
        let directory =
            std::env::temp_dir().join(format!("mynofy-review3-rotation-{}", std::process::id()));
        assert!(!directory.exists());
        std::fs::create_dir(&directory).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = directory.join("s");
        let admin_path = directory.join("a");
        cfg.service.listen = None;
        cfg.service.tls = None;
        cfg.service.unix_socket = Some(socket.clone());
        cfg.service.admin_socket = Some(admin_path.clone());
        cfg.limits.client_burst = 1000000;
        cfg.limits.client_requests_per_second = 1000000;
        cfg.limits.max_response_bytes = 4 * 1024 * 1024;
        for key in cfg.clients.iter_mut().flat_map(|client| &mut client.keys) {
            key.valid_until = protocol::timestamp(transport::now_ms() + 86_400_000).unwrap();
        }
        let subkey = crypto::generate_key().unwrap();
        let public = crypto::public_key(&subkey).unwrap();
        let client = crypto::initial_client_id(&public);
        cfg.clients.push(Client {
            client_id: client.clone(),
            display_name: None,
            disabled: false,
            keys: vec![PublicKey {
                algorithm: "ed25519".into(),
                public_key: crypto::encode(&public),
                valid_until: protocol::timestamp(transport::now_ms() + 86_400_000).unwrap(),
            }],
        });
        cfg.topics[0].acl.as_mut().unwrap().push(Rule {
            client_id: client.clone(),
            allow: vec![Action::Subscribe],
        });
        let owner = cfg.service.id.clone();
        let bound = transport::bind_unix(&socket).unwrap();
        let server = tokio::spawn(transport::run_unix(bound, cfg));
        let mut ws = mynofy_client::connect_unix_client(&socket, &key, &owner, None)
            .await
            .unwrap();
        for _ in 0..100 {
            let q = pb::Frame {
                version: 1,
                body: Some(pb::Body::Publish(pb::Publish {
                    id: "p".into(),
                    topic: "local/events".into(),
                    payload: protocol::payload::Payload::bytes(vec![42; 4096], None),
                    expires: None,
                })),
            };
            assert_eq!(
                mynofy_client::exchange_frame(&mut ws, &q)
                    .await
                    .unwrap()
                    .kind(),
                "result"
            );
        }
        let mut reader = mynofy_client::connect_unix_client(&socket, &subkey, &owner, None)
            .await
            .unwrap();
        let q=pb::Frame::from_json(serde_json::json!({"version":1,"type":"poll","id":"poll","topic":"local/events","limit":100})).unwrap();
        reader
            .send(Message::Binary(q.encode_to_vec().into()))
            .await
            .unwrap();
        // Observe actual queued output without consuming a frame. The reader
        // stays blocked, so revocation must cancel the pending response send.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.get_ref().readable(),
        )
        .await
        .unwrap()
        .unwrap();

        let newkey = crypto::generate_key().unwrap();
        let mut rotator = mynofy_client::connect_unix_client(&socket, &subkey, &owner, None)
            .await
            .unwrap();
        let rotated = mynofy_client::rotate_client(&mut rotator, &subkey, &newkey, &owner, &client)
            .await
            .unwrap();
        assert_eq!(rotated["type"], "rotation_complete");
        assert!(
            mynofy_client::connect_unix_client(&socket, &subkey, &owner, None)
                .await
                .is_err()
        );
        let new_session =
            mynofy_client::connect_unix_client(&socket, &newkey, &owner, Some(&client))
                .await
                .unwrap();
        println!("Rotation committed; old key denied, new key accepted");
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), reader.next())
            .await
            .unwrap();
        assert!(
            !matches!(response, Some(Ok(Message::Binary(_)))),
            "invalid credential completed a blocked sensitive response"
        );
        server.abort();
        let _ = server.await;
        drop(ws);
        drop(reader);
        drop(rotator);
        drop(new_session);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn expiry_cancels_blocked_output() {
        let (e, key, _) = setup();
        let mut cfg = e.config.clone();
        drop(e);
        let directory =
            std::env::temp_dir().join(format!("mynofy-review3-expiry-{}", std::process::id()));
        assert!(!directory.exists());
        std::fs::create_dir(&directory).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = directory.join("s");
        let admin_path = directory.join("a");
        cfg.service.listen = None;
        cfg.service.tls = None;
        cfg.service.unix_socket = Some(socket.clone());
        cfg.service.admin_socket = Some(admin_path.clone());
        cfg.limits.client_burst = 1000000;
        cfg.limits.client_requests_per_second = 1000000;
        cfg.limits.max_response_bytes = 4 * 1024 * 1024;
        for key in cfg.clients.iter_mut().flat_map(|client| &mut client.keys) {
            key.valid_until = protocol::timestamp(transport::now_ms() + 86_400_000).unwrap();
        }
        let expiry_at = transport::now_ms() + 2500;
        let subkey = crypto::generate_key().unwrap();
        let public = crypto::public_key(&subkey).unwrap();
        let client = crypto::initial_client_id(&public);
        cfg.clients.push(Client {
            client_id: client.clone(),
            display_name: None,
            disabled: false,
            keys: vec![PublicKey {
                algorithm: "ed25519".into(),
                public_key: crypto::encode(&public),
                valid_until: protocol::timestamp(expiry_at).unwrap(),
            }],
        });
        cfg.topics[0].acl.as_mut().unwrap().push(Rule {
            client_id: client.clone(),
            allow: vec![Action::Subscribe],
        });
        let owner = cfg.service.id.clone();
        let bound = transport::bind_unix(&socket).unwrap();
        let server = tokio::spawn(transport::run_unix(bound, cfg));
        let mut ws = mynofy_client::connect_unix_client(&socket, &key, &owner, None)
            .await
            .unwrap();
        for _ in 0..100 {
            let q = pb::Frame {
                version: 1,
                body: Some(pb::Body::Publish(pb::Publish {
                    id: "p".into(),
                    topic: "local/events".into(),
                    payload: protocol::payload::Payload::bytes(vec![42; 4096], None),
                    expires: None,
                })),
            };
            assert_eq!(
                mynofy_client::exchange_frame(&mut ws, &q)
                    .await
                    .unwrap()
                    .kind(),
                "result"
            );
        }
        let mut reader = mynofy_client::connect_unix_client(&socket, &subkey, &owner, None)
            .await
            .unwrap();
        let q=pb::Frame::from_json(serde_json::json!({"version":1,"type":"poll","id":"poll","topic":"local/events","limit":100})).unwrap();
        reader
            .send(Message::Binary(q.encode_to_vec().into()))
            .await
            .unwrap();
        // Observe actual queued output without consuming a frame. The reader
        // stays blocked, so revocation must cancel the pending response send.
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reader.get_ref().readable(),
        )
        .await
        .unwrap()
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(
            expiry_at.saturating_sub(transport::now_ms()) + 50,
        ))
        .await;
        assert!(
            mynofy_client::connect_unix_client(&socket, &subkey, &owner, None)
                .await
                .is_err()
        );
        println!("Credential expired; old key denied for new connection");
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), reader.next())
            .await
            .unwrap();
        assert!(
            !matches!(response, Some(Ok(Message::Binary(_)))),
            "invalid credential completed a blocked sensitive response"
        );
        server.abort();
        let _ = server.await;
        drop(ws);
        drop(reader);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::fixtures::*;
    use mynofy_server::{
        admin::{self, Command},
        config::{Config, Peer, SecurityStorage},
        engine::Engine,
    };
    use serde_json::json;
    #[test]
    fn snapshot_growth_is_rejected_before_mutation_and_capacity_is_reusable() {
        let (e, _, _) = setup();
        let mut cfg = e.config.clone();
        drop(e);
        let base = cfg.topics[0].clone();
        cfg.topics = (0..63)
            .map(|i| {
                let mut t = base.clone();
                t.name = format!("{i:03}{}", "x".repeat(250));
                t
            })
            .collect();
        let clients: serde_json::Map<_, _> = cfg
            .clients
            .iter()
            .map(|c| (c.client_id.clone(), serde_json::to_value(c).unwrap()))
            .collect();
        let acl: serde_json::Map<_, _> = cfg
            .topics
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    json!({cfg.clients[0].client_id.clone():["publish","subscribe"]}),
                )
            })
            .collect();
        let initial = json!({"version":1,"owner":cfg.service.id,"sequence":0,"state":{"peers":{},"disabled_peers":[],"revoked_peer_certificates":[],"restart_counter":1,"clients":clients,"acl":acl}});
        let mut bytes = serde_json::to_vec(&initial).unwrap().len();
        let target = 32 * 1024 * 1024 - 8000;
        for i in 0..10000 {
            let p = Peer {
                service_id: format!("peer{i:04}.test"),
                certificate_identity: None,
                topics: cfg.topics.iter().map(|t| t.name.clone()).collect(),
            };
            let added = serde_json::to_vec(&p.service_id).unwrap().len()
                + 1
                + serde_json::to_vec(&p).unwrap().len()
                + usize::from(!cfg.accepted_peers.is_empty());
            if bytes + added > target {
                break;
            }
            bytes += added;
            cfg.accepted_peers.push(p);
        }
        let directory =
            std::env::temp_dir().join(format!("mynofy-review3-snapshot-{}", std::process::id()));
        assert!(!directory.exists());
        cfg.security_storage = Some(SecurityStorage {
            directory: directory.clone(),
            queue_capacity: 4,
            compact_after_records: 1,
        });
        let raw = serde_json::to_string(&cfg).unwrap();
        assert!(raw.len() <= 32 * 1024 * 1024);
        cfg = Config::parse(std::path::Path::new("review.json"), &raw).unwrap();
        drop(raw);
        let baseline = cfg.clone();
        let mut engine = Engine::new(cfg, 1_800_000_000_000).unwrap();
        let snapshot = std::fs::metadata(directory.join("snapshot")).unwrap().len();
        println!(
            "baseline: {} peers; snapshot {} bytes",
            baseline.accepted_peers.len(),
            snapshot
        );
        assert!((snapshot as usize) < 32 * 1024 * 1024 + 80);
        let peer = Peer {
            service_id: "extra.test".into(),
            certificate_identity: None,
            topics: baseline.topics.iter().map(|t| t.name.clone()).collect(),
        };
        let command = Command::PeerSet { peer };
        let admin_len = serde_json::to_vec(&admin::Request {
            version: 1,
            command: command.clone(),
        })
        .unwrap()
        .len();
        assert!(admin_len <= 16384);
        assert!(engine.admin(command, 1_800_000_000_000).is_ok());
        let peer = Peer {
            service_id: "extra2.test".into(),
            certificate_identity: None,
            topics: baseline.topics.iter().map(|t| t.name.clone()).collect(),
        };
        let error = engine
            .admin(Command::PeerSet { peer: peer.clone() }, 1_800_000_000_000)
            .err()
            .unwrap();
        assert_eq!(error.code, "security_snapshot_too_large");
        assert!(
            !engine
                .config
                .accepted_peers
                .iter()
                .any(|p| p.service_id == "extra2.test")
        );
        let status =
            serde_json::to_value(engine.admin(Command::Status {}, 1_800_000_000_000).unwrap())
                .unwrap();
        assert_eq!(status["security_storage"]["submitted_sequence"], 1);
        engine
            .admin(
                Command::PeerDelete {
                    service_id: "extra.test".into(),
                },
                1_800_000_000_000,
            )
            .unwrap();
        engine
            .admin(Command::PeerSet { peer }, 1_800_000_000_000)
            .unwrap();
        let mut durable = false;
        for _ in 0..400 {
            let status =
                serde_json::to_value(engine.admin(Command::Status {}, 1_800_000_000_000).unwrap())
                    .unwrap();
            if status["security_storage"]["durable_sequence"] == 3
                && status["security_storage"]["snapshot_sequence"] == 3
            {
                assert!(status["security_storage"]["last_error"].is_null());
                durable = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(durable, "accepted security state was not compacted");
        drop(engine);
        let restarted = Engine::new(baseline, 1_800_000_001_000).unwrap();
        assert!(
            restarted
                .config
                .accepted_peers
                .iter()
                .any(|p| p.service_id == "extra2.test")
        );
        assert!(
            !restarted
                .config
                .accepted_peers
                .iter()
                .any(|p| p.service_id == "extra.test")
        );
        drop(restarted);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
