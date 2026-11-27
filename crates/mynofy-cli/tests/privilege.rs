#![cfg(target_os = "linux")]
mod support;
use mynofy_protocol::crypto;
use serde_json::{Value, json};
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, chown},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    dir: PathBuf,
    child: Option<Child>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "mnp-{:016x}",
        u64::from_le_bytes(crypto::random().unwrap())
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    Fixture { dir, child: None }
}
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mynofy"))
}
fn target() -> (u32, u32) {
    if rustix::process::geteuid().is_root() {
        (65534, 65534)
    } else {
        (
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw(),
        )
    }
}
#[test]
fn privileged_start_fails_before_any_command_file_access() {
    let f = fixture();
    let out = f.dir.join("must-not-exist");
    let result = cli()
        .args(["keygen", "--run-as-uid", "0", "--run-as-gid", "1", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!out.exists());
    if rustix::process::geteuid().is_root() {
        for command in ["serve", "check-config"] {
            let result = cli()
                .args([command, "--config", "/does/not/exist"])
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(String::from_utf8_lossy(&result.stderr).contains("Privileged start requires"));
        }
        let result = cli().args(["keygen", "--out"]).arg(&out).output().unwrap();
        assert!(!result.status.success());
        assert!(!out.exists());
        let (uid, gid) = target();
        let result = cli()
            .args([
                "--run-as-uid",
                &uid.to_string(),
                "--run-as-gid",
                &gid.to_string(),
                "keygen",
                "--out",
            ])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "must not access a root-only directory after dropping privileges"
        );
        assert!(!out.exists());
    }
}
fn field<'a>(status: &'a str, key: &str) -> &'a str {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap()
        .trim()
}
#[test]
fn runtime_threads_and_sockets_have_only_target_privileges() {
    for explicit in [true, false] {
        let mut f = fixture();
        let (uid, gid) = target();
        if rustix::process::geteuid().is_root() {
            chown(&f.dir, Some(uid), Some(gid)).unwrap();
        }
        let mut config: Value =
            serde_json::from_str(include_str!("../../../config/scenarios/local.json")).unwrap();
        let data = f.dir.join("data.sock");
        let admin = f.dir.join("admin.sock");
        config["service"] = json!({"id":"local","unix_socket":data,"admin_socket":admin});
        config["security_storage"] = json!({"directory":"security"});
        let cfg = f.dir.join("config.json");
        std::fs::write(&cfg, config.to_string()).unwrap();
        let command = || {
            let mut command = if !explicit && rustix::process::geteuid().is_root() {
                // Exercise an ordinary non-root invocation too, with no retained root group.
                let mut command = Command::new("python3");
                let script = "import os,sys; os.setgroups([]); os.setgid(65534); os.setuid(65534); os.execv(sys.argv[1],sys.argv[1:])";
                command.args(["-c", script, env!("CARGO_BIN_EXE_mynofy")]);
                command
            } else {
                cli()
            };
            if explicit {
                command.args([
                    "--run-as-uid",
                    &uid.to_string(),
                    "--run-as-gid",
                    &gid.to_string(),
                ]);
            }
            command
        };
        let setup = command()
            .args(["security-init", "--config"])
            .arg(&cfg)
            .output()
            .unwrap();
        assert!(
            setup.status.success(),
            "{}",
            String::from_utf8_lossy(&setup.stderr)
        );
        let mut command = command();
        let child = command
            .args(["serve", "--config"])
            .arg(&cfg)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        f.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(5);
        // The admin listener is bound before the data listener. Each path
        // becomes visible before bind_unix finishes setting its permissions.
        while !(support::unix_socket_ready(&data) && support::unix_socket_ready(&admin)) {
            let child = f.child.as_mut().unwrap();
            if let Some(status) = child.try_wait().unwrap() {
                panic!(
                    "server exited {status}: {}",
                    String::from_utf8_lossy(
                        &child
                            .stderr
                            .take()
                            .map(|mut err| {
                                use std::io::Read;
                                let mut buf = Vec::new();
                                err.read_to_end(&mut buf).unwrap();
                                buf
                            })
                            .unwrap_or_default()
                    )
                );
            }
            assert!(
                Instant::now() < deadline,
                "startup timed out waiting for both Unix sockets at mode 0600"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        for socket in [&data, &admin] {
            let meta = std::fs::metadata(socket).unwrap();
            assert_eq!(meta.uid(), uid);
            assert_eq!(meta.gid(), gid);
            assert_eq!(meta.mode() & 0o777, 0o600);
        }
        let threads: Vec<_> = std::fs::read_dir(format!("/proc/{pid}/task"))
            .unwrap()
            .collect();
        assert!(threads.len() > 1, "Tokio worker threads must be included");
        for thread in threads {
            let status = std::fs::read_to_string(thread.unwrap().path().join("status")).unwrap();
            assert_eq!(
                field(&status, "Uid:")
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                vec![uid.to_string(); 4]
            );
            assert_eq!(
                field(&status, "Gid:")
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                vec![gid.to_string(); 4]
            );
            if rustix::process::geteuid().is_root() {
                assert!(field(&status, "Groups:").is_empty());
            }
            for cap in ["CapEff:", "CapPrm:", "CapInh:", "CapAmb:"] {
                assert_eq!(
                    u64::from_str_radix(field(&status, cap), 16).unwrap(),
                    0,
                    "{cap}"
                );
            }
            assert_eq!(field(&status, "NoNewPrivs:"), "1");
            if explicit
                && rustix::thread::capabilities(None)
                    .unwrap()
                    .effective
                    .contains(rustix::thread::CapabilitySet::SETPCAP)
            {
                assert_eq!(
                    u64::from_str_radix(field(&status, "CapBnd:"), 16).unwrap(),
                    0
                );
            }
        }
        let mut request = cli();
        if rustix::process::geteuid().is_root() {
            request.args([
                "--run-as-uid",
                &uid.to_string(),
                "--run-as-gid",
                &gid.to_string(),
            ]);
        }
        let output = request
            .args(["admin", "status", "--socket"])
            .arg(&admin)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["service_id"], "local");
    }
}
