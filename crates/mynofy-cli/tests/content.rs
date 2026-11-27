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
        minimum_sequence: None,
        sync: false,
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
        store_id: Some("317050f003c743cb837757face732abc".into()),
        directory: dir.join("content"),
        encryption: mode,
        max_snapshot_bytes: 128 * 1024 * 1024,
        autosave_retry: Default::default(),
    });
    let config = dir.join("config.yaml");
    std::fs::write(&config, cfg.to_yaml().unwrap()).unwrap();
    mynofy_server::initialize_security(&cfg).unwrap();
    mynofy_server::content::initialize(&cfg).unwrap();
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
    let sockets = [f.dir.join("admin.sock"), f.dir.join("data.sock")];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "owner stopped before both listeners were ready"
            );
            if sockets.iter().all(|path| support::unix_socket_ready(path)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("admin and data listeners did not reach mode 0600");
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
        assert_eq!(status["resident_messages"], 3);

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
    let script = r#"import os,pty,stat,subprocess,sys,termios,time,select
binary,config,admin,data=sys.argv[1:]
def ready(path):
 try:
  mode=os.lstat(path).st_mode
 except FileNotFoundError:
  return False
 return stat.S_ISSOCK(mode) and stat.S_IMODE(mode)==0o600
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
   while not (ready(admin) and ready(data)):
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
        assert_eq!(call(&f, A::StorageStats {}).await["active_records"], 1);
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
    use std::os::unix::fs::MetadataExt;
    let inode = std::fs::metadata(&progress).unwrap().ino();
    let read_only = cli_input(client_args.clone(), b"{\"version\":1,\"type\":\"get_permissions\",\"id\":\"permissions\",\"topic\":\"local/events\"}\n").await;
    successful(&read_only);
    assert_eq!(std::fs::metadata(&progress).unwrap().ino(), inode);

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
    use std::os::unix::fs::PermissionsExt;
    for entry in std::fs::read_dir(&backup).unwrap() {
        std::fs::set_permissions(
            entry.unwrap().path(),
            std::fs::Permissions::from_mode(0o400),
        )
        .unwrap();
    }
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o500)).unwrap();
    let mut args = storage_args("restore");
    args.extend([
        "--from".into(),
        backup.display().to_string(),
        "--backup-secret".into(),
        "stdin".into(),
    ]);
    successful(&cli_input(args, format!("{new}{old}").as_bytes()).await);
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o700)).unwrap();

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

#[tokio::test]
async fn offline_topic_ram_preserves_unrelated_messages_after_fresh_cli_and_service_reopen() {
    if support::under_unprivileged_identity(
        "offline_topic_ram_preserves_unrelated_messages_after_fresh_cli_and_service_reopen",
    ) {
        return;
    }
    use mynofy_server::config::{Client, PublicKey, Rule, TopicStorage};
    let mut f = fixture(ContentEncryption::None);
    let other_key = crypto::generate_key().unwrap();
    let public = crypto::public_key(&other_key).unwrap();
    let other_id = crypto::initial_client_id(&public);
    f.cfg.clients.push(Client {
        client_id: other_id.clone(),
        display_name: None,
        disabled: false,
        keys: vec![PublicKey {
            algorithm: "ed25519".into(),
            public_key: crypto::encode(&public),
            valid_until: mynofy_protocol::timestamp(now_ms() + 86400000).unwrap(),
        }],
    });
    let mut live = f.cfg.topics[0].clone();
    live.name = "live".into();
    live.acl = Some(vec![Rule {
        client_id: other_id.clone(),
        allow: vec![Action::Publish, Action::Subscribe],
    }]);
    f.cfg.topics.push(live);
    let mut converted = f.cfg.topics[0].clone();
    converted.name = "converted".into();
    f.cfg.topics.push(converted);
    std::fs::remove_dir_all(&f.cfg.security_storage.as_ref().unwrap().directory).unwrap();
    mynofy_server::initialize_security(&f.cfg).unwrap();
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();
    let mut owner = spawn(&f, None).await;
    ready(&f, &mut owner).await;
    let mut first =
        mynofy_client::connect_unix_client(&f.dir.join("data.sock"), &f.key, "local", None)
            .await
            .unwrap();
    let expires = now_ms() + 3000;
    let expired=mynofy_client::exchange(&mut first,&json!({"version":1,"type":"publish","id":"old","topic":"local/events","payload":"expires","expires":{"time":mynofy_protocol::timestamp(expires).unwrap()}})).await.unwrap();
    assert_eq!(expired["type"], "result");
    let mut second =
        mynofy_client::connect_unix_client(&f.dir.join("data.sock"), &other_key, "local", None)
            .await
            .unwrap();
    let published=mynofy_client::exchange(&mut second,&json!({"version":1,"type":"publish","id":"live","topic":"live","payload":"survives conversion"})).await.unwrap();
    assert_eq!(published["type"], "result");
    let save = call(&f, A::ContentSave { topics: None }).await;
    saved(&f, save["revision"].as_u64().unwrap()).await;
    drop(first);
    drop(second);
    kill(&f, &mut owner).await;
    f.cfg.topics.last_mut().unwrap().storage = TopicStorage::Ram {};
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();
    tokio::time::sleep(Duration::from_millis(expires.saturating_sub(now_ms()) + 25)).await;
    for operation in ["topic-ram", "check"] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"));
        command
            .args(["storage", operation, "--config"])
            .arg(&f.config);
        if operation == "topic-ram" {
            command.args(["--topic", "converted"]);
        }
        let result = command.output().await.unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let result: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(result["active_messages"], 1);
    }
    let mut owner = spawn(&f, None).await;
    ready(&f, &mut owner).await;
    let mut client = mynofy_client::connect_unix_client(
        &f.dir.join("data.sock"),
        &other_key,
        "local",
        Some(&other_id),
    )
    .await
    .unwrap();
    let page = mynofy_client::exchange(
        &mut client,
        &json!({"version":1,"type":"poll","id":"read","topic":"live"}),
    )
    .await
    .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["message"]["id"], published["message_id"]);
    assert_eq!(page["items"][0]["message"]["producer_id"], other_id);
    assert_eq!(
        page["items"][0]["message"]["payload"],
        "survives conversion"
    );
    drop(client);
    kill(&f, &mut owner).await;
}

#[test]
fn serve_rejects_missing_or_empty_security_store_without_recreating_it() {
    if support::under_unprivileged_identity(
        "serve_rejects_missing_or_empty_security_store_without_recreating_it",
    ) {
        return;
    }
    let f = fixture(ContentEncryption::None);
    let security = &f.cfg.security_storage.as_ref().unwrap().directory;
    std::fs::rename(security, f.dir.join("preserved-security")).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["serve", "--config"])
            .arg(&f.config)
            .output()
            .unwrap()
    };
    let missing = run();
    assert!(!missing.status.success());
    assert!(!security.exists());
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(security)
        .unwrap();
    let empty = run();
    assert!(!empty.status.success());
    assert_eq!(std::fs::read_dir(security).unwrap().count(), 0);
    assert!(!f.dir.join("data.sock").exists());
}

#[test]
fn serve_requires_the_configured_content_volume() {
    if support::under_unprivileged_identity("serve_requires_the_configured_content_volume") {
        return;
    }
    let mut f = fixture(ContentEncryption::None);
    let storage = f.cfg.content_storage.as_mut().unwrap();
    let volume = storage.directory.clone();
    std::fs::rename(&volume, f.dir.join("preserved-content")).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["serve", "--config"])
            .arg(&f.config)
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    assert!(!volume.exists());
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&volume)
        .unwrap();
    assert!(!run().status.success());
    assert_eq!(std::fs::read_dir(&volume).unwrap().count(), 0);
    storage.store_id = Some("55555555555555555555555555555555".into());
    mynofy_server::content::initialize(&f.cfg).unwrap();
    assert!(!run().status.success());
    assert!(!f.dir.join("data.sock").exists());
}

#[tokio::test]
async fn sigterm_reports_failed_queued_save_and_preserves_previous_generation() {
    if support::under_unprivileged_identity(
        "sigterm_reports_failed_queued_save_and_preserves_previous_generation",
    ) {
        return;
    }
    let mut f = fixture(ContentEncryption::None);
    let mut automatic = f.cfg.topics[0].clone();
    automatic.name = "local/automatic".into();
    automatic.storage = mynofy_server::config::TopicStorage::Interval {
        seconds: 1,
        encrypted: false,
    };
    f.cfg.topics.push(automatic);
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();
    let mut owner = spawn(&f, None).await;
    ready(&f, &mut owner).await;
    call(
        &f,
        A::AclSet {
            client: f.id.clone(),
            topic: "local/automatic".into(),
            allow: vec![Action::Publish, Action::Subscribe],
        },
    )
    .await;
    let socket = f.dir.join("data.sock");
    let connect = || mynofy_client::connect_unix_client(&socket, &f.key, "local", None);
    let mut client = connect().await.unwrap();
    let publish = |payload| json!({"version":1,"type":"publish","id":"pub","topic":"local/events","payload":payload});
    assert_eq!(
        mynofy_client::exchange(&mut client, &publish("A"))
            .await
            .unwrap()["type"],
        "result"
    );
    let first = call(&f, A::ContentSave { topics: None }).await["revision"]
        .as_u64()
        .unwrap();
    saved(&f, first).await;
    std::fs::create_dir(f.dir.join("content/state.tmp")).unwrap();
    assert_eq!(
        mynofy_client::exchange(&mut client, &publish("B"))
            .await
            .unwrap()["type"],
        "result"
    );
    let second = call(
        &f,
        A::ContentSave {
            topics: Some(vec!["local/events".into()]),
        },
    )
    .await["revision"]
        .as_u64()
        .unwrap();
    assert!(second > first);
    let fixture = &f;
    let status = move |topic: &'static str| async move {
        let status = call(fixture, A::Status {}).await;
        status["content_storage"]["topics"][topic].clone()
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while status("local/events").await["save_failure"]["revision"] != second {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    std::fs::remove_dir(f.dir.join("content/state.tmp")).unwrap();
    let automatic = json!({"version":1,"type":"publish","id":"automatic","topic":"local/automatic","payload":"interval"});
    assert_eq!(
        mynofy_client::exchange(&mut client, &automatic)
            .await
            .unwrap()["type"],
        "result"
    );
    let automatic_revision = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let saved = status("local/automatic").await["saved_revision"]
                .as_u64()
                .unwrap();
            if saved > second {
                break saved;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();

    assert!(
        tokio::process::Command::new("kill")
            .args(["-TERM", &owner.id().unwrap().to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let output = tokio::time::timeout(Duration::from_secs(5), owner.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "content_drain_failed: queued_revision={second} saved_revision={automatic_revision}"
    )));
    let mut restarted = spawn(&f, None).await;
    ready(&f, &mut restarted).await;
    let mut client = connect().await.unwrap();
    let page = mynofy_client::exchange(
        &mut client,
        &json!({"version":1,"type":"poll","id":"poll","topic":"local/events"}),
    )
    .await
    .unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["message"]["payload"], "A");
    kill(&f, &mut restarted).await;
}

#[tokio::test]
async fn admin_topic_json_selects_names_with_commas() {
    if support::under_unprivileged_identity("admin_topic_json_selects_names_with_commas") {
        return;
    }
    let mut f = fixture(ContentEncryption::None);
    let name = "local/events,optional";
    let mut topic = f.cfg.topics[0].clone();
    topic.name = name.into();
    f.cfg.topics.push(topic);
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();
    let mut owner = spawn(&f, None).await;
    ready(&f, &mut owner).await;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args([
            "admin",
            "content",
            "save",
            "--topics-json",
            &json!([name]).to_string(),
            "--socket",
        ])
        .arg(f.dir.join("admin.sock"))
        .output()
        .await
        .unwrap();
    successful(&output);
    let revision = serde_json::from_slice::<Value>(&output.stdout).unwrap()["revision"]
        .as_u64()
        .unwrap();
    saved(&f, revision).await;
    assert_eq!(
        call(&f, A::Status {}).await["content_storage"]["topics"][name]["saved_revision"],
        revision
    );
    kill(&f, &mut owner).await;
}

#[tokio::test]
async fn cli_does_not_announce_readiness_when_admin_bind_fails() {
    if support::under_unprivileged_identity("cli_does_not_announce_readiness_when_admin_bind_fails")
    {
        return;
    }
    let f = fixture(ContentEncryption::None);
    let admin = f.dir.join("admin.sock");
    std::fs::write(&admin, b"owned by another program").unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        spawn(&f, None).await.wait_with_output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("listening on"));
    assert_eq!(std::fs::read(admin).unwrap(), b"owned by another program");
    assert!(!f.dir.join("data.sock").exists());
}

async fn mcp_poll_with_progress(
    socket: &std::path::Path,
    key: &std::path::Path,
    previous: Option<(&Value, &Value)>,
) -> (Value, Value) {
    use tokio::io::AsyncBufReadExt;
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["mcp", "--socket"])
        .arg(socket)
        .args(["--key"])
        .arg(key)
        .args(["--owner", "local"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = process.stdin.take().unwrap();
    let mut output = tokio::io::BufReader::new(process.stdout.take().unwrap()).lines();
    let mut info = Value::Null;
    let mut page = Value::Null;
    for id in 0..4 {
        let request = match id {
            0 => {
                json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"integration","version":"1"}}})
            }
            1 => json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            2 => {
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mynofy_connection_info","arguments":{}}})
            }
            _ => {
                let mut arguments = json!({"topic":"local/events"});
                if let Some((old, cursor)) = previous
                    && old["boot_id"] == info["boot_id"]
                {
                    arguments["after"] = cursor.clone();
                }
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"mynofy_poll","arguments":arguments}})
            }
        };
        input
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        if id != 1 {
            let line = tokio::time::timeout(Duration::from_secs(3), output.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["id"], id, "{reply}");
            if id == 2 {
                info = reply["result"]["structuredContent"].clone();
            }
            if id == 3 {
                page = reply["result"]["structuredContent"].clone();
            }
        }
    }
    drop(input);
    assert!(process.wait().await.unwrap().success());
    (info, page)
}

#[tokio::test]
async fn restored_security_counter_cannot_reuse_the_previous_boot_cursor() {
    if support::under_unprivileged_identity(
        "restored_security_counter_cannot_reuse_the_previous_boot_cursor",
    ) {
        return;
    }
    let mut f = fixture(ContentEncryption::None);
    let socket = f.dir.join("data.sock");
    let connect = || mynofy_client::connect_unix_client_info(&socket, &f.key, "local", None);
    let mut first = spawn(&f, None).await;
    ready(&f, &mut first).await;
    let (mut client, _) = connect().await.unwrap();
    let publish = |payload| json!({"version":1,"type":"publish","id":"pub","topic":"local/events","payload":payload});
    assert_eq!(
        mynofy_client::exchange(&mut client, &publish("A"))
            .await
            .unwrap()["type"],
        "result"
    );
    call(
        &f,
        A::AclSet {
            client: f.id.clone(),
            topic: "local/events".into(),
            allow: vec![Action::Publish, Action::Subscribe],
        },
    )
    .await;
    let revision = call(&f, A::ContentSave { topics: None }).await["revision"]
        .as_u64()
        .unwrap();
    saved(&f, revision).await;
    drop(client);
    kill(&f, &mut first).await;

    let security = f.dir.join("security");
    let snapshot = std::fs::read(security.join("snapshot")).unwrap();
    let journal = std::fs::read(security.join("journal")).unwrap();
    f.cfg.security_storage.as_mut().unwrap().minimum_sequence = Some(1);
    std::fs::write(&f.config, f.cfg.to_yaml().unwrap()).unwrap();

    let mut second = spawn(&f, None).await;
    ready(&f, &mut second).await;
    let (mut client, second_boot) = connect().await.unwrap();
    assert_eq!(
        mynofy_client::exchange(&mut client, &publish("B"))
            .await
            .unwrap()["type"],
        "result"
    );
    let keypath = f.dir.join("client.pk8");
    std::fs::write(&keypath, &f.key).unwrap();
    std::fs::set_permissions(
        &keypath,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let progress = f.dir.join("progress.json");
    let args = vec![
        "client".into(),
        "--socket".into(),
        socket.display().to_string(),
        "--owner".into(),
        "local".into(),
        "--key".into(),
        keypath.display().to_string(),
        "--progress".into(),
        progress.display().to_string(),
    ];
    let initial = cli_input(
        args.clone(),
        b"{\"version\":1,\"type\":\"poll\",\"id\":\"old\",\"topic\":\"local/events\"}\n",
    )
    .await;
    successful(&initial);
    let previous: Value = serde_json::from_slice(&initial.stdout).unwrap();
    assert_eq!(previous["items"].as_array().unwrap().len(), 2);
    let (mcp_before, mcp_first_page) = mcp_poll_with_progress(&socket, &keypath, None).await;
    assert_eq!(
        mcp_before["boot_id"],
        second_boot.boot_id.as_deref().unwrap()
    );
    assert_eq!(mcp_first_page["boot_id"], mcp_before["boot_id"]);
    assert_eq!(
        mcp_first_page["response"]["next_cursor"],
        previous["next_cursor"]
    );
    drop(client);
    kill(&f, &mut second).await;

    std::fs::write(security.join("snapshot"), snapshot).unwrap();
    std::fs::write(security.join("journal"), journal).unwrap();
    let mut restored = spawn(&f, None).await;
    ready(&f, &mut restored).await;
    let (mut client, current_boot) = connect().await.unwrap();
    assert_eq!(second_boot.restart_counter, current_boot.restart_counter);
    assert!(second_boot.boot_id.is_some());
    assert!(current_boot.boot_id.is_some());
    assert_ne!(second_boot.boot_id, current_boot.boot_id);
    assert_eq!(
        mynofy_client::exchange(&mut client, &publish("C"))
            .await
            .unwrap()["type"],
        "result"
    );
    let request = format!(
        "{}\n",
        json!({"version":1,"type":"poll","id":"new","topic":"local/events","after":previous["next_cursor"]})
    );
    let output = cli_input(args, request.as_bytes()).await;
    successful(&output);
    let replay: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(replay["items"].as_array().unwrap().len(), 2);
    assert_eq!(replay["items"][1]["message"]["payload"], "C");
    let (mcp_current, mcp_replay) = mcp_poll_with_progress(
        &socket,
        &keypath,
        Some((&mcp_before, &previous["next_cursor"])),
    )
    .await;
    assert_eq!(
        mcp_current["restart_counter"],
        mcp_before["restart_counter"]
    );
    assert_ne!(mcp_current["boot_id"], mcp_before["boot_id"]);
    assert_eq!(mcp_replay["boot_id"], mcp_current["boot_id"]);
    assert_eq!(
        mcp_replay["response"]["items"][1]["message"]["payload"],
        "C"
    );
    kill(&f, &mut restored).await;
}

#[tokio::test]
async fn progress_over_budget_fails_before_emitting_poll_response() {
    if support::under_unprivileged_identity(
        "progress_over_budget_fails_before_emitting_poll_response",
    ) {
        return;
    }
    let f = fixture(ContentEncryption::None);
    let mut owner = spawn(&f, None).await;
    ready(&f, &mut owner).await;
    let socket = f.dir.join("data.sock");
    let (mut client, welcome) =
        mynofy_client::connect_unix_client_info(&socket, &f.key, "local", None)
            .await
            .unwrap();
    let response = mynofy_client::exchange(
        &mut client,
        &json!({
            "version":1,"type":"publish","id":"pub","topic":"local/events","payload":"visible"
        }),
    )
    .await
    .unwrap();
    assert_eq!(response["type"], "result");

    let mut state = mynofy_client::progress::ClientProgress {
        service_id: welcome.service_id,
        client_id: welcome.client_id,
        restart_counter: welcome.restart_counter,
        boot_id: welcome.boot_id,
        ..Default::default()
    };
    let limit = 4 * 1024 * 1024;
    let mut size = serde_json::to_vec(&state).unwrap().len();
    let cursor = "00000001".to_owned();
    for index in 0..10_000 {
        let prefix = format!("{index:05}");
        let cost = |name: &str| {
            serde_json::to_vec(name).unwrap().len() + 11 + usize::from(!state.cursors.is_empty())
        };
        let name = (0..=251).rev().find_map(|slashes| {
            let name = format!("{prefix}{}", "\\".repeat(slashes));
            (size + cost(&name) <= limit).then_some(name)
        });
        let Some(name) = name else { break };
        size += cost(&name);
        state.cursors.insert(name, cursor.clone());
        if limit - size < 15 {
            break;
        }
    }
    let original = serde_json::to_vec(&state).unwrap();
    assert_eq!(original.len(), size);
    assert!((limit - 15..=limit).contains(&size));
    let keypath = f.dir.join("client.pk8");
    let progress = f.dir.join("progress.json");
    use std::os::unix::fs::PermissionsExt;
    for (path, bytes) in [
        (&keypath, f.key.as_slice()),
        (&progress, original.as_slice()),
    ] {
        std::fs::write(path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let output = cli_input(
        vec![
            "client".into(),
            "--socket".into(),
            socket.display().to_string(),
            "--owner".into(),
            "local".into(),
            "--key".into(),
            keypath.display().to_string(),
            "--progress".into(),
            progress.display().to_string(),
        ],
        b"{\"version\":1,\"type\":\"poll\",\"id\":\"poll\",\"topic\":\"local/events\"}\n",
    )
    .await;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Progress file exceeds size limit"));
    assert!(std::fs::read(progress).unwrap() == original);
    kill(&f, &mut owner).await;
}
