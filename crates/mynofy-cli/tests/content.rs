#![cfg(unix)]
mod support;
use mynofy_protocol::crypto;
use mynofy_server::{
    admin::{self, Command as A},
    config::{Action, Config, ContentEncryption, ContentStorage, SecurityStorage},
    transport::now_ms,
};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;
struct Fixture {
    dir: PathBuf,
    config: PathBuf,
    cfg: Config,
    key: Vec<u8>,
    id: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn fixture(mode: ContentEncryption) -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "mnc-{:016x}",
        u64::from_le_bytes(crypto::random().unwrap())
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&key).unwrap();
    let id = crypto::initial_client_id(&public);
    let mut cfg: Value =
        serde_json::from_str(include_str!("../../../config/scenarios/unix.json")).unwrap();
    cfg["service"] = json!({"id":"local","unix_socket":dir.join("data.sock"),"admin_socket":dir.join("admin.sock")});
    cfg["clients"] = json!([{"client_id":id,"keys":[{"algorithm":"ed25519","public_key":crypto::encode(&public),"valid_until":mynofy_protocol::timestamp(now_ms()+86_400_000).unwrap()}]}]);
    cfg["topics"][0]["acl"] =
        json!([{"client_id":id,"allow":["publish","subscribe","inspect_own"]}]);
    let mut cfg: Config = serde_json::from_value(cfg).unwrap();
    cfg.security_storage = Some(SecurityStorage {
        directory: dir.join("security"),
        queue_capacity: 128,
        compact_after_records: 4096,
    });
    for topic in &mut cfg.topics {
        if topic.owner_id == cfg.service.id {
            topic.storage = mynofy_server::config::TopicStorage::Manual {
                encrypted: mode != ContentEncryption::None,
            };
        }
    }
    cfg.content_storage = Some(ContentStorage {
        directory: dir.join("content"),
        encryption: mode,
        max_snapshot_bytes: 128 * 1024 * 1024,
    });
    let config = dir.join("config.yaml");
    std::fs::write(&config, cfg.to_yaml().unwrap()).unwrap();
    Fixture {
        dir,
        config,
        cfg,
        key,
        id,
    }
}
async fn spawn(f: &Fixture, line: Option<&[u8]>) -> tokio::process::Child {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["serve", "--config"])
        .arg(&f.config)
        .stdin(if line.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    if let Some(line) = line {
        let mut input = child.stdin.take().unwrap();
        let _ = input.write_all(line).await;
        let _ = input.shutdown().await;
    }
    child
}
async fn ready(f: &Fixture, child: &mut tokio::process::Child) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !f.dir.join("admin.sock").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "owner stopped before listener creation"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn call(f: &Fixture, command: A) -> Value {
    let reply = admin::request(&f.dir.join("admin.sock"), command)
        .await
        .unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}
async fn kill(f: &Fixture, child: &mut tokio::process::Child) {
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    for name in ["data.sock", "admin.sock"] {
        if f.dir.join(name).exists() {
            std::fs::remove_file(f.dir.join(name)).unwrap();
        }
    }
}
async fn saved(f: &Fixture, revision: u64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = call(f, A::Status {}).await;
            let storage = &status["content_storage"];
            assert!(storage["last_error"].is_null(), "{status}");
            if storage["saved_revision"] == revision && storage["save_in_progress"] == false {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn plaintext_password_and_key_snapshots_restore_complete_state_before_serving() {
    if support::under_unprivileged_identity(
        "plaintext_password_and_key_snapshots_restore_complete_state_before_serving",
    ) {
        return;
    }
    for mode in [
        ContentEncryption::None,
        ContentEncryption::PasswordStdin,
        ContentEncryption::KeyStdin,
    ] {
        let f = fixture(mode);
        let keyline = format!("{}\n", crypto::encode(&[9; 32]));
        let secret = match mode {
            ContentEncryption::None => None,
            ContentEncryption::PasswordStdin => Some(b"test-only password line\n".as_slice()),
            ContentEncryption::KeyStdin => Some(keyline.as_bytes()),
        };
        let mut owner = spawn(&f, secret).await;
        ready(&f, &mut owner).await;
        let mut client =
            mynofy_client::connect_unix_client(&f.dir.join("data.sock"), &f.key, "local", None)
                .await
                .unwrap();
        let mut ids = Vec::new();
        for index in 0..3 {
            let reply=mynofy_client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"pub","topic":"local/events","payload":format!("confidential snapshot marker {index}")})).await.unwrap();
            assert_eq!(reply["type"], "result");
            ids.push(reply["message_id"].clone());
        }
        let _page = mynofy_client::exchange(
            &mut client,
            &json!({"version":1,"type":"poll","id":"poll","topic":"local/events"}),
        )
        .await
        .unwrap();

        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args([
                "admin",
                "content",
                "save",
                "--topics",
                "local/events",
                "--socket",
            ])
            .arg(f.dir.join("admin.sock"))
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        saved(&f, reply["revision"].as_u64().unwrap()).await;
        let snapshot = std::fs::read(f.dir.join("content/state.bin")).unwrap();
        if mode != ContentEncryption::None {
            assert!(snapshot.starts_with(b"MYNENC01"));
            assert!(
                !snapshot
                    .windows(28)
                    .any(|bytes| bytes == b"confidential snapshot marker")
            );
        }
        // A later security update is independent of the older content snapshot.
        call(
            &f,
            A::AclSet {
                client: f.id.clone(),
                topic: "local/events".into(),
                allow: vec![Action::Subscribe, Action::InspectOwn],
            },
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = call(&f, A::Status {}).await;
                if status["security_storage"]["pending_records"] == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        drop(client);
        kill(&f, &mut owner).await;
        let mut owner = spawn(&f, secret).await;
        ready(&f, &mut owner).await;
        let status = call(&f, A::Status {}).await;
        assert_eq!(status["content_storage"]["loaded"], true);
        assert_eq!(status["storage"]["resident_records"], 3);

        let mut client =
            mynofy_client::connect_unix_client(&f.dir.join("data.sock"), &f.key, "local", None)
                .await
                .unwrap();
        let _page = mynofy_client::exchange(
            &mut client,
            &json!({"version":1,"type":"poll","id":"poll","topic":"local/events"}),
        )
        .await
        .unwrap();

        let replay=mynofy_client::exchange(&mut client,&json!({"version":1,"type":"poll","id":"replay","topic":"local/events","after":"zzzzzzzz"})).await.unwrap();
        assert_eq!(replay["items"].as_array().unwrap().len(), 3);
        for id in &ids {
            let status = mynofy_client::exchange(
                &mut client,
                &json!({"version":1,"type":"get_message","id":"get","message_id":id}),
            )
            .await
            .unwrap();
            assert!(status.get("deliveries").is_none());
            assert_eq!(&status["message"]["id"], id);
        }
        assert_eq!(mynofy_client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"deny","topic":"local/events","payload":"no"})).await.unwrap()["code"],"forbidden");

        let load = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["admin", "content", "load", "--socket"])
            .arg(f.dir.join("admin.sock"))
            .output()
            .await
            .unwrap();
        assert!(!load.status.success());
        drop(client);
        kill(&f, &mut owner).await;
    }
}
#[tokio::test]
async fn wrong_key_missing_secret_and_corruption_fail_before_any_listener_is_bound() {
    if support::under_unprivileged_identity(
        "wrong_key_missing_secret_and_corruption_fail_before_any_listener_is_bound",
    ) {
        return;
    }
    let mut f = fixture(ContentEncryption::KeyStdin);
    let key = format!("{}\n", crypto::encode(&[8; 32]));
    let wrong = format!("{}\n", crypto::encode(&[7; 32]));
    let mut owner = spawn(&f, Some(key.as_bytes())).await;
    ready(&f, &mut owner).await;
    let revision = call(&f, A::ContentSave { topics: None }).await["revision"]
        .as_u64()
        .unwrap();
    saved(&f, revision).await;
    kill(&f, &mut owner).await;
    for secret in [
        None,
        Some(wrong.as_bytes()),
        Some(b"invalid-base64\n".as_slice()),
        Some(b"\n".as_slice()),
    ] {
        let child = spawn(&f, secret).await;
        let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(!f.dir.join("data.sock").exists() && !f.dir.join("admin.sock").exists());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(key.trim()));
    }
    let path = f.dir.join("content/state.bin");
    let original = std::fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    std::fs::write(&path, &corrupt).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        spawn(&f, Some(key.as_bytes())).await.wait_with_output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    assert!(!f.dir.join("data.sock").exists() && !f.dir.join("admin.sock").exists());
    std::fs::write(path, original).unwrap();
    f.cfg.content_storage.as_mut().unwrap().encryption = ContentEncryption::None;
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();
    assert!(
        !spawn(&f, None)
            .await
            .wait_with_output()
            .await
            .unwrap()
            .status
            .success()
    );
    assert!(!f.dir.join("data.sock").exists());
}

#[test]
fn terminal_secret_input_is_hidden_and_echo_is_restored_on_success_and_error() {
    if support::under_unprivileged_identity(
        "terminal_secret_input_is_hidden_and_echo_is_restored_on_success_and_error",
    ) {
        return;
    }
    let f = fixture(ContentEncryption::PasswordStdin);
    let script = r#"import os,pty,subprocess,sys,termios,time,select
binary,config,admin,data=sys.argv[1:]
for secret,success in [(b'pty-only-test-secret\n',True),(b'\n',False)]:
 master,slave=pty.openpty()
 original=termios.tcgetattr(slave)
 child=subprocess.Popen([binary,'serve','--config',config],stdin=slave,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
 try:
  deadline=time.monotonic()+10
  prompt=b''
  while b'Content password:' not in prompt:
   assert time.monotonic()<deadline, 'missing password prompt'
   if select.select([child.stderr],[],[],0.1)[0]:
    block=os.read(child.stderr.fileno(),1024)
    assert block, 'owner exited before prompt'
    prompt+=block
  assert not (termios.tcgetattr(slave)[3] & termios.ECHO), 'echo still enabled'
  os.write(master,secret)
  if success:
   while not os.path.exists(admin):
    assert child.poll() is None, 'owner exited'
    assert time.monotonic()<deadline, 'owner not ready'
    time.sleep(0.01)
  else:
   assert child.wait(timeout=10)!=0
   assert not os.path.exists(admin) and not os.path.exists(data)
  assert (termios.tcgetattr(slave)[3] & termios.ECHO)==(original[3] & termios.ECHO), 'echo not restored'
  if select.select([master],[],[],0.05)[0]:
   leaked=os.read(master,4096)
   assert b'pty-only-test-secret' not in leaked, 'secret echoed'
  if success:
   child.terminate()
   assert child.wait(timeout=5)==0, 'normal shutdown after prompt failed'
 finally:
  if child.poll() is None: child.kill()
  child.wait(timeout=10)
  os.close(master);os.close(slave)
  for path in [admin,data]:
   if os.path.exists(path): os.unlink(path)
"#;
    let output = std::process::Command::new("python3")
        .args(["-c", script, env!("CARGO_BIN_EXE_mynofy")])
        .arg(&f.config)
        .arg(f.dir.join("admin.sock"))
        .arg(f.dir.join("data.sock"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn secret_prompt_signals_restore_terminal_and_never_start_the_service() {
    if support::under_unprivileged_identity(
        "secret_prompt_signals_restore_terminal_and_never_start_the_service",
    ) {
        return;
    }
    let script = r#"import os,pty,subprocess,sys,termios,time,select,signal
binary,config,admin,data=sys.argv[1:]
for command in [('serve',),('storage','inspect')]:
 for sig in [signal.SIGINT,signal.SIGTERM]:
  for partial in [b'',b'private-partial-secret']:
   master,slave=pty.openpty()
   original=termios.tcgetattr(slave)
   child=subprocess.Popen([binary,*command,'--config',config],stdin=slave,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
   try:
    deadline=time.monotonic()+10
    prompt=b''
    while b'Content ' not in prompt or b': ' not in prompt:
     assert time.monotonic()<deadline, 'missing secret prompt'
     if select.select([child.stderr],[],[],0.1)[0]:
      block=os.read(child.stderr.fileno(),1024)
      assert block, 'process exited before prompt'
      prompt+=block
    assert not (termios.tcgetattr(slave)[3] & termios.ECHO), 'echo still enabled'
    if partial: os.write(master,partial)
    child.send_signal(sig)
    assert child.wait(timeout=5)==1, 'expected controlled error exit'
    diagnostic=child.stderr.read()
    assert signal.Signals(sig).name.encode() in diagnostic, diagnostic
    assert termios.tcgetattr(slave)==original, 'terminal attributes not restored'
    assert not os.path.exists(admin) and not os.path.exists(data), 'listener started'
    if select.select([master],[],[],0.05)[0]:
     assert b'private-partial-secret' not in os.read(master,4096), 'secret echoed'
   finally:
    if child.poll() is None: child.kill()
    child.wait(timeout=5)
    child.stderr.close()
    termios.tcsetattr(slave,termios.TCSANOW,original)
    os.close(master);os.close(slave)
"#;
    for mode in [
        ContentEncryption::PasswordStdin,
        ContentEncryption::KeyStdin,
    ] {
        let f = fixture(mode);
        let output = std::process::Command::new("python3")
            .args(["-c", script, env!("CARGO_BIN_EXE_mynofy")])
            .arg(&f.config)
            .arg(f.dir.join("admin.sock"))
            .arg(f.dir.join("data.sock"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

async fn cli_input(args: Vec<String>, input: &[u8]) -> std::process::Output {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input).await.unwrap();
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}
fn successful(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[tokio::test]
async fn cli_restart_progress_and_offline_storage_workflow_preserve_security() {
    if support::under_unprivileged_identity(
        "cli_restart_progress_and_offline_storage_workflow_preserve_security",
    ) {
        return;
    }
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fixture(ContentEncryption::KeyStdin);
    f.cfg.topics[0].storage = mynofy_server::config::TopicStorage::Ram {};
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();
    let keypath = f.dir.join("client.pk8");
    let mut keyfile = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&keypath)
        .unwrap();
    keyfile.write_all(&f.key).unwrap();
    drop(keyfile);
    let old = format!("{}\n", crypto::encode(&[8; 32]));
    let new = format!("{}\n", crypto::encode(&[9; 32]));
    let mut owner = spawn(&f, Some(old.as_bytes())).await;
    ready(&f, &mut owner).await;
    let (mut client, welcome) =
        mynofy_client::connect_unix_client_info(&f.dir.join("data.sock"), &f.key, "local", None)
            .await
            .unwrap();
    assert_eq!(welcome.restart_counter, Some(1));
    let published=mynofy_client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"retained before activation"})).await.unwrap();
    assert_eq!(published["type"], "result");
    let enabled = cli_input(
        vec![
            "admin".into(),
            "topics".into(),
            "storage".into(),
            "local/events".into(),
            "--mode".into(),
            "manual".into(),
            "--encrypted".into(),
            "true".into(),
            "--socket".into(),
            f.dir.join("admin.sock").display().to_string(),
        ],
        b"",
    )
    .await;
    successful(&enabled);
    let revision = call(&f, A::ContentSave { topics: None }).await["revision"]
        .as_u64()
        .unwrap();
    saved(&f, revision).await;
    let progress = f.dir.join("progress.json");
    let client_args = vec![
        "client".into(),
        "--socket".into(),
        f.dir.join("data.sock").display().to_string(),
        "--key".into(),
        keypath.display().to_string(),
        "--owner".into(),
        "local".into(),
        "--progress".into(),
        progress.display().to_string(),
    ];
    let adjacent = f.dir.join("progress.json.tmp");
    let marker = b"private operator file, not progress staging\n";
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&adjacent)
            .unwrap();
        file.write_all(marker).unwrap();
    }
    // Authentication and EOF already save progress; no Poll or mutation is needed.
    let empty = cli_input(client_args.clone(), b"").await;
    successful(&empty);
    assert!(progress.is_file());
    assert_eq!(std::fs::read(&adjacent).unwrap(), marker);
    assert!(!std::fs::read_dir(&f.dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".mynofy-progress-")
    }));
    // Both CLI JSONL surfaces reject malformed commands before any publish.
    for args in [
        client_args.clone(),
        client_args[..client_args.len() - 2].to_vec(),
    ] {
        let invalid = cli_input(args.clone(), b"{\"version\":1,\"type\":\"publish\",\"id\":\"null\",\"topic\":\"local/events\",\"payload\":\"must not publish\",\"expires\":null}\n").await;
        assert!(!invalid.status.success());
        let oversized = cli_input(args, &vec![b'x'; 64 * 1024 + 1]).await;
        assert!(!oversized.status.success());
        assert!(String::from_utf8_lossy(&oversized.stderr).contains("64 KiB"));
        assert_eq!(call(&f, A::Status {}).await["storage"]["active_records"], 1);
    }
    let initial = cli_input(
        client_args.clone(),
        b"{\"version\":1,\"type\":\"poll\",\"id\":\"initial\",\"topic\":\"local/events\"}\n",
    )
    .await;
    successful(&initial);
    let page: Value = serde_json::from_slice(&initial.stdout).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["next_cursor"].as_str().unwrap();
    drop(client);
    kill(&f, &mut owner).await;
    std::fs::create_dir(f.dir.join("security/snapshot.tmp")).unwrap();
    let failed = spawn(&f, Some(old.as_bytes()))
        .await
        .wait_with_output()
        .await
        .unwrap();
    assert!(!failed.status.success());
    assert!(!f.dir.join("data.sock").exists() && !f.dir.join("admin.sock").exists());
    std::fs::remove_dir(f.dir.join("security/snapshot.tmp")).unwrap();
    let mut owner = spawn(&f, Some(old.as_bytes())).await;
    ready(&f, &mut owner).await;
    let status = call(&f, A::Status {}).await;
    assert_eq!(status["restart_counter"], 2);
    let request = format!(
        "{}\n{}\n",
        json!({"version":1,"type":"poll","id":"replay","topic":"local/events","after":cursor}),
        json!({"version":1,"type":"poll","id":"after","topic":"local/events","after":cursor})
    );
    let replay = cli_input(client_args, request.as_bytes()).await;
    successful(&replay);
    let pages: Vec<Value> = String::from_utf8(replay.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(pages[0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(pages[1]["items"], json!([]));
    let stored: Value = serde_json::from_slice(&std::fs::read(&progress).unwrap()).unwrap();
    assert_eq!(stored["restart_counter"], 2);
    let storage_args = |op: &str| {
        vec![
            "storage".into(),
            op.into(),
            "--config".into(),
            f.config.display().to_string(),
        ]
    };
    let locked = cli_input(storage_args("check"), old.as_bytes()).await;
    assert!(!locked.status.success());
    kill(&f, &mut owner).await;
    let security = std::fs::read(f.dir.join("security/snapshot")).unwrap();
    let journal = std::fs::read(f.dir.join("security/journal")).unwrap();
    let backup = f.dir.join("backup");
    let mut args = storage_args("backup");
    args.extend(["--out".into(), backup.display().to_string()]);
    successful(&cli_input(args, old.as_bytes()).await);
    let before_rekey = std::fs::read(f.dir.join("content/state.bin")).unwrap();
    assert!(
        !cli_input(storage_args("rekey"), format!("{old}{new}{old}").as_bytes())
            .await
            .status
            .success()
    );
    assert_eq!(
        std::fs::read(f.dir.join("content/state.bin")).unwrap(),
        before_rekey
    );
    successful(&cli_input(storage_args("check"), old.as_bytes()).await);
    successful(&cli_input(storage_args("rekey"), format!("{old}{new}{new}").as_bytes()).await);
    assert!(
        !cli_input(storage_args("check"), old.as_bytes())
            .await
            .status
            .success()
    );
    successful(&cli_input(storage_args("check"), new.as_bytes()).await);
    let mut args = storage_args("restore");
    args.extend([
        "--from".into(),
        backup.display().to_string(),
        "--backup-secret".into(),
        "stdin".into(),
    ]);
    successful(&cli_input(args, format!("{new}{old}").as_bytes()).await);
    assert_eq!(
        std::fs::read(f.dir.join("security/snapshot")).unwrap(),
        security
    );
    assert_eq!(
        std::fs::read(f.dir.join("security/journal")).unwrap(),
        journal
    );
    let mut args = storage_args("topic-ram");
    args.extend(["--topic".into(), "local/events".into()]);
    successful(&cli_input(args, new.as_bytes()).await);
    assert_eq!(
        std::fs::read_dir(f.dir.join("content"))
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("topic-"))
            .count(),
        0
    );
    let mut owner = spawn(&f, Some(new.as_bytes())).await;
    ready(&f, &mut owner).await;
    let (mut client, welcome) =
        mynofy_client::connect_unix_client_info(&f.dir.join("data.sock"), &f.key, "local", None)
            .await
            .unwrap();
    assert_eq!(welcome.restart_counter, Some(3));
    assert_eq!(
        mynofy_client::exchange(
            &mut client,
            &json!({"version":1,"type":"poll","id":"empty","topic":"local/events"})
        )
        .await
        .unwrap()["items"],
        json!([])
    );
    drop(client);
    kill(&f, &mut owner).await;
}

#[tokio::test]
async fn offline_inspection_cleanup_and_help_do_not_modify_committed_data() {
    if support::under_unprivileged_identity(
        "offline_inspection_cleanup_and_help_do_not_modify_committed_data",
    ) {
        return;
    }
    let f = fixture(ContentEncryption::KeyStdin);
    let key = format!("{}\n", crypto::encode(&[8; 32]));
    let wrong = format!("{}\n", crypto::encode(&[9; 32]));
    let help = cli_input(vec!["storage".into(), "help".into()], b"").await;
    successful(&help);
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("storage cleanup")
    );
    let mut owner = spawn(&f, Some(key.as_bytes())).await;
    ready(&f, &mut owner).await;
    let mut client =
        mynofy_client::connect_unix_client(&f.dir.join("data.sock"), &f.key, "local", None)
            .await
            .unwrap();
    let published=mynofy_client::exchange(&mut client,&json!({"version":1,"type":"publish","id":"p","topic":"local/events","payload":"payload-must-not-appear-in-inspect"})).await.unwrap();
    assert_eq!(published["type"], "result");
    let revision = call(&f, A::ContentSave { topics: None }).await["revision"]
        .as_u64()
        .unwrap();
    saved(&f, revision).await;
    let args = |operation: &str| {
        vec![
            "storage".into(),
            operation.into(),
            "--config".into(),
            f.config.display().to_string(),
        ]
    };
    assert!(
        !cli_input(args("cleanup"), key.as_bytes())
            .await
            .status
            .success()
    );
    drop(client);
    kill(&f, &mut owner).await;
    let state = std::fs::read(f.dir.join("content/state.bin")).unwrap();
    let security = std::fs::read(f.dir.join("security/snapshot")).unwrap();
    let hash: String = crypto::sha256(b"unreferenced/topic")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let orphan = format!("topic-{hash}-0.bin");
    for name in [
        "state.tmp",
        "topic.tmp",
        "references.tmp",
        orphan.as_str(),
        "user-file",
    ] {
        std::fs::write(
            f.dir.join("content").join(name),
            b"keep unless internal unreferenced file",
        )
        .unwrap();
    }
    let inspected = cli_input(args("inspect"), key.as_bytes()).await;
    successful(&inspected);
    assert!(
        !String::from_utf8_lossy(&inspected.stdout).contains("payload-must-not-appear-in-inspect")
    );
    let report: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(report["topic_details"][0]["topic"], "local/events");
    assert_eq!(report["topic_details"][0]["active_messages"], 1);
    assert!(
        !cli_input(args("cleanup"), wrong.as_bytes())
            .await
            .status
            .success()
    );
    assert!(f.dir.join("content").join(&orphan).exists());
    let invalid = cli_input(args("backup"), b"").await;
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Missing --out"));
    successful(&cli_input(args("cleanup"), key.as_bytes()).await);
    successful(&cli_input(args("cleanup"), key.as_bytes()).await);
    for name in ["state.tmp", "topic.tmp", "references.tmp", orphan.as_str()] {
        assert!(!f.dir.join("content").join(name).exists());
    }
    assert!(f.dir.join("content/user-file").exists());
    assert_eq!(
        std::fs::read(f.dir.join("content/state.bin")).unwrap(),
        state
    );

    assert_eq!(
        std::fs::read(f.dir.join("security/snapshot")).unwrap(),
        security
    );
    let checked = cli_input(args("check"), key.as_bytes()).await;
    successful(&checked);
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(checked["active_messages"], 1);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn backup_and_admin_export_propagate_parent_directory_sync_failure() {
    if support::under_unprivileged_identity(
        "backup_and_admin_export_propagate_parent_directory_sync_failure",
    ) {
        return;
    }
    let f = fixture(ContentEncryption::None);
    let source = f.dir.join("parent_sync.c");
    std::fs::write(
        &source,
        r#"
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
int fsync(int fd) {
    char link[64], path[4096];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t n = readlink(link, path, sizeof(path)-1);
    const char *parent = getenv("MYNOFY_TEST_SYNC_PARENT");
    if (n >= 0) {
        path[n] = 0;
        if (parent && strcmp(path, parent) == 0) { errno = EIO; return -1; }
    }
    return syscall(SYS_fsync, fd);
}
"#,
    )
    .unwrap();
    let library = f.dir.join("parent_sync.so");
    let compiled = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let outputs = f.dir.join("outputs");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&outputs)
        .unwrap();
    let mut owner = spawn(&f, None).await;
    ready(&f, &mut owner).await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["admin", "export", "--socket"])
        .arg(f.dir.join("admin.sock"))
        .arg("--out")
        .arg(outputs.join("export.json"))
        .env("LD_PRELOAD", &library)
        .env("MYNOFY_TEST_SYNC_PARENT", &outputs)
        .output()
        .await
        .unwrap();
    assert!(
        !output.status.success(),
        "admin export ignored parent sync failure"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Input/output error"));
    let revision = call(&f, A::ContentSave { topics: None }).await["revision"]
        .as_u64()
        .unwrap();
    saved(&f, revision).await;
    kill(&f, &mut owner).await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["storage", "backup", "--config"])
        .arg(&f.config)
        .arg("--out")
        .arg(outputs.join("failed-backup"))
        .env("LD_PRELOAD", &library)
        .env("MYNOFY_TEST_SYNC_PARENT", &outputs)
        .output()
        .await
        .unwrap();
    assert!(
        !output.status.success(),
        "backup ignored parent sync failure"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Input/output error"));
    let good = cli_input(
        vec![
            "storage".into(),
            "backup".into(),
            "--config".into(),
            f.config.display().to_string(),
            "--out".into(),
            outputs.join("good-backup").display().to_string(),
        ],
        b"",
    )
    .await;
    successful(&good);
    assert!(outputs.join("good-backup/state.bin").is_file());
}
