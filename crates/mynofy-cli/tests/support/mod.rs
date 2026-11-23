//! Existing CLI integration scenarios must run under a real unprivileged identity.
//! A root test runner re-executes the selected test, rather than weakening the CLI policy.
pub fn under_unprivileged_identity(test: &str) -> bool {
    #[cfg(target_os = "linux")]
    let root = rustix::process::geteuid().is_root();
    #[cfg(not(target_os = "linux"))]
    let root = nix::unistd::geteuid().is_root();
    if !root {
        return false;
    }
    let output = std::process::Command::new("python3")
        .args(["-c", "import os,sys; os.setgroups([]); os.setgid(65534); os.setuid(65534); os.execv(sys.argv[1], sys.argv[1:])"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "unprivileged {test}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
