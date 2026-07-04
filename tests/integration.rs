use std::path::{Path, PathBuf};
use std::process::Command;

fn clync_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clync"))
}

fn run(args: &[&str]) -> (String, String, bool) {
    let output = Command::new(clync_bin())
        .args(args)
        .output()
        .expect("failed to run clync");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir()
            .join("clync-tests")
            .join(name)
            .join(format!("{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn help_works() {
    let (stdout, _, ok) = run(&["--help"]);
    assert!(ok);
    assert!(stdout.contains("clync"));
    assert!(stdout.contains("init"));
    assert!(stdout.contains("push"));
    assert!(stdout.contains("pull"));
    assert!(stdout.contains("sync"));
    assert!(stdout.contains("list"));
    assert!(stdout.contains("join"));
    assert!(stdout.contains("mcp"));
}

#[test]
fn version_works() {
    let (stdout, _, ok) = run(&["--version"]);
    assert!(ok);
    assert!(stdout.contains("clync"));
}

#[test]
fn init_no_encrypt() {
    let dir = TempDir::new("init_no_encrypt");
    let repo = dir.path().join("repo");
    let config_dir = dir.path().join("config");

    let output = Command::new(clync_bin())
        .args(["init", "--no-encrypt", "--repo"])
        .arg(&repo)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("HOME", dir.path().join("home"))
        .output()
        .expect("failed to run clync init");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("encryption: disabled"), "stdout: {stdout}");
    assert!(repo.join(".git").exists());
    assert!(repo.join("sessions").exists());
}
