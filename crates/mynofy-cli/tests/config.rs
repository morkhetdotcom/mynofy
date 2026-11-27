#![cfg(unix)]
mod support;
use mynofy_protocol::crypto;
use mynofy_server::config::Config;
use std::{
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mny-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn conversion_preserves_paths_and_security_identity_and_never_overwrites() {
    if support::under_unprivileged_identity(
        "conversion_preserves_paths_and_security_identity_and_never_overwrites",
    ) {
        return;
    }
    let directory = Directory::new();
    let source = directory.0.join("original.json");
    let yaml = directory.0.join("config.yaml");
    std::fs::write(&source, include_str!("../../../config/scenarios/unix.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["config-convert", "--config"])
        .arg(&source)
        .arg("--out")
        .arg(&yaml)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = Config::load(&source).unwrap();
    let converted = Config::load(&yaml).unwrap();
    assert_eq!(
        serde_json::to_value(original).unwrap(),
        serde_json::to_value(converted).unwrap()
    );
    assert_eq!(std::fs::metadata(&yaml).unwrap().mode() & 0o777, 0o600);
    assert!(!directory.0.join("original.json.security").exists());
    let before = std::fs::read(&yaml).unwrap();
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["config-convert", "--config"])
            .arg(&source)
            .arg("--out")
            .arg(&yaml)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(std::fs::read(&yaml).unwrap(), before);
    let invalid = directory.0.join("invalid.yaml");
    let invalid_out = directory.0.join("must-not-exist.yaml");
    std::fs::write(&invalid, "schema_version: 1\nschema_version: 2\n").unwrap();
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["config-convert", "--config"])
            .arg(invalid)
            .arg("--out")
            .arg(&invalid_out)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!invalid_out.exists());
    let json = directory.0.join("roundtrip.json");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_mynofy"))
            .args(["config-convert", "--config"])
            .arg(&yaml)
            .arg("--out")
            .arg(&json)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        serde_json::to_value(Config::load(&json).unwrap()).unwrap(),
        serde_json::to_value(Config::load(&source).unwrap()).unwrap()
    );
}
#[test]
fn setup_scripts_generate_valid_yaml_without_python_yaml_dependency() {
    if support::under_unprivileged_identity(
        "setup_scripts_generate_valid_yaml_without_python_yaml_dependency",
    ) {
        return;
    }
    let directory = Directory::new();
    // Exercise the same directory aliasing as macOS /tmp on every Unix platform.
    let alias = directory.0.join("alias");
    std::os::unix::fs::symlink(&directory.0, &alias).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for relative in ["scripts", "config/scenarios", "config/adapters", "bin"] {
        std::fs::create_dir_all(directory.0.join(relative)).unwrap();
    }
    for relative in [
        "config/scenarios/local.json",
        "config/scenarios/unix.json",
        "config/adapters/echo.json",
    ] {
        std::fs::copy(root.join(relative), directory.0.join(relative)).unwrap();
    }
    let cargo = directory.0.join("bin/cargo");
    // Exercise the scripts against the actual built binary without nested Cargo compilation.
    std::fs::write(&cargo,"#!/bin/sh\nwhile [ \"$#\" -gt 0 ] && [ \"$1\" != -- ]; do shift; done\n[ \"$#\" -gt 0 ] || exit 2\nshift\nexec \"$MYNOFY_TEST_BINARY\" \"$@\"\n").unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    for (script, demo) in [
        ("setup-local.sh", "demo"),
        ("setup-unix.sh", "unix"),
        ("setup-adapter.sh", "adapter"),
    ] {
        let path = directory.0.join("scripts").join(script);
        std::fs::copy(root.join("scripts").join(script), &path).unwrap();
        let path_env = std::env::join_paths(
            std::iter::once(directory.0.join("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let output = Command::new("bash")
            .arg(&path)
            .env("PATH", &path_env)
            .env("MYNOFY_TEST_BINARY", env!("CARGO_BIN_EXE_mynofy"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{script}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let demo = directory.0.join(".local").join(demo);
        let cfg = demo.join("config.yaml");
        let config = Config::load(
            &alias
                .join(".local")
                .join(demo.file_name().unwrap())
                .join("config.yaml"),
        )
        .unwrap();
        assert_eq!(
            config.security_storage.unwrap().directory,
            demo.canonicalize().unwrap().join("security")
        );
        assert!(!demo.join("config-seed.json").exists());
        assert!(demo.join("security/snapshot").is_file());
        assert!(demo.join("client.pk8").exists());
        assert_eq!(std::fs::metadata(cfg).unwrap().mode() & 0o777, 0o600);
        assert!(
            !Command::new("bash")
                .arg(&path)
                .env("PATH", path_env)
                .env("MYNOFY_TEST_BINARY", env!("CARGO_BIN_EXE_mynofy"))
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

#[test]
fn security_init_rejects_positive_recovery_floor_before_creating_store() {
    if support::under_unprivileged_identity(
        "security_init_rejects_positive_recovery_floor_before_creating_store",
    ) {
        return;
    }
    let f = Directory::new();
    let path = f.0.join("floor.json");
    let mut config: serde_json::Value =
        serde_json::from_str(include_str!("../../../config/scenarios/unix.json")).unwrap();
    config["security_storage"] =
        serde_json::json!({"directory":"security-floor","minimum_sequence":1});
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_mynofy"))
        .args(["security-init", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("positive minimum_sequence"));
    assert!(!f.0.join("security-floor").exists());
}

#[test]
fn failed_private_output_can_retry_at_same_path() {
    if support::under_unprivileged_identity("failed_private_output_can_retry_at_same_path") {
        return;
    }
    let f = Directory::new();
    let input = f.0.join("input.json");
    std::fs::write(&input, include_str!("../../../config/scenarios/unix.json")).unwrap();
    let output = Command::new("python3")
        .args([
            "-c",
            r#"
import os, pathlib, resource, signal, subprocess, sys
binary, source = sys.argv[1:]
root = pathlib.Path(source).parent
def limit():
    signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
    resource.setrlimit(resource.RLIMIT_FSIZE, (32, 32))
cases = [('config.json', ['config-convert', '--config', source]), ('key.pk8', ['keygen'])]
for name, command in cases:
    output = root / name
    args = [binary, *command, '--out', str(output)]
    failed = subprocess.run(args, preexec_fn=limit, capture_output=True)
    assert failed.returncode == 1, failed
    assert not output.exists(), output
    assert not list(root.glob('.mynofy-output-*.tmp'))
    retry = subprocess.run(args, capture_output=True)
    assert retry.returncode == 0, retry
    data = output.read_bytes()
    assert output.stat().st_mode & 0o777 == 0o600
    refused = subprocess.run(args, capture_output=True)
    assert refused.returncode != 0 and output.read_bytes() == data
"#,
            env!("CARGO_BIN_EXE_mynofy"),
        ])
        .arg(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn static_serve_check_and_start_require_explicit_storage_configuration() {
    if support::under_unprivileged_identity(
        "static_serve_check_and_start_require_explicit_storage_configuration",
    ) {
        return;
    }
    let f = Directory::new();
    let input = f.0.join("input.json");
    let modern = f.0.join("modern.json");
    std::fs::write(&input, include_str!("../../../config/scenarios/unix.json")).unwrap();
    let run = |command: &str, path: &std::path::Path, output: Option<&std::path::Path>| {
        let mut run = Command::new(env!("CARGO_BIN_EXE_mynofy"));
        run.args([command, "--config"]).arg(path);
        if let Some(path) = output {
            run.arg("--out").arg(path);
        }
        run.output().unwrap()
    };
    let incompatible = run("check-config", &input, None);
    assert!(!incompatible.status.success());
    assert!(String::from_utf8_lossy(&incompatible.stderr).contains("Static serve check failed"));
    assert!(
        run("config-convert", &input, Some(&modern))
            .status
            .success()
    );
    let valid = run("check-config", &modern, None);
    assert!(valid.status.success());
    assert!(String::from_utf8_lossy(&valid.stdout).contains("Valid static serve configuration"));

    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&modern).unwrap()).unwrap();
    config["content_storage"] =
        serde_json::json!({"directory":"content","encryption":"password_stdin"});
    std::fs::write(&modern, serde_json::to_vec(&config).unwrap()).unwrap();
    for command in ["check-config", "serve"] {
        // No secret is provided. Serve must report the static error before asking for it.
        let result = run(command, &modern, None);
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(
            error.contains("content_storage.store_id"),
            "{command}: {error}"
        );
    }
    config["content_storage"]["store_id"] = "317050f003c743cb837757face732abc".into();
    std::fs::write(&modern, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(run("check-config", &modern, None).status.success());
    assert!(!f.0.join("input.json.security").exists());
    assert!(!f.0.join("content").exists());
}
