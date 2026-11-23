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
mod security_tests {
    use super::fixtures::*;
    use mynofy_server::{
        admin::{self, Command},
        config::{Peer, SecurityStorage},
        engine::Engine,
    };
    use serde_json::json;
    #[test]
    fn oversized_peer_change_is_rejected_and_following_disable_becomes_durable() {
        let (e, _, _) = setup();
        let mut cfg = e.config.clone();
        drop(e);
        cfg.service.id = "s".repeat(256);
        cfg.topics[0].owner_id = cfg.service.id.clone();
        let base = cfg.topics[0].clone();
        cfg.topics.clear();
        for i in 0..63 {
            let mut t = base.clone();
            t.name = format!("{i:03}{}", "x".repeat(250));
            cfg.topics.push(t);
        }
        let directory =
            std::env::temp_dir().join(format!("mynofy-review2-journal-{}", std::process::id()));
        assert!(!directory.exists());
        cfg.security_storage = Some(SecurityStorage {
            directory: directory.clone(),
            queue_capacity: 4,
            compact_after_records: 4096,
        });
        let mut peer = Peer {
            service_id: "edge".into(),
            certificate_identity: None,
            topics: cfg.topics.iter().map(|t| t.name.clone()).collect(),
        };
        // Fit the public admin request inside its accepted 16 KiB envelope.
        while serde_json::to_vec(&admin::Request {
            version: 1,
            command: Command::PeerSet { peer: peer.clone() },
        })
        .unwrap()
        .len()
            > 16350
        {
            peer.topics.pop();
        }
        let raw = serde_json::to_vec(&admin::Request {
            version: 1,
            command: Command::PeerSet { peer: peer.clone() },
        })
        .unwrap();
        assert!(raw.len() <= admin::MAX_REQUEST_BYTES);
        let journal = json!({"version":1,"owner":cfg.service.id,"change":{"op":"peer","service_id":peer.service_id,"value":peer}});
        println!(
            "admin request {} bytes; journal payload {} bytes",
            raw.len(),
            serde_json::to_vec(&journal).unwrap().len()
        );
        assert!(serde_json::to_vec(&journal).unwrap().len() > 16384);
        let baseline = cfg.clone();
        let mut owner = Engine::new(cfg, 1_800_000_000_000).unwrap();
        let error = owner
            .admin(Command::PeerSet { peer }, 1_800_000_000_000)
            .err()
            .unwrap();
        assert_eq!(error.code, "security_record_too_large");
        assert!(owner.config.accepted_peers.is_empty());
        let status =
            serde_json::to_value(owner.admin(Command::Status {}, 1_800_000_000_000).unwrap())
                .unwrap();
        assert_eq!(status["security_storage"]["submitted_sequence"], 0);
        assert_eq!(status["security_storage"]["pending_records"], 0);
        let client = owner.config.clients[0].client_id.clone();
        assert!(
            owner
                .admin(
                    Command::ClientDisable {
                        client,
                        disabled: true
                    },
                    1_800_000_000_000
                )
                .is_ok()
        );
        let mut durable = false;
        for _ in 0..200 {
            let status =
                serde_json::to_value(owner.admin(Command::Status {}, 1_800_000_000_000).unwrap())
                    .unwrap();
            if status["security_storage"]["durable_sequence"] == 1 {
                durable = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            durable,
            "valid change blocked after rejected oversized change"
        );
        drop(owner);
        let mut recovered = Engine::new(baseline, 1_800_000_001_000).unwrap();
        let st = serde_json::to_value(
            recovered
                .admin(
                    Command::ClientGet {
                        client: recovered.config.clients[0].client_id.clone(),
                    },
                    1_800_000_001_000,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(st["disabled"], true);
        println!(
            "client disable remains effective after restart: {}",
            st["disabled"]
        );
        drop(recovered);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod transport_tests {
    use super::fixtures::*;
    use futures_util::{SinkExt, StreamExt};
    use mynofy_server::{
        admin::{self, Command},
        config::{Action, Client, PublicKey, Rule},
        crypto,
        protocol::{self, ProtobufMessage, pb},
        transport,
    };
    use tokio_tungstenite::tungstenite::Message;
    #[tokio::test]
    async fn revoke_acl_cancels_pending_poll_output() {
        cancel_output("acl").await;
    }
    #[tokio::test]
    async fn revoke_key_cancels_pending_poll_output() {
        cancel_output("key").await;
    }
    #[tokio::test]
    async fn disable_client_cancels_pending_poll_output() {
        cancel_output("client").await;
    }
    async fn cancel_output(mode: &str) {
        let (e, key, _) = setup();
        let mut cfg = e.config.clone();
        drop(e);
        let directory = std::env::temp_dir().join(format!(
            "mynofy-rv-{mode}-{}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
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
        let valid_until = protocol::timestamp(transport::now_ms() + 86_400_000).unwrap();
        for key in cfg.clients.iter_mut().flat_map(|client| &mut client.keys) {
            key.valid_until = valid_until.clone();
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
                valid_until,
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

        let command = match mode {
            "key" => Command::KeyRevoke {
                token: crypto::token_id(&public),
            },
            "client" => Command::ClientDisable {
                client,
                disabled: true,
            },
            _ => Command::AclSet {
                client,
                topic: "local/events".into(),
                allow: Vec::<Action>::new(),
            },
        };
        let revoked = admin::request(&admin_path, command).await.unwrap();
        assert_eq!(revoked["ok"], true);
        let response = tokio::time::timeout(std::time::Duration::from_secs(2), reader.next())
            .await
            .unwrap();
        assert!(
            !matches!(response, Some(Ok(Message::Binary(_)))),
            "revoked reader received a completed sensitive poll"
        );
        server.abort();
        let _ = server.await;
        drop(ws);
        drop(reader);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
