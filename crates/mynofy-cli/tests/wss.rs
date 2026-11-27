//! Real TLS + WebSocket tests, using only the already available OpenSSL CLI for test certificates.
mod support;
use mynofy_client as client;
use mynofy_protocol::crypto;
use mynofy_server::config::Config;
use serde_json::json;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
struct Fixture {
    dir: PathBuf,
    config: Config,
    key: Vec<u8>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn cert_command(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("openssl")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("OpenSSL CLI is required for TLS integration tests");
    assert!(
        output.status.success(),
        "openssl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn fixture() -> Fixture {
    let random = crypto::random::<16>().unwrap();
    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("mynofy-test-{suffix}"));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    cert_command(
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
            "/CN=test-ca",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
        ],
    );
    cert_command(
        &dir,
        &[
            "req",
            "-new",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
            "-subj",
            "/CN=localhost",
        ],
    );
    std::fs::write(dir.join("ext"),"basicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth\nkeyUsage=critical,digitalSignature\n").unwrap();
    cert_command(
        &dir,
        &[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.crt",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "server.crt",
            "-days",
            "1",
            "-extfile",
            "ext",
        ],
    );
    let key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&key).unwrap();
    let client = crypto::initial_client_id(&public);
    let mut cfg: serde_json::Value =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
    cfg["service"]["listen"] = "127.0.0.1:0".into();
    cfg["service"]["tls"]["certificate_file"] = dir.join("server.crt").to_str().unwrap().into();
    cfg["service"]["tls"]["private_key_file"] = dir.join("server.key").to_str().unwrap().into();
    cfg["service"]["tls"]["peer_ca_file"] = dir.join("ca.crt").to_str().unwrap().into();
    cfg["clients"] = json!([{"client_id":client,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":"2030-01-01T00:00:00Z"}]}]);
    cfg["topics"][0]["acl"] =
        json!([{"client_id":client,"allow":["publish","subscribe","inspect_own"]}]);
    Fixture {
        dir,
        config: serde_json::from_value(cfg).unwrap(),
        key,
    }
}
#[tokio::test]
async fn encrypted_publish_poll_rotation_and_reconnect() {
    let f = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = f.config.clone();
    let server = tokio::spawn(async move { support::run_wss_fixture(listener, config).await });
    let url = format!("wss://localhost:{}/", addr.port());
    let ca = f.dir.join("ca.crt");
    let mut ws = client::connect_client(&url, &ca, &f.key, "local", None)
        .await
        .unwrap();
    let catalog = client::get_catalog(&mut ws, "mynofy", 1).await.unwrap();
    assert_eq!(catalog["limits"]["max_opaque_payload_bytes"], 4096);
    let p=client::exchange(&mut ws,&json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":{"text":"hello"}})).await.unwrap();
    assert_eq!(p["type"], "result");
    let poll = client::exchange(
        &mut ws,
        &json!({"version":1,"type":"poll","id":"poll","topic":"local/events"}),
    )
    .await
    .unwrap();
    assert_eq!(poll["items"][0]["message"]["payload"]["text"], "hello");

    let _status = client::exchange(
        &mut ws,
        &json!({"version":1,"type":"get_message","id":"get","message_id":p["message_id"]}),
    )
    .await
    .unwrap();

    let new = crypto::generate_key().unwrap();
    let client = crypto::initial_client_id(&crypto::public_key(&f.key).unwrap());
    let result = client::rotate_client(&mut ws, &f.key, &new, "local", &client)
        .await
        .unwrap();
    assert_eq!(result["client_id"], client);
    ws.close(None).await.unwrap();
    assert!(
        client::connect_client(&url, &ca, &f.key, "local", Some(&client))
            .await
            .is_err()
    );
    let mut next = client::connect_client(&url, &ca, &new, "local", Some(&client))
        .await
        .unwrap();
    let empty=client::exchange(&mut next,&json!({"version":1,"type":"poll","id":"after","topic":"local/events","after":poll["next_cursor"]})).await.unwrap();
    assert_eq!(empty["items"], json!([]));
    next.close(None).await.unwrap();
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn rejects_unregistered_client_and_wrong_expected_owner() {
    let f = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = f.config.clone();
    let server = tokio::spawn(async move { support::run_wss_fixture(listener, config).await });
    let url = format!("wss://localhost:{}/", addr.port());
    let ca = f.dir.join("ca.crt");
    let unknown = crypto::generate_key().unwrap();
    assert!(
        client::connect_client(&url, &ca, &unknown, "local", None)
            .await
            .is_err()
    );
    assert!(
        client::connect_client(&url, &ca, &f.key, "wrong-owner", None)
            .await
            .is_err()
    );
    let other = fixture();
    assert!(
        client::connect_client(&url, &other.dir.join("ca.crt"), &f.key, "local", None)
            .await
            .is_err()
    );
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn application_frames_before_auth_and_idle_auth_connections_are_closed() {
    use futures_util::StreamExt;
    use std::{sync::Arc, time::Duration};
    use tokio_rustls::{TlsConnector, rustls::pki_types::ServerName};
    let mut f = fixture();
    f.config.limits.auth_timeout_seconds = 1;
    f.config.limits.max_payload_bytes = 1024;
    f.config.limits.max_response_bytes = 9216;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = f.config.clone();
    let server = tokio::spawn(async move { support::run_wss_fixture(listener, config).await });
    let url = format!("wss://localhost:{}/", address.port());
    let ca = f.dir.join("ca.crt");
    let tls = client::client_tls(&ca).unwrap();
    async fn open(
        address: std::net::SocketAddr,
        url: &str,
        tls: Arc<tokio_rustls::rustls::ClientConfig>,
    ) -> client::ClientSocket {
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
        let stream = TlsConnector::from(tls)
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .unwrap();
        tokio_tungstenite::client_async(url, stream)
            .await
            .unwrap()
            .0
    }
    let mut socket = open(address, &url, tls.clone()).await;
    let response = client::exchange(
        &mut socket,
        &json!({"version":1,"type":"poll","id":"unauth","topic":"local/events"}),
    )
    .await
    .unwrap();
    assert_eq!(response["code"], "unauthorized");
    drop(socket);
    let mut idle = open(address, &url, tls).await;
    let closed = tokio::time::timeout(Duration::from_secs(3), idle.next())
        .await
        .expect("Unauthenticated connection must time out");
    assert!(
        closed.is_none()
            || matches!(
                closed,
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
            )
    );
    server.abort();
    let _ = server.await;
}

#[cfg(unix)]
#[tokio::test]
async fn separate_admin_unix_socket_registers_a_client_on_the_wss_listener() {
    use mynofy_server::admin::{self, Command as A};
    let f = fixture();
    let path = f.dir.join("admin.sock");
    let mut config = f.config.clone();
    config.service.admin_socket = Some(path.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { support::run_wss_fixture(listener, config).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !support::unix_socket_ready(&path) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let key = crypto::generate_key().unwrap();
    let public = crypto::encode(&crypto::public_key(&key).unwrap());
    let added = admin::request(
        &path,
        A::ClientAdd {
            public_key: public,
            valid_until: None,
            display_name: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(added["ok"], true);
    let id = added["result"]["client_id"].as_str().unwrap();
    let grant = admin::request(
        &path,
        A::AclSet {
            client: id.into(),
            topic: "local/events".into(),
            allow: vec![mynofy_server::config::Action::Publish],
        },
    )
    .await
    .unwrap();
    assert_eq!(grant["ok"], true);
    let mut client = client::connect_client(
        &format!("wss://localhost:{}/", addr.port()),
        &f.dir.join("ca.crt"),
        &key,
        "local",
        Some(id),
    )
    .await
    .unwrap();
    let published=client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"admin-wss"})).await.unwrap();
    assert_eq!(published["type"], "result");
    server.abort();
    let _ = server.await;
    assert!(!path.exists());
}

#[tokio::test]
async fn wss_rejects_json_application_frames_before_and_after_authentication() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};
    let f = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(support::run_wss_fixture(listener, f.config.clone()));
    let url = format!("wss://localhost:{}/", address.port());
    let ca = f.dir.join("ca.crt");
    for authenticated in [false, true] {
        let mut socket = if authenticated {
            client::connect_client(&url, &ca, &f.key, "local", None)
                .await
                .unwrap()
        } else {
            let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
            let tls = tokio_rustls::TlsConnector::from(client::client_tls(&ca).unwrap())
                .connect(
                    tokio_rustls::rustls::pki_types::ServerName::try_from("localhost").unwrap(),
                    tcp,
                )
                .await
                .unwrap();
            tokio_tungstenite::client_async(&url, tls).await.unwrap().0
        };
        let frame = if authenticated {
            json!({"version":1,"type":"publish","id":"json-publish","topic":"local/events","payload":"must not publish"})
        } else {
            json!({"version":1,"type":"auth_hello","id":"json-hello","token_id":crypto::token_id(&crypto::public_key(&f.key).unwrap())})
        };
        socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Close(Some(close)) = reply else {
            panic!("JSON client frame must close the connection");
        };
        assert_eq!(close.code, CloseCode::Unsupported);
    }
    let mut healthy = client::connect_client(&url, &ca, &f.key, "local", None)
        .await
        .unwrap();
    let page = client::exchange(
        &mut healthy,
        &json!({"version":1,"type":"poll","id":"binary","topic":"local/events"}),
    )
    .await
    .unwrap();
    assert_eq!(page["items"], json!([]));
    server.abort();
    let _ = server.await;
}
