use std::path::PathBuf;
use std::process::Command;

fn clync() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clync"))
}

fn run_clync(home: &std::path::Path, config: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new(clync())
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", config)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@clync")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@clync")
        .output()
        .unwrap_or_else(|e| panic!("clync {} failed: {e}", args.join(" ")));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "clync {} failed.\nstdout: {stdout}\nstderr: {stderr}",
        args.join(" ")
    );
    stdout
}

fn mode_entry() -> String {
    r#"{"type":"mode","mode":"normal","timestamp":"2026-01-01T00:00:00Z"}"#.to_string()
}

fn msg(uuid: &str, ts: u64, role: &str, content: &str) -> String {
    format!(
        r#"{{"type":"{role}","uuid":"{uuid}","timestamp":{ts},"message":{{"role":"{role}","content":"{content}"}}}}"#
    )
}

#[test]
fn folder_storage_roundtrip() {
    let dir = std::env::temp_dir()
        .join("clync-folder-integration")
        .join(format!("{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }

    let shared_folder = dir.join("shared");
    let machine_a = dir.join("a");
    let machine_b = dir.join("b");

    std::fs::create_dir_all(machine_a.join("home/.claude/projects")).unwrap();
    std::fs::create_dir_all(machine_b.join("home/.claude/projects")).unwrap();

    // Machine A: init with folder storage
    run_clync(
        &machine_a.join("home"),
        &machine_a.join("config"),
        &[
            "init",
            "--no-encrypt",
            "--storage",
            "folder",
            "--repo",
            shared_folder.to_str().unwrap(),
        ],
    );

    assert!(
        !shared_folder.join(".git").exists(),
        "no git for folder storage"
    );
    assert!(
        shared_folder.join("sessions").exists(),
        "sessions dir created"
    );

    // Machine A: write a session and push
    let proj_dir = machine_a.join("home/.claude/projects/proj");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let session_content = format!(
        "{}\n{}\n",
        mode_entry(),
        msg("m1", 100, "user", "hello from A")
    );
    std::fs::write(proj_dir.join("s1.jsonl"), &session_content).unwrap();

    let push_out = run_clync(
        &machine_a.join("home"),
        &machine_a.join("config"),
        &["push", "--no-sync"],
    );
    assert!(push_out.contains("1 sessions"), "push output: {push_out}");

    // Verify manifest exists in shared folder
    assert!(
        shared_folder.join("manifest.json").exists(),
        "manifest written"
    );

    // Machine B: init pointing at same shared folder, then pull
    run_clync(
        &machine_b.join("home"),
        &machine_b.join("config"),
        &[
            "init",
            "--no-encrypt",
            "--storage",
            "folder",
            "--repo",
            shared_folder.to_str().unwrap(),
        ],
    );

    let pull_out = run_clync(
        &machine_b.join("home"),
        &machine_b.join("config"),
        &["pull", "--no-sync"],
    );
    assert!(pull_out.contains("1 new"), "pull output: {pull_out}");

    // Verify session arrived on machine B
    let b_sessions: Vec<_> = walkdir::WalkDir::new(machine_b.join("home/.claude/projects"))
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    assert_eq!(b_sessions.len(), 1, "session synced to machine B");

    let b_content = std::fs::read_to_string(b_sessions[0].path()).unwrap();
    assert!(
        b_content.contains("hello from A"),
        "session content matches"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(feature = "s3")]
fn s3_storage_roundtrip() {
    // Requires MinIO running on localhost:9123 with bucket "clync-test"
    // Start with: docker run -d -p 9123:9000 -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin minio/minio server /data
    // Create bucket: docker exec <id> mc alias set local http://localhost:9000 minioadmin minioadmin && docker exec <id> mc mb local/clync-test

    // Check if MinIO is reachable
    let check = std::process::Command::new("curl")
        .args(["-sf", "http://localhost:9123/minio/health/live"])
        .output();
    if check.is_err() || !check.unwrap().status.success() {
        eprintln!("skipping S3 test: MinIO not running on localhost:9123");
        return;
    }

    let dir = std::env::temp_dir()
        .join("clync-s3-integration")
        .join(format!("{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }

    let machine_a = dir.join("a");
    std::fs::create_dir_all(machine_a.join("home/.claude/projects")).unwrap();
    let config_dir = machine_a.join("config/clync");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Write S3 config manually (init doesn't support S3 interactively)
    let prefix = format!("test-{}", std::process::id());
    let config_content = format!(
        r#"[sync]
claude_dir = "{}"

[sync.storage]
type = "s3"
bucket = "clync-test"
prefix = "{prefix}"
region = "us-east-1"
endpoint = "http://localhost:9123"
access_key = "minioadmin"
secret_key = "minioadmin"

[encryption]
method = "none"

[targets]
sessions = true
memories = false
settings = false
commands = false
skills = false
global_claude_md = false
"#,
        machine_a.join("home/.claude").display()
    );
    std::fs::write(config_dir.join("config.toml"), &config_content).unwrap();

    // Write a session
    let proj_dir = machine_a.join("home/.claude/projects/proj");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let session_content = format!("{}\n{}\n", mode_entry(), msg("m1", 100, "user", "hello S3"));
    std::fs::write(proj_dir.join("s1.jsonl"), &session_content).unwrap();

    // Push
    let push_out = run_clync(
        &machine_a.join("home"),
        &machine_a.join("config"),
        &["push"],
    );
    assert!(
        push_out.contains("1 sessions"),
        "S3 push output: {push_out}"
    );

    // Now set up machine B pointing at same S3 bucket
    let machine_b = dir.join("b");
    std::fs::create_dir_all(machine_b.join("home/.claude/projects")).unwrap();
    let config_dir_b = machine_b.join("config/clync");
    std::fs::create_dir_all(&config_dir_b).unwrap();

    let config_b = format!(
        r#"[sync]
claude_dir = "{}"

[sync.storage]
type = "s3"
bucket = "clync-test"
prefix = "{prefix}"
region = "us-east-1"
endpoint = "http://localhost:9123"
access_key = "minioadmin"
secret_key = "minioadmin"

[encryption]
method = "none"

[targets]
sessions = true
memories = false
settings = false
commands = false
skills = false
global_claude_md = false
"#,
        machine_b.join("home/.claude").display()
    );
    std::fs::write(config_dir_b.join("config.toml"), &config_b).unwrap();

    // Pull
    let pull_out = run_clync(
        &machine_b.join("home"),
        &machine_b.join("config"),
        &["pull"],
    );
    assert!(pull_out.contains("1 new"), "S3 pull output: {pull_out}");

    // Verify session arrived
    let b_sessions: Vec<_> = walkdir::WalkDir::new(machine_b.join("home/.claude/projects"))
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    assert_eq!(b_sessions.len(), 1, "session synced via S3");

    let b_content = std::fs::read_to_string(b_sessions[0].path()).unwrap();
    assert!(b_content.contains("hello S3"), "S3 session content matches");

    std::fs::remove_dir_all(&dir).ok();
}
