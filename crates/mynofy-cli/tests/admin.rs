#![cfg(unix)]
mod support;
use mynofy_protocol::crypto;
use mynofy_server::{
    admin::{self, Command as A},
    config::Config,
    transport,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
struct Fixture {
    dir: PathBuf,
    data: PathBuf,
    admin: PathBuf,
    config: Config,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "mna-{:016x}",
        u64::from_le_bytes(crypto::random().unwrap())
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let data = dir.join("d.sock");
    let admin = dir.join("a.sock");
    let key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&key).unwrap();
    let client = crypto::initial_client_id(&public);
    let mut cfg: Value =
        serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
    cfg["service"] = json!({"id":"local","unix_socket":data,"admin_socket":admin});
    cfg["clients"] = json!([{"client_id":client,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":"2030-01-01T00:00:00Z"}]}]);
    cfg["topics"][0]["acl"] =
        json!([{"client_id":client,"allow":["publish","subscribe","inspect_own"]}]);
    let config = serde_json::from_value(cfg).unwrap();
    Fixture {
        dir,
        data,
        admin,
        config,
    }
}
async fn start(
    f: &Fixture,
    config: Config,
) -> tokio::task::JoinHandle<Result<(), mynofy_server::BoxError>> {
    let bound = transport::bind_unix(&f.data).unwrap();
    let task = tokio::spawn(async move { transport::run_unix(bound, config).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !f.admin.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task
}
async fn call(f: &Fixture, command: A) -> Value {
    let mut reply = admin::request(&f.admin, command).await.unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].take()
}
async fn raw(f: &Fixture, request: &[u8]) -> Value {
    let mut stream = UnixStream::connect(&f.admin).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    serde_json::from_slice(&response).unwrap()
}
async fn cli(args: &[&str]) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(args)
        .output()
        .await
        .unwrap()
}
#[tokio::test]
async fn admin_socket_registers_live_clients_revokes_keys_and_exports_a_restartable_snapshot() {
    if support::under_unprivileged_identity(
        "admin_socket_registers_live_clients_revokes_keys_and_exports_a_restartable_snapshot",
    ) {
        return;
    }
    let f = fixture();
    let server = start(&f, f.config.clone()).await;
    assert_eq!(std::fs::metadata(&f.admin).unwrap().mode() & 0o777, 0o600);
    let key = crypto::generate_key().unwrap();
    let public = crypto::encode(&crypto::public_key(&key).unwrap());
    let added = call(
        &f,
        A::ClientAdd {
            public_key: public,
            valid_until: None,
            display_name: Some("runtime-agent".into()),
        },
    )
    .await;
    let id = added["client_id"].as_str().unwrap().to_owned();
    let mut client = mynofy_client::connect_unix_client(&f.data, &key, "local", None)
        .await
        .unwrap();
    let poll = json!({"version":1,"type":"poll","id":"p","topic":"local/events"});
    assert_eq!(
        mynofy_client::exchange(&mut client, &poll).await.unwrap()["code"],
        "forbidden"
    );
    call(
        &f,
        A::AclSet {
            client: id.clone(),
            topic: "local/events".into(),
            allow: vec![
                mynofy_server::config::Action::Publish,
                mynofy_server::config::Action::Subscribe,
            ],
        },
    )
    .await;
    // Existing sessions are invalidated, then a new client can use the same data listener.
    assert!(mynofy_client::exchange(&mut client, &poll).await.is_err());
    let mut client = mynofy_client::connect_unix_client(&f.data, &key, "local", Some(&id))
        .await
        .unwrap();
    let published = mynofy_client::exchange(
        &mut client,
        &json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"runtime"}),
    )
    .await
    .unwrap();
    assert_eq!(published["type"], "result");
    let _page = mynofy_client::exchange(&mut client, &poll).await.unwrap();
    call(
        &f,
        A::ClientDisable {
            client: id.clone(),
            disabled: true,
        },
    )
    .await;
    assert!(
        mynofy_client::connect_unix_client(&f.data, &key, "local", Some(&id))
            .await
            .is_err()
    );
    call(
        &f,
        A::ClientDisable {
            client: id.clone(),
            disabled: false,
        },
    )
    .await;
    let _client = mynofy_client::connect_unix_client(&f.data, &key, "local", Some(&id))
        .await
        .unwrap();

    let token = crypto::token_id(&crypto::public_key(&key).unwrap());
    call(&f, A::KeyRevoke { token }).await;
    assert!(
        mynofy_client::connect_unix_client(&f.data, &key, "local", Some(&id))
            .await
            .is_err()
    );
    let new_key = crypto::generate_key().unwrap();
    let new_public = crypto::encode(&crypto::public_key(&new_key).unwrap());
    call(
        &f,
        A::KeyAdd {
            client: id.clone(),
            public_key: new_public,
            valid_until: None,
        },
    )
    .await;
    let public_file = f.dir.join("public.json");
    std::fs::write(&public_file,json!({"public_key":crypto::encode(&crypto::public_key(&crypto::generate_key().unwrap()).unwrap())}).to_string()).unwrap();
    let result = cli(&[
        "admin",
        "clients",
        "add",
        "--socket",
        f.admin.to_str().unwrap(),
        "--public-key",
        public_file.to_str().unwrap(),
    ])
    .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let snapshot = f.dir.join("snapshot.json");
    let args = [
        "admin",
        "export",
        "--socket",
        f.admin.to_str().unwrap(),
        "--out",
        snapshot.to_str().unwrap(),
    ];
    let result = cli(&args).await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::metadata(&snapshot).unwrap().mode() & 0o777, 0o600);
    assert!(!cli(&args).await.status.success());
    let restored = Config::load(&snapshot).unwrap();
    assert_eq!(restored.clients.len(), 3);
    let yaml = f.dir.join("snapshot.yaml");
    let result = cli(&[
        "admin",
        "export",
        "--socket",
        f.admin.to_str().unwrap(),
        "--out",
        yaml.to_str().unwrap(),
    ])
    .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !std::fs::read_to_string(&yaml)
            .unwrap()
            .trim_start()
            .starts_with('{')
    );
    let yaml_config = Config::load(&yaml).unwrap();
    assert_eq!(
        serde_json::to_value(&restored.clients).unwrap(),
        serde_json::to_value(yaml_config.clients).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&restored.topics).unwrap(),
        serde_json::to_value(yaml_config.topics).unwrap()
    );
    assert_eq!(std::fs::metadata(&yaml).unwrap().mode() & 0o777, 0o600);
    server.abort();
    let _ = server.await;
    assert!(!f.admin.exists());
    assert!(!f.data.exists());
    let server = start(&f, restored).await;
    let mut client = mynofy_client::connect_unix_client(&f.data, &new_key, "local", Some(&id))
        .await
        .unwrap();
    assert_eq!(
        mynofy_client::exchange(&mut client, &poll).await.unwrap()["type"],
        "messages"
    );
    assert!(
        mynofy_client::connect_unix_client(&f.data, &key, "local", Some(&id))
            .await
            .is_err()
    );
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn admin_rejects_malformed_duplicate_oversized_and_message_protocol_requests() {
    let f = fixture();
    let server = start(&f, f.config.clone()).await;
    for bytes in [
        b"{\"version\":1,\"command\":{\"op\":\"status\",\"unexpected\":1}}\n".as_slice(),
        b"{\"version\":1,\"version\":1,\"command\":{\"op\":\"status\"}}\n",
        b"{\"version\":2,\"command\":{\"op\":\"status\"}}\n",
        b"{\"version\":1,\"type\":\"publish\",\"topic\":\"local/events\"}\n",
    ] {
        assert_eq!(raw(&f, bytes).await["error"]["code"], "invalid_request");
    }
    let oversized = vec![b' '; admin::MAX_REQUEST_BYTES + 1];
    assert_eq!(
        raw(&f, &oversized).await["error"]["code"],
        "invalid_request"
    );
    let status = call(&f, A::Status {}).await;
    assert_eq!(status["clients"], 1);
    assert_eq!(status["storage"]["resident_records"], 0);
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn admin_peer_uid_check_survives_relaxed_test_socket_permissions() {
    let f = fixture();
    let server = start(&f, f.config.clone()).await;
    if std::fs::metadata(&f.admin).unwrap().uid() == 0 {
        use std::os::unix::process::CommandExt;
        std::fs::set_permissions(&f.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&f.admin, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(admin::request(&f.admin, A::Status {}).await.is_err());
        let mut command = tokio::process::Command::new("python3");
        command.args(["-c","import socket,sys\ns=socket.socket(socket.AF_UNIX);s.settimeout(2);s.connect(sys.argv[1])\ntry:\n s.sendall(b'{\"version\":1,\"command\":{\"op\":\"status\"}}\\n'); data=s.recv(100)\nexcept (ConnectionResetError,BrokenPipeError): data=b''\nsys.exit(0 if not data else 1)",f.admin.to_str().unwrap()]);
        command.as_std_mut().gid(65534).uid(65534);
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), command.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::set_permissions(&f.admin, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(call(&f, A::Status {}).await["service_id"], "local");
    server.abort();
    let _ = server.await;
}
#[tokio::test]
async fn admin_startup_preserves_existing_files_and_socket_replacements() {
    let f = fixture();
    std::fs::write(&f.admin, b"keep").unwrap();
    let bound = transport::bind_unix(&f.data).unwrap();
    assert!(transport::run_unix(bound, f.config.clone()).await.is_err());
    assert_eq!(std::fs::read(&f.admin).unwrap(), b"keep");
    assert!(!f.data.exists());
    std::fs::remove_file(&f.admin).unwrap();
    let server = start(&f, f.config.clone()).await;
    std::fs::remove_file(&f.admin).unwrap();
    std::fs::write(&f.admin, b"replacement").unwrap();
    server.abort();
    let _ = server.await;
    assert_eq!(std::fs::read(&f.admin).unwrap(), b"replacement");
}

#[test]
fn admin_configuration_resolves_relative_paths_and_rejects_shared_endpoint() {
    let f = fixture();
    let mut config = serde_json::to_value(&f.config).unwrap();
    config["service"]["unix_socket"] = "run/data.sock".into();
    config["service"]["admin_socket"] = "run/admin.sock".into();
    config["security_storage"] = json!({"directory":"security-data"});
    let path = f.dir.join("config.json");
    std::fs::write(&path, config.to_string()).unwrap();
    let loaded = Config::load(&path).unwrap();
    let storage = loaded.security_storage.as_ref().unwrap();
    let canonical_dir = f.dir.canonicalize().unwrap();
    assert_eq!(storage.directory, canonical_dir.join("security-data"));
    assert_eq!(storage.queue_capacity, 128);
    assert_eq!(storage.compact_after_records, 4096);
    assert!(
        !storage.directory.exists(),
        "Loading config must not create storage"
    );
    assert_eq!(
        loaded.service.admin_socket,
        Some(canonical_dir.join("run/admin.sock"))
    );
    // Exercise aliased temporary directories on Linux too (/tmp is an alias on macOS).
    let alias = f.dir.join("alias");
    std::os::unix::fs::symlink(&f.dir, &alias).unwrap();
    let aliased = Config::load(&alias.join("config.json")).unwrap();
    assert_eq!(
        aliased.security_storage.unwrap().directory,
        storage.directory
    );
    assert_eq!(aliased.service.admin_socket, loaded.service.admin_socket);
    config["service"]["admin_socket"] = "run/data.sock".into();
    assert!(
        serde_json::from_value::<Config>(config)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[tokio::test]
async fn admin_cli_routes_client_key_acl_and_status_commands() {
    if support::under_unprivileged_identity("admin_cli_routes_client_key_acl_and_status_commands") {
        return;
    }
    let f = fixture();
    let config_file = f.dir.join("config.yaml");
    std::fs::write(&config_file, f.config.to_yaml().unwrap()).unwrap();
    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["serve", "--config", config_file.to_str().unwrap()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !f.admin.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let socket = f.admin.to_str().unwrap();
    let key = f.dir.join("client.pk8");
    let public = f.dir.join("public.json");
    let generated = cli(&["keygen", "--out", key.to_str().unwrap()]).await;
    assert!(generated.status.success());
    std::fs::write(&public, &generated.stdout).unwrap();
    let identity: Value = serde_json::from_slice(&generated.stdout).unwrap();
    let id = identity["client_id"].as_str().unwrap();
    let token = identity["token_id"].as_str().unwrap();
    for args in [
        vec![
            "admin",
            "clients",
            "add",
            "--socket",
            socket,
            "--public-key",
            public.to_str().unwrap(),
            "--name",
            "cli-agent",
        ],
        vec!["admin", "clients", "get", id, "--socket", socket],
        vec![
            "admin", "clients", "list", "--socket", socket, "--after", "0", "--limit", "1",
        ],
        vec![
            "admin",
            "acl",
            "set",
            id,
            "local/events",
            "--allow",
            "publish",
            "--socket",
            socket,
        ],
        vec![
            "admin",
            "acl",
            "get",
            id,
            "local/events",
            "--socket",
            socket,
        ],
        vec!["admin", "clients", "disable", id, "--socket", socket],
        vec!["admin", "clients", "enable", id, "--socket", socket],
        vec!["admin", "keys", "revoke", token, "--socket", socket],
        vec![
            "admin",
            "keys",
            "add",
            id,
            "--public-key",
            public.to_str().unwrap(),
            "--valid-until",
            "2030-01-01T00:00:00Z",
            "--socket",
            socket,
        ],
        vec!["admin", "status", "--socket", socket],
    ] {
        let output = cli(&args).await;
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _: Value = serde_json::from_slice(&output.stdout).unwrap();
    }
    assert!(
        !cli(&[
            "admin",
            "clients",
            "disable",
            id,
            "--socket",
            socket,
            "--unexpected",
            "1"
        ])
        .await
        .status
        .success()
    );
    assert!(
        !cli(&[
            "admin",
            "acl",
            "set",
            id,
            "local/events",
            "--allow",
            "bogus",
            "--socket",
            socket
        ])
        .await
        .status
        .success()
    );
    assert_eq!(
        call(&f, A::ClientGet { client: id.into() }).await["disabled"],
        false
    );
    assert_eq!(
        call(
            &f,
            A::AclGet {
                client: id.into(),
                topic: "local/events".into()
            }
        )
        .await["allow"],
        json!(["publish"])
    );
    let result = tokio::process::Command::new("kill")
        .args(["-TERM", &server.id().unwrap().to_string()])
        .status()
        .await
        .unwrap();
    assert!(result.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(2), server.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    assert!(!f.admin.exists());
    assert!(!f.data.exists());
}

#[tokio::test]
async fn security_journal_recovers_after_sigkill_while_messages_stay_in_ram() {
    if support::under_unprivileged_identity(
        "security_journal_recovers_after_sigkill_while_messages_stay_in_ram",
    ) {
        return;
    }
    let f = fixture();
    let config = f.dir.join("config.json");
    std::fs::write(&config, serde_json::to_vec(&f.config).unwrap()).unwrap();
    let stderr = f.dir.join("server.stderr");
    let launch = || {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["serve", "--config"])
            .arg(&config)
            .stdout(std::process::Stdio::null())
            .stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&stderr)
                    .unwrap(),
            )
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    };
    async fn observed_status(
        f: &Fixture,
        child: &mut tokio::process::Child,
        phase: &str,
        ready: bool,
        mut predicate: impl FnMut(&Value) -> bool,
    ) {
        let mut last = String::from("no status received");
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Some(exit) = child.try_wait().unwrap() {
                    return Err(format!("server exited: {exit}"));
                }
                match admin::request(&f.admin, A::Status {}).await {
                    Ok(reply) => {
                        if reply["ok"] != true {
                            return Err(format!("status rejected: {reply}"));
                        }
                        last = reply.to_string();
                        if predicate(&reply["result"]) {
                            return Ok(());
                        }
                    }
                    // Only startup may precede bind/listen. Never retry mutations or
                    // hide resets, rate-limit rejections or malformed responses.
                    Err(error)
                        if ready
                            && error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                                matches!(
                                    e.kind(),
                                    std::io::ErrorKind::NotFound
                                        | std::io::ErrorKind::ConnectionRefused
                                )
                            }) =>
                    {
                        last = error.to_string();
                    }
                    Err(error) => return Err(format!("status failed: {error}")),
                }
                // Admin admission refills at 10/s (burst 20). Stay below that
                // sustained rate even when journal fsync takes several seconds.
                tokio::time::sleep(std::time::Duration::from_millis(125)).await;
            }
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!(
                "{phase}: {result:?}; last observation: {last}; server stderr:\n{}",
                std::fs::read_to_string(f.dir.join("server.stderr")).unwrap_or_default()
            );
        }
    }
    let mut server = launch();
    observed_status(&f, &mut server, "initial readiness", true, |_| true).await;
    let key = crypto::generate_key().unwrap();
    let public = crypto::encode(&crypto::public_key(&key).unwrap());
    let added = call(
        &f,
        A::ClientAdd {
            public_key: public,
            valid_until: None,
            display_name: Some("survives-crash".into()),
        },
    )
    .await;
    let id = added["client_id"].as_str().unwrap().to_owned();
    call(
        &f,
        A::AclSet {
            client: id.clone(),
            topic: "local/events".into(),
            allow: vec![
                mynofy_server::config::Action::Publish,
                mynofy_server::config::Action::Subscribe,
            ],
        },
    )
    .await;
    let mut client = mynofy_client::connect_unix_client(&f.data, &key, "local", None)
        .await
        .unwrap();
    let published=mynofy_client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"volatile crash test"})).await.unwrap();
    assert_eq!(published["type"], "result");
    drop(client);
    // Observe beyond the listener's initial 20-token burst even on fast disks.
    // This exercises sustained monitoring instead of depending on slow fsync.
    let mut observations = 0;
    observed_status(
        &f,
        &mut server,
        "journal durability before SIGKILL",
        false,
        |status| {
            observations += 1;
            observations >= 24 && status["security_storage"]["durable_sequence"] == 2
        },
    )
    .await;
    let storage = f.dir.join("config.json.security");
    assert!(std::fs::metadata(storage.join("journal")).unwrap().len() > 0);
    server.kill().await.unwrap();
    server.wait().await.unwrap();
    // A hard kill leaves socket files. Remove them only after this exact child has exited.
    std::fs::remove_file(&f.data).unwrap();
    std::fs::remove_file(&f.admin).unwrap();
    let mut server = launch();
    observed_status(&f, &mut server, "recovery readiness", true, |_| true).await;
    assert_eq!(
        call(&f, A::ClientGet { client: id.clone() }).await["display_name"],
        "survives-crash"
    );
    let status = call(&f, A::Status {}).await;
    assert_eq!(status["security_storage"]["durable_sequence"], 2);
    assert_eq!(status["security_storage"]["snapshot_sequence"], 2);
    assert_eq!(status["storage"]["resident_records"], 0);
    let mut client = mynofy_client::connect_unix_client(&f.data, &key, "local", Some(&id))
        .await
        .unwrap();
    let page = mynofy_client::exchange(
        &mut client,
        &json!({"version":1,"type":"poll","id":"p","topic":"local/events"}),
    )
    .await
    .unwrap();
    assert_eq!(page["items"], json!([]));
    drop(client);
    server.kill().await.unwrap();
    server.wait().await.unwrap();
}

#[tokio::test]
async fn admin_cli_manages_topics_with_live_data_acl_and_export() {
    if support::under_unprivileged_identity(
        "admin_cli_manages_topics_with_live_data_acl_and_export",
    ) {
        return;
    }
    let f = fixture();
    let server = start(&f, f.config.clone()).await;
    let socket = f.admin.to_str().unwrap();
    let created = cli(&[
        "admin",
        "topics",
        "create",
        "runtime/jobs",
        "--default-ttl",
        "30",
        "--max-ttl",
        "60",
        "--socket",
        socket,
    ])
    .await;
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let item: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(item["storage"]["mode"], "ram");
    let changed = cli(&[
        "admin",
        "topics",
        "policy",
        "runtime/jobs",
        "--default-ttl",
        "10",
        "--max-ttl",
        "20",
        "--socket",
        socket,
    ])
    .await;
    assert!(changed.status.success());
    let get = cli(&["admin", "topics", "get", "runtime/jobs", "--socket", socket]).await;
    assert_eq!(
        serde_json::from_slice::<Value>(&get.stdout).unwrap()["policy"]["default_ttl_seconds"],
        10
    );
    let invalid = cli(&[
        "admin",
        "topics",
        "policy",
        "runtime/jobs",
        "--default-ttl",
        "30",
        "--max-ttl",
        "20",
        "--socket",
        socket,
    ])
    .await;
    assert!(!invalid.status.success());
    let list = cli(&[
        "admin", "topics", "list", "--limit", "1", "--socket", socket,
    ])
    .await;
    assert!(list.status.success());
    let list: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["next_after"], "local/events");
    let more = cli(&[
        "admin",
        "topics",
        "list",
        "--after",
        "local/events",
        "--socket",
        socket,
    ])
    .await;
    assert_eq!(
        serde_json::from_slice::<Value>(&more.stdout).unwrap()["items"][0]["topic"],
        "runtime/jobs"
    );
    let client = f.config.clients[0].client_id.clone();
    call(
        &f,
        A::AclSet {
            client: client.clone(),
            topic: "runtime/jobs".into(),
            allow: vec![mynofy_server::config::Action::Publish],
        },
    )
    .await;
    let export = call(&f, A::Export {}).await;
    let topic = export["topics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "runtime/jobs")
        .unwrap();
    assert_eq!(topic["acl"][0]["client_id"], client);
    let deleted = cli(&[
        "admin",
        "topics",
        "delete",
        "runtime/jobs",
        "--socket",
        socket,
    ])
    .await;
    assert!(
        deleted.status.success(),
        "{}",
        String::from_utf8_lossy(&deleted.stderr)
    );
    let get = cli(&["admin", "topics", "get", "runtime/jobs", "--socket", socket]).await;
    assert!(!get.status.success());
    call(
        &f,
        A::TopicCreate {
            topic: "runtime/jobs".into(),
            policy: mynofy_server::config::Policy {
                default_ttl_seconds: 10,
                max_ttl_seconds: 20,
            },
        },
    )
    .await;
    assert_eq!(
        call(
            &f,
            A::AclGet {
                client,
                topic: "runtime/jobs".into()
            }
        )
        .await["allow"],
        json!([])
    );
    server.abort();
    let _ = server.await;
}
