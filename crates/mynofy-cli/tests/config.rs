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
        assert!(!demo.join("security").exists());
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
