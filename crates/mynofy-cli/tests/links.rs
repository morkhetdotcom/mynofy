//! Real CA/mTLS service chains and owner-terminated client TLS, no mock authentication.
use mynofy_client as client;
use mynofy_protocol::ProtobufMessage as _;
use mynofy_protocol::crypto;
use mynofy_server::{config::Config, transport};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tokio::net::TcpListener;
struct Cluster {
    dir: PathBuf,
    configs: Vec<Config>,
    listeners: Vec<TcpListener>,
    key: Vec<u8>,
    second: Vec<u8>,
    local: Vec<u8>,
}
impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn openssl(dir: &Path, args: &[&str]) {
    let out = Command::new("openssl")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
async fn cluster(count: usize) -> Cluster {
    let dir = std::env::temp_dir().join(format!(
        "mynofy-links-{}",
        u64::from_le_bytes(crypto::random().unwrap())
    ));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    openssl(
        &dir,
        &[
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.crt",
            "-days",
            "1",
            "-subj",
            "/CN=links-ca",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
        ],
    );
    let names = if count == 2 {
        vec!["edge", "owner"]
    } else {
        vec!["edge", "hub", "owner"]
    };
    for name in &names {
        openssl(
            &dir,
            &[
                "req",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-nodes",
                "-keyout",
                &format!("{name}.key"),
                "-out",
                &format!("{name}.csr"),
                "-subj",
                &format!("/CN={name}"),
            ],
        );
        std::fs::write(dir.join("ext"),format!("basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:{name},DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth,clientAuth\nkeyUsage=critical,digitalSignature\n")).unwrap();
        openssl(
            &dir,
            &[
                "x509",
                "-req",
                "-in",
                &format!("{name}.csr"),
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-out",
                &format!("{name}.crt"),
                "-days",
                "1",
                "-extfile",
                "ext",
            ],
        );
    }
    let key = crypto::generate_key().unwrap();
    let second = crypto::generate_key().unwrap();
    let local = crypto::generate_key().unwrap();
    let identity = |key: &[u8]| {
        let public = crypto::public_key(key).unwrap();
        json!({"client_id":crypto::initial_client_id(&public),"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":"2030-01-01T00:00:00Z"}]})
    };
    let listeners: Vec<_> =
        futures_util::future::join_all(names.iter().map(|_| TcpListener::bind("127.0.0.1:0")))
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect();
    let mut configs = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let mut cfg: Value =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        cfg["service"] = json!({"id":name,"listen":listeners[i].local_addr().unwrap().to_string(),"tls":{"certificate_file":dir.join(format!("{name}.crt")),"private_key_file":dir.join(format!("{name}.key")),"peer_ca_file":dir.join("ca.crt")}});
        cfg["clients"] = if *name == "owner" {
            json!([identity(&key), identity(&second)])
        } else {
            json!([identity(&local)])
        };
        let grants:Vec<_>=cfg["clients"].as_array().unwrap().iter().map(|c|json!({"client_id":c["client_id"],"allow":["publish","subscribe","inspect_own"]})).collect();
        let policy = json!({"default_ttl_seconds":300,"max_ttl_seconds":3600});
        let local_topic =
            json!({"name":format!("{name}/events"),"owner_id":name,"policy":policy,"acl":grants});
        let shared = if *name == "owner" {
            json!({"name":"shared/events","owner_id":"owner","policy":policy,"acl":grants})
        } else {
            json!({"name":"shared/events","owner_id":"owner","via":names[i+1]})
        };
        cfg["topics"] = json!([local_topic, shared]);
        if i + 1 < names.len() {
            cfg["upstream"] = json!({"service_id":names[i+1],"url":format!("wss://{}/",listeners[i+1].local_addr().unwrap()),"trust":{"mode":"ca","ca_file":dir.join("ca.crt"),"expected_certificate_identity":names[i+1]}});
        }
        if i > 0 {
            cfg["accepted_peers"] = json!([{"service_id":names[i-1],"topics":["shared/events"]}]);
        }
        cfg["limits"]["client_burst"] = 100000.into();
        cfg["limits"]["client_requests_per_second"] = 100000.into();
        cfg["limits"]["source_auth_burst"] = 100.into();
        cfg["limits"]["max_connections"] = 128.into();
        cfg["limits"]["max_connections_per_source"] = 64.into();
        cfg["limits"]["request_timeout_seconds"] = 2.into();
        cfg["limits"]["auth_timeout_seconds"] = 2.into();
        let cfg: Config = serde_json::from_value(cfg).unwrap();
        cfg.validate().unwrap();
        configs.push(cfg);
    }
    Cluster {
        dir,
        configs,
        listeners,
        key,
        second,
        local,
    }
}
fn start(c: &mut Cluster) -> Vec<tokio::task::JoinHandle<Result<(), mynofy_server::BoxError>>> {
    let listeners = std::mem::take(&mut c.listeners);
    listeners
        .into_iter()
        .zip(c.configs.clone())
        .map(|(l, cfg)| tokio::spawn(async move { support::run_wss_fixture(l, cfg).await }))
        .collect()
}
fn url(c: &Cluster, index: usize) -> String {
    format!("wss://{}/", c.configs[index].service.listen.unwrap())
}
// Keep CA paths in caller scope; all nested TLS handshakes use the real owner's name.
async fn connect(
    c: &Cluster,
    key: &[u8],
) -> client::OwnerSocket<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    tokio::time::timeout(
        Duration::from_secs(10),
        client::connect_via_info(&url(c, 0), &ca, opts, key, None),
    )
    .await
    .unwrap()
    .unwrap()
    .0
}
async fn exchange<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    v: Value,
) -> Value {
    tokio::time::timeout(Duration::from_secs(5), client::exchange(socket, &v))
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn two_services_preserve_owner_auth_acl_pairs_and_parallel_channels() {
    let mut c = cluster(2).await;
    #[cfg(unix)]
    {
        c.configs[0].service.admin_socket = Some(c.dir.join("admin.sock"));
    }
    for config in &mut c.configs {
        config.limits.max_service_channels = 128;
    }
    let mut second_topic = c.configs[0].topics[1].clone();
    second_topic.name = "owner/events".into();
    c.configs[0].topics.push(second_topic);
    c.configs[1].accepted_peers[0]
        .topics
        .push("owner/events".into());
    let tasks = start(&mut c);
    let ca = c.dir.join("ca.crt");
    assert!(
        client::connect_client(&url(&c, 0), &ca, &c.key, "edge", None)
            .await
            .is_err()
    );
    let mut sockets = vec![connect(&c, &c.key).await];
    #[cfg(unix)]
    let original_route = link_status(&c).await["routes"]["shared/events"].clone();
    tokio::time::sleep(Duration::from_millis(10)).await;
    sockets.extend(futures_util::future::join_all((1..8).map(|_| connect(&c, &c.key))).await);
    #[cfg(unix)]
    assert_eq!(
        link_status(&c).await["routes"]["shared/events"],
        original_route
    );
    for (i, socket) in sockets.iter_mut().enumerate() {
        assert_eq!(exchange(socket,json!({"version":1,"type":"publish","id":"p","topic":"shared/events","payload":format!("job-{i}")})).await["type"],"result");
    }
    let page = exchange(
        &mut sockets[0],
        json!({"version":1,"type":"poll","id":"q","topic":"shared/events"}),
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 8);
    let producer = crypto::initial_client_id(&crypto::public_key(&c.key).unwrap());
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["message"]["producer_id"] == producer)
    );
    let other = connect(&c, &c.second).await;

    let permissions = exchange(
        &mut sockets[0],
        json!({"version":1,"type":"get_permissions","id":"r","topic":"shared/events"}),
    )
    .await;
    assert_eq!(permissions["owner_id"], "owner");
    let denied = exchange(
        &mut sockets[0],
        json!({"version":1,"type":"poll","id":"scope","topic":"owner/events"}),
    )
    .await;
    assert_eq!(denied["code"], "forbidden");
    let mut direct = client::connect_via_info(
        &url(&c, 0),
        &ca,
        client::ViaOptions {
            topic: "owner/events",
            owner: "owner",
            owner_certificate_identity: "owner",
            owner_ca: &ca,
        },
        &c.key,
        None,
    )
    .await
    .unwrap()
    .0;
    let allowed = exchange(
        &mut direct,
        json!({"version":1,"type":"poll","id":"scope","topic":"owner/events"}),
    )
    .await;
    assert_eq!(allowed["type"], "messages");
    drop(direct);
    let unknown = crypto::generate_key().unwrap();
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    assert!(
        client::connect_via_info(&url(&c, 0), &ca, opts, &unknown, None)
            .await
            .is_err()
    );
    #[cfg(unix)]
    {
        let snapshot = link_status(&c).await;
        assert_eq!(snapshot["registered_peers"], json!(["owner"]));
        assert!(snapshot["active_channels"].as_u64().unwrap() >= 8);
        assert_eq!(snapshot["routes"]["shared/events"]["next_hop"], "owner");
        assert_eq!(
            snapshot["routes"]["shared/events"]["path"],
            json!(["owner"])
        );
        drop(other);
        drop(sockets);
        wait_no_channels(&c).await;
    }
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}
#[tokio::test]
async fn three_services_forward_large_pages_and_keep_local_topics_after_owner_failure() {
    let mut c = cluster(3).await;
    let tasks = start(&mut c);
    let ca = c.dir.join("ca.crt");
    let mut socket = connect(&c, &c.key).await;
    let payload = crypto::encode(&vec![0x5a; 4096]);
    for _ in 0..48 {
        assert_eq!(exchange(&mut socket,json!({"version":1,"type":"publish","id":"p","topic":"shared/events","payload":{"kind":"bytes","data":payload}})).await["type"],"result");
    }
    let mut count = 0;
    let mut cursor = Value::Null;
    loop {
        let mut request =
            json!({"version":1,"type":"poll","id":"q","topic":"shared/events","limit":100});
        if !cursor.is_null() {
            request["after"] = cursor;
        }
        let page = exchange(&mut socket, request).await;
        assert_eq!(page["type"], "messages", "{page}");
        count += page["items"].as_array().unwrap().len();
        cursor = page["next_cursor"].clone();
        if page["has_more"] == false {
            break;
        }
    }
    assert_eq!(count, 48);
    {
        use futures_util::SinkExt;
        let mut stalled = connect(&c, &c.key).await;
        stalled
            .send(tokio_tungstenite::tungstenite::Message::Binary(
                mynofy_protocol::pb::Frame::from_json(json!({"version":1,"type":"poll","id":"slow","topic":"shared/events","limit":100})).unwrap().encode_to_vec().into(),
            ))
            .await
            .unwrap();
        assert_eq!(
            exchange(
                &mut socket,
                json!({"version":1,"type":"get_permissions","id":"healthy","topic":"shared/events"})
            )
            .await["owner_id"],
            "owner"
        );
        drop(stalled);
    }

    tasks.last().unwrap().abort();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            client::exchange(
                &mut socket,
                &json!({"version":1,"type":"poll","id":"q","topic":"shared/events"})
            )
        )
        .await
        .unwrap()
        .is_err()
    );
    let mut local = client::connect_client(&url(&c, 0), &ca, &c.local, "edge", None)
        .await
        .unwrap();
    assert_eq!(exchange(&mut local,json!({"version":1,"type":"publish","id":"p","topic":"edge/events","payload":"local remains"})).await["type"],"result");
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}
#[tokio::test]
async fn owner_certificate_and_peer_topic_allowlist_are_enforced() {
    let mut c = cluster(2).await;
    c.configs[1].accepted_peers[0].topics = vec!["owner/events".into()];
    let tasks = start(&mut c);
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    assert!(
        client::connect_via_info(&url(&c, 0), &ca, opts, &c.key, None)
            .await
            .is_err()
    );
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    let mut c = cluster(2).await;
    let tasks = start(&mut c);
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "edge",
        owner_ca: &ca,
    };
    assert!(
        client::connect_via_info(&url(&c, 0), &ca, opts, &c.key, None)
            .await
            .is_err()
    );
    let wrong_ca = c.dir.join("edge.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &wrong_ca,
    };
    assert!(
        client::connect_via_info(&url(&c, 0), &ca, opts, &c.key, None)
            .await
            .is_err()
    );
    // Rejected owner TLS must not poison the shared peer link.
    let mut socket = connect(&c, &c.key).await;
    assert_eq!(
        exchange(
            &mut socket,
            json!({"version":1,"type":"get_permissions","id":"q","topic":"shared/events"})
        )
        .await["owner_id"],
        "owner"
    );
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn cyclic_routes_fail_and_owner_rotation_keeps_stable_client_identity() {
    let mut c = cluster(2).await;
    c.configs[0].topics[1].owner_id = "missing".into();
    c.configs[1].topics[1].owner_id = "missing".into();
    c.configs[1].topics[1].policy = None;
    c.configs[1].topics[1].acl = None;
    c.configs[1].topics[1].via = Some("edge".into());
    c.configs[1].upstream = Some(mynofy_server::config::Upstream {
        service_id: "edge".into(),
        url: url(&c, 0),
        trust: mynofy_server::config::Trust {
            mode: "ca".into(),
            ca_file: c.dir.join("ca.crt"),
            expected_certificate_identity: "edge".into(),
        },
    });
    c.configs[0].accepted_peers = vec![mynofy_server::config::Peer {
        service_id: "owner".into(),
        certificate_identity: None,
        topics: vec!["shared/events".into()],
    }];
    let tasks = start(&mut c);
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "missing",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            client::connect_via_info(&url(&c, 0), &ca, opts, &c.key, None)
        )
        .await
        .unwrap()
        .is_err()
    );
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    let mut c = cluster(2).await;
    let tasks = start(&mut c);
    let mut socket = connect(&c, &c.key).await;
    let stable = crypto::initial_client_id(&crypto::public_key(&c.key).unwrap());
    let new = crypto::generate_key().unwrap();
    assert_eq!(
        client::rotate_client(&mut socket, &c.key, &new, "owner", &stable)
            .await
            .unwrap()["type"],
        "rotation_complete"
    );
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    let (mut socket, welcome) =
        client::connect_via_info(&url(&c, 0), &ca, opts, &new, Some(&stable))
            .await
            .unwrap();
    assert_eq!(welcome.client_id, stable);
    assert_eq!(exchange(&mut socket,json!({"version":1,"type":"publish","id":"p","topic":"shared/events","payload":"after rotation"})).await["type"],"result");
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

async fn peer_socket(
    c: &Cluster,
    cert: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    use std::sync::Arc;
    use tokio_rustls::rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject},
    };
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(c.dir.join("ca.crt")).unwrap() {
        roots.add(cert.unwrap()).unwrap();
    }
    let chain = CertificateDer::pem_file_iter(c.dir.join(format!("{cert}.crt")))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_root_certificates(roots)
    .with_client_auth_cert(
        chain,
        PrivateKeyDer::from_pem_file(c.dir.join(format!("{cert}.key"))).unwrap(),
    )
    .unwrap();
    let stream = tokio::net::TcpStream::connect(c.configs[1].service.listen.unwrap())
        .await
        .unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from("owner").unwrap(), stream)
        .await
        .unwrap();
    tokio_tungstenite::client_async(url(c, 1), tls)
        .await
        .unwrap()
        .0
}
#[tokio::test]
async fn mtls_peer_name_binding_and_duplicate_registration_are_enforced() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let mut c = cluster(2).await;
    let tasks = start(&mut c);
    let hello = || {
        Message::Text(
            json!({"version":1,"type":"service_hello","link_version":2,"id":"hello","service_id":"edge"})
                .to_string()
                .into(),
        )
    };
    let mut legacy = peer_socket(&c, "edge").await;
    legacy
        .send(Message::Text(
            json!({"version":1,"type":"service_hello","id":"legacy","service_id":"edge"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let Some(Ok(Message::Text(rejection))) = legacy.next().await else {
        panic!("missing version rejection")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&rejection).unwrap()["code"],
        "unsupported_version"
    );
    drop(legacy);
    let mut wrong = peer_socket(&c, "owner").await;
    wrong.send(hello()).await.unwrap();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(3), wrong.next())
            .await
            .unwrap(),
        Some(Ok(Message::Text(_)))
    ));
    let mut good = peer_socket(&c, "edge").await;
    good.send(hello()).await.unwrap();
    let Some(Ok(Message::Text(welcome))) = good.next().await else {
        panic!("missing welcome")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&welcome).unwrap()["service_id"],
        "owner"
    );
    let mut duplicate = peer_socket(&c, "edge").await;
    duplicate.send(hello()).await.unwrap();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(3), duplicate.next())
            .await
            .unwrap(),
        Some(Ok(Message::Text(_)))
    ));
    let query=Message::Text(json!({"version":1,"type":"route_query","id":"r","topic":"shared/events","visited":["edge"],"hop_limit":1}).to_string().into());
    good.send(query).await.unwrap();
    let Some(Ok(Message::Text(info))) = good.next().await else {
        panic!("missing route")
    };
    let info: Value = serde_json::from_str(&info).unwrap();
    assert_eq!(info["path"], json!(["owner"]));
    good.close(None).await.unwrap();
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(unix)]
mod support;
#[cfg(unix)]
#[tokio::test]
async fn cli_reaches_owner_through_a_unix_proxy() {
    if support::under_unprivileged_identity("cli_reaches_owner_through_a_unix_proxy") {
        return;
    }
    use tokio::io::AsyncWriteExt;
    let mut c = cluster(2).await;
    let mut listeners = std::mem::take(&mut c.listeners);
    let owner_listener = listeners.pop().unwrap();
    drop(listeners);
    let owner_cfg = c.configs[1].clone();
    let owner =
        tokio::spawn(async move { support::run_wss_fixture(owner_listener, owner_cfg).await });
    let path = c.dir.join("edge.sock");
    c.configs[0].service.listen = None;
    c.configs[0].service.unix_socket = Some(path.clone());
    c.configs[0].validate().unwrap();
    let bound = transport::bind_unix(&path).unwrap();
    let edge_cfg = c.configs[0].clone();
    let edge = tokio::spawn(async move { support::run_unix_fixture(bound, edge_cfg).await });
    let key = c.dir.join("client.pk8");
    std::fs::write(&key, &c.key).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args([
            "client",
            "--socket",
            path.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--owner",
            "owner",
            "--via-topic",
            "shared/events",
            "--owner-ca",
            c.dir.join("ca.crt").to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"{\"version\":1,\"type\":\"publish\",\"id\":\"p\",\"topic\":\"shared/events\",\"payload\":\"from CLI\"}\n{\"version\":1,\"type\":\"poll\",\"id\":\"q\",\"topic\":\"shared/events\"}\n").await.unwrap();
    drop(input);
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows[0]["type"], "result");
    assert_eq!(rows[1]["items"][0]["message"]["payload"], "from CLI");
    edge.abort();
    owner.abort();
    let _ = edge.await;
    let _ = owner.await;
}

#[cfg(unix)]
async fn link_status(c: &Cluster) -> Value {
    let value = mynofy_server::admin::request(
        c.configs[0].service.admin_socket.as_ref().unwrap(),
        mynofy_server::admin::Command::Status {},
    )
    .await
    .unwrap();
    assert_eq!(value["ok"], true, "{value}");
    value["result"]["service_links"].clone()
}
#[cfg(unix)]
async fn wait_no_channels(c: &Cluster) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if link_status(c).await["active_channels"] == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[cfg(unix)]
#[tokio::test]
async fn channel_capacity_is_released_after_disconnect_without_replaying_commands() {
    let mut c = cluster(2).await;
    c.configs[0].service.admin_socket = Some(c.dir.join("admin.sock"));
    c.configs[0].limits.max_service_channels = 1;
    c.configs[1].limits.max_service_channels = 1;
    let tasks = start(&mut c);
    let socket = connect(&c, &c.key).await;
    assert_eq!(link_status(&c).await["active_channels"], 1);
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    assert!(
        client::connect_via_info(&url(&c, 0), &ca, opts, &c.key, None)
            .await
            .is_err()
    );
    drop(socket);
    wait_no_channels(&c).await;
    let mut socket = connect(&c, &c.key).await;
    let page = exchange(
        &mut socket,
        json!({"version":1,"type":"poll","id":"p","topic":"shared/events"}),
    )
    .await;
    assert_eq!(page["items"], json!([]));
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn mcp_cli_uses_the_same_end_to_end_owner_tunnel() {
    if support::under_unprivileged_identity("mcp_cli_uses_the_same_end_to_end_owner_tunnel") {
        return;
    }
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut c = cluster(3).await;
    let tasks = start(&mut c);
    let key = c.dir.join("mcp-client.pk8");
    std::fs::write(&key, &c.key).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args([
            "mcp",
            "--url",
            &url(&c, 0),
            "--ca",
            c.dir.join("ca.crt").to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--owner",
            "owner",
            "--via-topic",
            "shared/events",
            "--owner-ca",
            c.dir.join("ca.crt").to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1"},"capabilities":{}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mynofy_publish","arguments":{"topic":"shared/events","payload":"MCP through three services"}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"mynofy_poll","arguments":{"topic":"shared/events"}}}),
    ];
    for request in requests {
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
        if request.get("id").is_none() {
            continue;
        }
        let line = tokio::time::timeout(Duration::from_secs(5), output.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], request["id"]);
        if request["id"] == 2 || request["id"] == 3 {
            assert_eq!(reply["result"]["isError"], false, "{reply}");
            assert_eq!(reply["result"]["structuredContent"]["owner_id"], "owner");
        }
        if request["id"] == 3 {
            assert_eq!(
                reply["result"]["structuredContent"]["response"]["items"][0]["message"]["payload"],
                "MCP through three services"
            );
        }
    }
    drop(input);
    let result = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(unix)]
async fn peer_admin(c: &Cluster, index: usize, command: mynofy_server::admin::Command) -> Value {
    let path = c.configs[index].service.admin_socket.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !support::unix_socket_ready(path) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reply = mynofy_server::admin::request(path, command).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}
#[cfg(unix)]
async fn peer_denied(c: &Cluster) {
    let ca = c.dir.join("ca.crt");
    let opts = client::ViaOptions {
        topic: "shared/events",
        owner: "owner",
        owner_certificate_identity: "owner",
        owner_ca: &ca,
    };
    assert!(
        tokio::time::timeout(
            Duration::from_secs(6),
            client::connect_via_info(&url(c, 0), &ca, opts, &c.key, None)
        )
        .await
        .unwrap()
        .is_err()
    );
}
#[cfg(unix)]
async fn wait_peers_gone(c: &Cluster) {
    use mynofy_server::admin::Command as A;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let mut empty = true;
            for i in 0..c.configs.len() {
                let status = peer_admin(c, i, A::Status {}).await;
                empty &= status["service_links"]["registered_peers"] == json!([])
                    && status["service_links"]["active_channels"] == 0;
            }
            if empty {
                break;
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    })
    .await
    .expect("peer and channel resources released");
}
#[cfg(unix)]
#[tokio::test]
async fn runtime_peer_management_updates_existing_links_without_restarting_local_topics() {
    if support::under_unprivileged_identity(
        "runtime_peer_management_updates_existing_links_without_restarting_local_topics",
    ) {
        return;
    }
    use mynofy_server::{admin::Command as A, config::Peer};
    let mut c = cluster(2).await;
    for i in 0..2 {
        c.configs[i].service.admin_socket = Some(c.dir.join(format!("a{i}.sock")));
    }
    // A configured CA must request peer certificates even before any peers are allowed.
    c.configs[1].accepted_peers.clear();
    let tasks = start(&mut c);
    peer_denied(&c).await;
    wait_peers_gone(&c).await;
    let cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args([
            "admin",
            "peers",
            "set",
            "edge",
            "--topics",
            "shared/events",
            "--socket",
            c.configs[1]
                .service
                .admin_socket
                .as_ref()
                .unwrap()
                .to_str()
                .unwrap(),
        ])
        .output()
        .await
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    // The previous rejected handshake starts the Edge reconnect cooldown.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut active = connect(&c, &c.key).await;
    let published=exchange(&mut active,json!({"version":1,"type":"publish","id":"before","topic":"shared/events","payload":"preserve"})).await;
    assert_eq!(published["type"], "result");
    peer_admin(
        &c,
        1,
        A::PeerDisable {
            service_id: "edge".into(),
            disabled: true,
        },
    )
    .await;
    assert!(
        tokio::time::timeout(
            Duration::from_secs(4),
            client::exchange(
                &mut active,
                &json!({"version":1,"type":"poll","id":"after-disable","topic":"shared/events"})
            )
        )
        .await
        .unwrap()
        .is_err()
    );
    wait_peers_gone(&c).await;
    peer_denied(&c).await;
    wait_peers_gone(&c).await;
    peer_admin(
        &c,
        1,
        A::PeerDisable {
            service_id: "edge".into(),
            disabled: false,
        },
    )
    .await;
    // The previous rejected handshake starts the Edge reconnect cooldown.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut active = connect(&c, &c.key).await;
    let page = exchange(
        &mut active,
        json!({"version":1,"type":"poll","id":"recovered","topic":"shared/events"}),
    )
    .await;
    assert_eq!(page["items"][0]["message"]["id"], published["message_id"]);
    peer_admin(
        &c,
        1,
        A::PeerSet {
            peer: Peer {
                service_id: "edge".into(),
                certificate_identity: None,
                topics: vec!["owner/events".into()],
            },
        },
    )
    .await;
    wait_peers_gone(&c).await;
    peer_denied(&c).await;
    // A valid peer link remains registered when only this topic is denied.
    peer_admin(
        &c,
        1,
        A::PeerSet {
            peer: Peer {
                service_id: "edge".into(),
                certificate_identity: None,
                topics: vec!["shared/events".into()],
            },
        },
    )
    .await;
    wait_peers_gone(&c).await;
    let active = connect(&c, &c.key).await;
    peer_admin(
        &c,
        1,
        A::PeerDelete {
            service_id: "edge".into(),
        },
    )
    .await;
    wait_peers_gone(&c).await;
    drop(active);
    peer_denied(&c).await;
    // Direct local clients are unaffected by removal of a service peer.
    let ca = c.dir.join("ca.crt");
    let mut local = client::connect_client(&url(&c, 1), &ca, &c.key, "owner", None)
        .await
        .unwrap();
    assert_eq!(
        exchange(
            &mut local,
            json!({"version":1,"type":"poll","id":"local","topic":"shared/events"})
        )
        .await["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn certificate_revocation_closes_incoming_and_outgoing_tunnels_and_rejects_reconnect() {
    if support::under_unprivileged_identity(
        "certificate_revocation_closes_incoming_and_outgoing_tunnels_and_rejects_reconnect",
    ) {
        return;
    }
    use mynofy_server::admin::{self, Command as A};
    use tokio_rustls::rustls::pki_types::{CertificateDer, pem::PemObject};
    for revoked_at in [0, 1] {
        let mut c = cluster(2).await;
        for i in 0..2 {
            c.configs[i].service.admin_socket = Some(c.dir.join(format!("a{i}.sock")));
        }
        let tasks = start(&mut c);
        let mut active = connect(&c, &c.key).await;
        // A policy notification for an unrelated certificate must not close this link.
        peer_admin(
            &c,
            revoked_at,
            A::PeerCertificateRevoke {
                fingerprint: admin::certificate_fingerprint(b"unrelated"),
            },
        )
        .await;
        assert_eq!(
            exchange(
                &mut active,
                json!({"version":1,"type":"poll","id":"still-live","topic":"shared/events"})
            )
            .await["items"],
            json!([])
        );
        let peer = if revoked_at == 0 { "owner" } else { "edge" };
        let cert = CertificateDer::pem_file_iter(c.dir.join(format!("{peer}.crt")))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let fingerprint = admin::certificate_fingerprint(cert.as_ref());
        let status = peer_admin(&c, revoked_at, A::Status {}).await;
        assert_eq!(
            status["service_links"]["registered_certificates"][peer],
            fingerprint
        );
        let cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args([
                "admin",
                "peers",
                "revoke-certificate",
                "--certificate",
                c.dir.join(format!("{peer}.crt")).to_str().unwrap(),
                "--socket",
                c.configs[revoked_at]
                    .service
                    .admin_socket
                    .as_ref()
                    .unwrap()
                    .to_str()
                    .unwrap(),
            ])
            .output()
            .await
            .unwrap();
        assert!(
            cli.status.success(),
            "{}",
            String::from_utf8_lossy(&cli.stderr)
        );
        assert!(
            tokio::time::timeout(
                Duration::from_secs(4),
                client::exchange(
                    &mut active,
                    &json!({"version":1,"type":"poll","id":"closed","topic":"shared/events"})
                )
            )
            .await
            .unwrap()
            .is_err()
        );
        wait_peers_gone(&c).await;
        peer_denied(&c).await;
        let revoked = peer_admin(
            &c,
            revoked_at,
            A::PeerCertificatesList {
                after: None,
                limit: Some(100),
            },
        )
        .await;
        assert!(
            revoked["items"]
                .as_array()
                .unwrap()
                .contains(&json!(fingerprint))
        );
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
    }
}

#[tokio::test]
async fn owner_tunnel_rejects_json_application_frames() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};
    let mut c = cluster(2).await;
    let tasks = start(&mut c);
    let mut socket = connect(&c, &c.key).await;
    socket.send(Message::Text(json!({"version":1,"type":"publish","id":"json-tunnel","topic":"shared/events","payload":"must not publish"}).to_string().into())).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    match reply {
        Some(Ok(Message::Close(Some(close)))) => assert_eq!(close.code, CloseCode::Unsupported),
        None | Some(Err(_)) | Some(Ok(Message::Close(None))) => {} // Closing the owner channel can terminate inner TLS.
        other => panic!("Owner must reject tunneled JSON commands: {other:?}"),
    }
    drop(socket);
    let mut healthy = connect(&c, &c.key).await;
    let page = exchange(
        &mut healthy,
        json!({"version":1,"type":"poll","id":"binary","topic":"shared/events"}),
    )
    .await;
    assert_eq!(page["items"], json!([]));
    drop(healthy);
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn pending_owner_tunnels_have_their_own_quota() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{client_async, tungstenite::Message};

    let mut c = cluster(3).await;
    c.configs[0].upstream = c.configs[1].upstream.clone();
    c.configs[0]
        .topics
        .iter_mut()
        .find(|topic| topic.name == "shared/events")
        .unwrap()
        .via = Some("owner".into());
    let mut edge_grant = c.configs[2].accepted_peers[0].clone();
    edge_grant.service_id = "edge".into();
    c.configs[2].accepted_peers.push(edge_grant);
    for config in &mut c.configs {
        config.limits.max_service_channels = 8;
    }
    c.configs[2].limits.max_service_channels = 16;
    let tasks = start(&mut c);
    let mut established = connect(&c, &c.key).await;
    async fn open_pending(
        c: &Cluster,
        edge: usize,
        ip: std::net::Ipv4Addr,
    ) -> (
        tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
        bool,
    ) {
        let ca = c.dir.join("ca.crt");
        let tcp = tokio::net::TcpSocket::new_v4().unwrap();
        tcp.bind((ip, 0).into()).unwrap();
        let tcp = tcp
            .connect(c.configs[edge].service.listen.unwrap())
            .await
            .unwrap();
        let tls = tokio_rustls::TlsConnector::from(client::client_tls(&ca).unwrap())
            .connect(
                tokio_rustls::rustls::pki_types::ServerName::try_from(
                    c.configs[edge].service.id.clone(),
                )
                .unwrap(),
                tcp,
            )
            .await
            .unwrap();
        let (mut socket, _) = client_async(url(c, edge), tls).await.unwrap();
        socket.send(Message::Text(
            json!({"version":1,"type":"client_tunnel","id":"pending","topic":"shared/events","owner_id":"owner"})
                .to_string().into(),
        )).await.unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap();
        let ready = matches!(response, Some(Ok(Message::Text(ref text))) if text.contains("client_tunnel_ready"));
        (socket, ready)
    }
    let local = std::net::Ipv4Addr::LOCALHOST;
    let (pending, ready) = open_pending(&c, 0, local).await;
    assert!(ready);
    let (_, ready) = open_pending(&c, 0, local).await;
    assert!(
        !ready,
        "a second pending tunnel must exceed the source quota"
    );
    #[cfg(target_os = "linux")]
    let second_source = {
        let (pending, ready) = open_pending(&c, 0, std::net::Ipv4Addr::new(127, 0, 0, 2)).await;
        assert!(ready, "an independent source has a separate edge slot");
        let (_, ready) = open_pending(&c, 0, std::net::Ipv4Addr::new(127, 0, 0, 3)).await;
        assert!(
            !ready,
            "a third source still shares its owner's per-peer quota"
        );
        pending
    };
    let (independent_peer, ready) = open_pending(&c, 1, local).await;
    assert!(ready, "an independent peer must retain an admission slot");
    let reply = exchange(
        &mut established,
        json!({"version":1,"type":"poll","id":"still-live","topic":"shared/events"}),
    )
    .await;
    assert_eq!(reply["type"], "messages");
    drop(pending);
    drop(independent_peer);
    #[cfg(target_os = "linux")]
    drop(second_source);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (_, ready) = open_pending(&c, 0, local).await;
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn stalled_upstream_attempt_is_shared_and_does_not_block_local_client() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut c = cluster(2).await;
    c.configs[0].limits.request_timeout_seconds = 1;
    #[cfg(unix)]
    {
        c.configs[0].service.admin_socket = Some(c.dir.join("admin.sock"));
    }
    let listener = c.listeners.pop().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let observe = accepted.clone();
    let stalled = tokio::spawn(async move {
        let mut connections = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            observe.fetch_add(1, Ordering::Relaxed);
            connections.push(socket);
        }
    });
    let tasks = start(&mut c);
    let ca = c.dir.join("ca.crt");
    let edge = url(&c, 0);
    let requests = futures_util::future::join_all((0..4).map(|_| {
        client::connect_via_info(
            &edge,
            &ca,
            client::ViaOptions {
                topic: "shared/events",
                owner: "owner",
                owner_certificate_identity: "owner",
                owner_ca: &ca,
            },
            &c.key,
            None,
        )
    }));
    let local = async {
        let mut socket = client::connect_client(&edge, &ca, &c.local, "edge", None)
            .await
            .unwrap();
        let reply = exchange(
            &mut socket,
            json!({"version":1,"type":"poll","id":"local","topic":"edge/events"}),
        )
        .await;
        assert_eq!(reply["type"], "messages");
    };
    let (results, ()) = tokio::time::timeout(Duration::from_millis(1700), async {
        tokio::join!(requests, local)
    })
    .await
    .unwrap();
    assert!(results.iter().all(Result::is_err));
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    #[cfg(unix)]
    {
        let status = peer_admin(&c, 0, mynofy_server::admin::Command::Status {}).await;
        let links = &status["service_links"];
        assert_eq!(links["upstream_service_id"], "owner");
        assert_eq!(links["upstream_connected"], false);
        assert_eq!(links["upstream_connecting"], false);
        assert_eq!(links["upstream_last_error"]["phase"], "tls");
        assert!(links["upstream_last_error"]["at_ms"].is_u64());
    }
    stalled.abort();
    for task in tasks {
        task.abort();
    }
}
