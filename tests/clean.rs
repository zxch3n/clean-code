use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::SystemTime,
};

struct Fixture(PathBuf);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "clean-my-code cli {}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let init = Command::new("git").arg("init").arg(&root).output().unwrap();
        assert!(
            init.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        fs::write(root.join(".gitignore"), "node_modules/\ndist/\n").unwrap();
        for name in ["node_modules", "dist", "target", "local-state"] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("data"), "test").unwrap();
        }
        Self(root)
    }

    fn clean(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_clean-my-code"))
            .arg("--root")
            .arg(&self.0)
            .arg("clean")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn assert_untouched(&self) {
        for name in ["node_modules", "dist", "target", "local-state"] {
            assert_eq!(
                fs::read_to_string(self.0.join(name).join("data")).unwrap(),
                "test"
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn dry_run_previews_without_deleting_even_with_yes() {
    let fixture = Fixture::new();
    let output = fixture.clean(&["--older-than", "0", "--min-size", "0", "--dry-run", "--yes"]);
    let stdout = assert_success(&output);
    assert!(stdout.contains("Plan: delete 2 artifact dirs from 1 repos"));
    assert!(stdout.contains("[dry-run]"));
    assert!(stdout.contains("Nothing deleted"));
    fixture.assert_untouched();
}

#[test]
fn yes_deletes_only_ignored_artifacts() {
    let fixture = Fixture::new();
    let output = fixture.clean(&[
        "--older-than",
        "0",
        "--min-size",
        "0",
        "--yes",
        "--threads",
        "2",
    ]);
    assert!(assert_success(&output).contains("Deleted: 2 dirs"));
    assert!(!fixture.0.join("node_modules").exists());
    assert!(!fixture.0.join("dist").exists());
    assert!(fixture.0.join("target/data").is_file());
    assert!(fixture.0.join("local-state/data").is_file());
    assert!(fixture.0.join(".gitignore").is_file());
    assert!(fixture.0.join(".git").is_dir());
}

#[test]
fn noninteractive_cleanup_requires_explicit_yes() {
    let fixture = Fixture::new();
    let output = fixture.clean(&["--older-than", "0", "--min-size", "0"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("use --yes"));
    fixture.assert_untouched();
}

#[test]
fn age_and_repo_total_size_filters_limit_cleanup() {
    let fixture = Fixture::new();
    // Each ignored artifact is 4 bytes; the repo's combined size is 8 bytes.
    for args in [
        vec!["--yes"],
        vec!["--min-size", "0", "--yes"],
        vec!["--older-than", "0", "--min-size", "9B", "--yes"],
    ] {
        assert!(assert_success(&fixture.clean(&args)).contains("Nothing to clean"));
        fixture.assert_untouched();
    }
    assert!(
        assert_success(&fixture.clean(&["--older-than", "0", "--min-size", "8B", "--yes"]))
            .contains("Deleted: 2 dirs")
    );
}

#[test]
fn custom_artifact_list_is_respected() {
    let fixture = Fixture::new();
    let output = fixture.clean(&[
        "--no-default-artifacts",
        "--artifact",
        "dist",
        "--older-than",
        "0",
        "--min-size",
        "0",
        "-y",
    ]);
    assert!(assert_success(&output).contains("Deleted: 1 dirs"));
    assert!(!fixture.0.join("dist").exists());
    assert!(fixture.0.join("node_modules/data").is_file());
}
