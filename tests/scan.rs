use std::{fs, process::Command, time::SystemTime};

#[test]
fn scan_silently_skips_broken_worktrees() {
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "clean-my-code-broken-worktree-{}-{stamp}",
        std::process::id()
    ));
    let broken = root.join("broken");
    let healthy = root.join("healthy");
    fs::create_dir_all(broken.join("node_modules")).unwrap();
    fs::write(broken.join(".git"), "gitdir: ../missing/worktrees/broken\n").unwrap();
    fs::create_dir_all(healthy.join("node_modules")).unwrap();
    let init = Command::new("git")
        .arg("init")
        .arg(&healthy)
        .output()
        .unwrap();
    assert!(init.status.success());
    fs::write(healthy.join(".gitignore"), "node_modules/\n").unwrap();
    fs::write(healthy.join("node_modules/artifact"), "test").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_clean-my-code"))
        .arg("--root")
        .arg(&root)
        .arg("scan")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Repos with gitignored artifacts: 1"));
    assert!(stdout.contains("healthy"));
    assert!(broken.join("node_modules").is_dir());
    fs::remove_dir_all(root).unwrap();
}
