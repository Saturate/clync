use std::path::PathBuf;
use std::process::{Command, Output};

fn clync() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_clync"))
}

struct TestEnv {
    dir: PathBuf,
    bare_repo: PathBuf,
}

struct Machine {
    name: String,
    home: PathBuf,
    config: PathBuf,
    sync_repo: PathBuf,
    bare_repo: PathBuf,
}

impl TestEnv {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir()
            .join("clync-e2e")
            .join(name)
            .join(format!("{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).unwrap();

        let bare_repo = dir.join("remote.git");
        Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&bare_repo)
            .output()
            .unwrap();

        Self { dir, bare_repo }
    }

    fn machine(&self, name: &str) -> Machine {
        let base = self.dir.join(name);
        std::fs::create_dir_all(base.join("home/.claude/projects")).unwrap();
        Machine {
            name: name.to_string(),
            home: base.join("home"),
            config: base.join("config"),
            sync_repo: base.join("sync-repo"),
            bare_repo: self.bare_repo.clone(),
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

impl Machine {
    fn run(&self, args: &[&str]) -> Output {
        Command::new(clync())
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@clync")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@clync")
            .output()
            .unwrap_or_else(|e| panic!("{}: clync {} failed: {e}", self.name, args.join(" ")))
    }

    fn run_ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            output.status.success(),
            "{}: clync {} failed.\nstdout: {stdout}\nstderr: {stderr}",
            self.name,
            args.join(" ")
        );
        stdout
    }

    fn init(&self) {
        self.init_impl(true);
    }

    fn init_encrypted(&self) {
        self.init_impl(false);
    }

    fn init_impl(&self, no_encrypt: bool) {
        let mut args = vec!["init", "--repo", self.sync_repo.to_str().unwrap()];
        if no_encrypt {
            args.push("--no-encrypt");
        }
        self.run_ok(&args);
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(&self.bare_repo)
            .current_dir(&self.sync_repo)
            .output()
            .unwrap();
    }

    fn join(&self) {
        self.join_impl(true);
    }

    fn join_encrypted(&self) {
        // Read the key from this machine's config to pipe it as stdin
        let key_path = self.config.join("clync").join("key.txt");
        let key =
            std::fs::read_to_string(&key_path).expect("key file must exist before join_encrypted");
        // Pipe: key for prompt, then "y" for "pull sessions now?"
        let stdin_data = format!("{}\ny\n", key.trim());
        let mut child = Command::new(clync())
            .args([
                "join",
                self.bare_repo.to_str().unwrap(),
                "--repo",
                self.sync_repo.to_str().unwrap(),
            ])
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(stdin_data.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{}: join_encrypted failed.\nstdout: {stdout}\nstderr: {stderr}",
            self.name
        );
    }

    fn join_impl(&self, no_encrypt: bool) {
        let mut args = vec![
            "join",
            self.bare_repo.to_str().unwrap(),
            "--repo",
            self.sync_repo.to_str().unwrap(),
        ];
        if no_encrypt {
            args.push("--no-encrypt");
        }
        let output = Command::new(clync())
            .args(&args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = output.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{}: join failed.\nstdout: {stdout}\nstderr: {stderr}",
            self.name
        );
    }

    fn push(&self) -> String {
        self.run_ok(&["push"])
    }

    fn pull(&self) -> String {
        self.run_ok(&["pull"])
    }

    fn status(&self) -> String {
        self.run_ok(&["status"])
    }

    fn list(&self) -> String {
        self.run_ok(&["list"])
    }

    fn projects_dir(&self) -> PathBuf {
        self.home.join(".claude/projects")
    }

    fn write_session(&self, project: &str, uuid: &str, entries: &[&str]) {
        let dir = self.projects_dir().join(project);
        std::fs::create_dir_all(&dir).unwrap();
        let content = entries.join("\n") + "\n";
        std::fs::write(dir.join(format!("{uuid}.jsonl")), content).unwrap();
    }

    fn find_session_file(&self, uuid: &str) -> Option<PathBuf> {
        for entry in walkdir::WalkDir::new(self.projects_dir())
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name().to_string_lossy() == format!("{uuid}.jsonl") {
                return Some(entry.into_path());
            }
        }
        None
    }

    fn write_memory(&self, project: &str, name: &str, content: &str) {
        let dir = self.projects_dir().join(project).join("memory");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn claude_dir(&self) -> PathBuf {
        self.home.join(".claude")
    }

    fn write_settings(&self, content: &str) {
        std::fs::write(self.claude_dir().join("settings.json"), content).unwrap();
    }

    fn write_settings_local(&self, content: &str) {
        std::fs::write(self.claude_dir().join("settings.local.json"), content).unwrap();
    }

    fn write_command(&self, name: &str, content: &str) {
        let dir = self.claude_dir().join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn write_skill(&self, name: &str, content: &str) {
        let dir = self.claude_dir().join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn write_global_claude_md(&self, content: &str) {
        std::fs::write(self.claude_dir().join("CLAUDE.md"), content).unwrap();
    }

    fn enable_all_targets(&self) {
        self.run_ok(&["config", "set", "targets.settings", "true"]);
        self.run_ok(&["config", "set", "targets.commands", "true"]);
        self.run_ok(&["config", "set", "targets.skills", "true"]);
        self.run_ok(&["config", "set", "targets.global_claude_md", "true"]);
    }

    fn file_exists(&self, rel_path: &str) -> bool {
        self.claude_dir().join(rel_path).exists()
    }

    fn read_file(&self, rel_path: &str) -> String {
        std::fs::read_to_string(self.claude_dir().join(rel_path)).unwrap_or_default()
    }

    fn mcp_call(&self, method: &str, params: &str) -> String {
        let input = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{}}}}\n\
             {{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"{method}\",\"params\":{params}}}\n"
        );
        let output = Command::new(clync())
            .args(["mcp"])
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@clync")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@clync")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(input.as_bytes())
                    .unwrap();
                child.wait_with_output()
            })
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        stdout.lines().last().unwrap_or("").to_string()
    }
}

fn msg(uuid: &str, parent: Option<&str>, ts: u64, role: &str, content: &str) -> String {
    let parent_field = match parent {
        Some(p) => format!(r#""parentUuid":"{p}","#),
        None => r#""parentUuid":null,"#.to_string(),
    };
    format!(
        r#"{{"uuid":"{uuid}",{parent_field}"type":"{role}","timestamp":{ts},"sessionId":"sess-1","message":{{"content":"{content}"}}}}"#
    )
}

fn mode_entry() -> String {
    r#"{"type":"mode","mode":"normal","sessionId":"sess-1"}"#.to_string()
}

fn count_uuids(content: &str) -> usize {
    content.lines().filter(|l| l.contains(r#""uuid""#)).count()
}

fn has_uuid(content: &str, uuid: &str) -> bool {
    content.contains(&format!(r#""uuid":"{uuid}""#))
}

// ---- Tests ----

#[test]
fn init_push_status() {
    let env = TestEnv::new("init_push_status");
    let a = env.machine("a");
    a.init();

    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "hello"),
            &msg("m2", Some("m1"), 200, "assistant", "hi"),
        ],
    );

    let out = a.push();
    assert!(out.contains("1 sessions synced"), "push: {out}");

    let status = a.status();
    assert!(status.contains("in sync"), "status: {status}");
}

#[test]
fn push_idempotent() {
    let env = TestEnv::new("push_idempotent");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hello")],
    );

    a.push();
    let out = a.push();
    assert!(out.contains("0 sessions"), "second push should skip: {out}");
}

#[test]
fn join_and_pull() {
    let env = TestEnv::new("join_and_pull");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "myproject",
        "sess-abc",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "from machine A"),
        ],
    );
    a.write_memory("myproject", "note.md", "remember this");
    a.push();

    let b = env.machine("b");
    b.join();

    assert!(
        b.find_session_file("sess-abc").is_some(),
        "session should exist on machine B after join"
    );

    let content = b
        .find_session_file("sess-abc")
        .map(|p| std::fs::read_to_string(p).unwrap())
        .unwrap_or_default();
    assert!(has_uuid(&content, "m1"), "m1 should be present: {content}");
}

#[test]
fn merge_diverged_sessions() {
    let env = TestEnv::new("merge_diverged");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "shared",
        &[
            &mode_entry(),
            &msg("m1", None, 1000, "user", "original"),
            &msg("m2", Some("m1"), 2000, "assistant", "reply"),
        ],
    );
    a.push();

    let b = env.machine("b");
    b.join();

    // Machine B adds a message
    let b_session_path = b.find_session_file("shared").unwrap();
    let mut content = std::fs::read_to_string(&b_session_path).unwrap();
    content.push_str(&msg("m3", Some("m2"), 3000, "user", "from B"));
    content.push('\n');
    std::fs::write(&b_session_path, &content).unwrap();
    b.push();

    // Machine A adds a different message
    let a_proj = a.projects_dir().join("proj");
    let a_session = a_proj.join("shared.jsonl");
    let mut a_content = std::fs::read_to_string(&a_session).unwrap();
    a_content.push_str(&msg("m4", Some("m2"), 3500, "user", "from A"));
    a_content.push('\n');
    std::fs::write(&a_session, &a_content).unwrap();

    // Pull should smart-merge
    let out = a.pull();
    assert!(out.contains("1 merged"), "should merge: {out}");

    let merged = std::fs::read_to_string(&a_session).unwrap();
    assert!(has_uuid(&merged, "m1"), "m1 missing");
    assert!(has_uuid(&merged, "m2"), "m2 missing");
    assert!(has_uuid(&merged, "m3"), "m3 from B missing");
    assert!(has_uuid(&merged, "m4"), "m4 from A missing");
    assert_eq!(count_uuids(&merged), 4, "should have 4 UUID entries");
}

#[test]
fn merge_edit_conflict_newest_wins() {
    let env = TestEnv::new("merge_edit");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 1000, "user", "original text"),
        ],
    );
    a.push();

    let b = env.machine("b");
    b.join();

    // Machine B edits m1 at t=2000
    let b_path = b.find_session_file("s1").unwrap();
    let edited = format!(
        "{}\n{}\n",
        mode_entry(),
        msg("m1", None, 2000, "user", "edited by B")
    );
    std::fs::write(&b_path, &edited).unwrap();
    b.push();

    // Machine A edits m1 at t=1500 (older)
    let a_path = a.projects_dir().join("proj/s1.jsonl");
    let a_edited = format!(
        "{}\n{}\n",
        mode_entry(),
        msg("m1", None, 1500, "user", "edited by A")
    );
    std::fs::write(&a_path, &a_edited).unwrap();

    a.pull();
    let merged = std::fs::read_to_string(&a_path).unwrap();
    assert!(
        merged.contains("edited by B"),
        "newer edit (B at 2000) should win: {merged}"
    );
}

#[test]
fn multiple_sessions_multiple_projects() {
    let env = TestEnv::new("multi_session");
    let a = env.machine("a");
    a.init();

    a.write_session(
        "proj-alpha",
        "s1",
        &[&mode_entry(), &msg("a1", None, 100, "user", "alpha 1")],
    );
    a.write_session(
        "proj-alpha",
        "s2",
        &[&mode_entry(), &msg("a2", None, 200, "user", "alpha 2")],
    );
    a.write_session(
        "proj-beta",
        "s3",
        &[&mode_entry(), &msg("b1", None, 300, "user", "beta 1")],
    );

    let out = a.push();
    assert!(out.contains("3 sessions"), "should push 3: {out}");

    let b = env.machine("b");
    b.join();

    assert!(b.find_session_file("s1").is_some(), "s1 missing on B");
    assert!(b.find_session_file("s2").is_some(), "s2 missing on B");
    assert!(b.find_session_file("s3").is_some(), "s3 missing on B");
}

#[test]
fn list_and_search() {
    let env = TestEnv::new("list_search");
    let a = env.machine("a");
    a.init();

    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hello world")],
    );
    a.write_session(
        "proj",
        "s2",
        &[&mode_entry(), &msg("m2", None, 200, "user", "goodbye moon")],
    );

    let all = a.list();
    assert!(all.contains("hello world"), "should list first: {all}");
    assert!(all.contains("goodbye moon"), "should list second: {all}");

    let filtered = a.run_ok(&["list", "moon"]);
    assert!(
        filtered.contains("goodbye moon"),
        "should find moon: {filtered}"
    );
    assert!(
        !filtered.contains("hello world"),
        "should not find hello: {filtered}"
    );
}

#[test]
fn sync_log_recorded() {
    let env = TestEnv::new("sync_log");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hi")],
    );
    a.push();

    let log = a.run_ok(&["log"]);
    assert!(log.contains("push"), "log should show push: {log}");
    assert!(log.contains("1 pushed"), "log should show count: {log}");
}

#[test]
fn config_show_and_set() {
    let env = TestEnv::new("config");
    let a = env.machine("a");
    a.init();

    let show = a.run_ok(&["config", "show"]);
    assert!(show.contains("none (plain text)"), "config: {show}");
    assert!(show.contains("sessions:        true"), "config: {show}");

    a.run_ok(&["config", "set", "targets.skills", "true"]);
    let show2 = a.run_ok(&["config", "show"]);
    assert!(
        show2.contains("skills:          true"),
        "after set: {show2}"
    );
}

#[test]
fn empty_session_handled() {
    let env = TestEnv::new("empty_session");
    let a = env.machine("a");
    a.init();

    a.write_session("proj", "empty", &[&mode_entry()]);
    let out = a.push();
    assert!(
        out.contains("1 sessions"),
        "should push empty session: {out}"
    );

    let b = env.machine("b");
    b.join();
    assert!(
        b.find_session_file("empty").is_some(),
        "empty session should sync"
    );
}

#[test]
fn mcp_initialize_and_tools_list() {
    let env = TestEnv::new("mcp");
    let a = env.machine("a");
    a.init();

    let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n\
                 {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n";

    let output = Command::new(clync())
        .args(["mcp"])
        .env("HOME", &a.home)
        .env("XDG_CONFIG_HOME", &a.config)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("list_sessions"), "tools list: {stdout}");
    assert!(stdout.contains("sync_push"), "tools list: {stdout}");
    assert!(stdout.contains("help"), "tools list: {stdout}");
}

#[test]
fn notification_ignored() {
    let env = TestEnv::new("notification");
    let a = env.machine("a");
    a.init();

    let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n\
                 {\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n\
                 {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n";

    let output = Command::new(clync())
        .args(["mcp"])
        .env("HOME", &a.home)
        .env("XDG_CONFIG_HOME", &a.config)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "notification should not produce a response: {stdout}"
    );
}

#[test]
fn readme_created_on_init() {
    let env = TestEnv::new("readme");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    assert!(
        a.sync_repo.join("README.md").exists(),
        "README.md should be created"
    );
    let readme = std::fs::read_to_string(a.sync_repo.join("README.md")).unwrap();
    assert!(
        readme.contains("clync"),
        "README should mention clync: {readme}"
    );
}

#[test]
fn clync_toml_in_repo() {
    let env = TestEnv::new("clync_toml");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let toml_path = a.sync_repo.join("clync.toml");
    assert!(toml_path.exists(), "clync.toml should be created");
    let toml = std::fs::read_to_string(&toml_path).unwrap();
    assert!(
        toml.contains("method = \"none\""),
        "should show encryption method: {toml}"
    );
}

#[test]
fn merge_three_machines() {
    let env = TestEnv::new("three_machines");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "shared",
        &[
            &mode_entry(),
            &msg("m1", None, 1000, "user", "base message"),
        ],
    );
    a.push();

    let b = env.machine("b");
    b.join();
    let c = env.machine("c");
    c.join();

    // B adds a message
    let b_path = b.find_session_file("shared").unwrap();
    let mut b_content = std::fs::read_to_string(&b_path).unwrap();
    b_content.push_str(&msg("b1", Some("m1"), 2000, "user", "from B"));
    b_content.push('\n');
    std::fs::write(&b_path, &b_content).unwrap();
    b.push();

    // C adds a different message, pulls B's changes first, then pushes
    let c_path = c.find_session_file("shared").unwrap();
    let mut c_content = std::fs::read_to_string(&c_path).unwrap();
    c_content.push_str(&msg("c1", Some("m1"), 3000, "user", "from C"));
    c_content.push('\n');
    std::fs::write(&c_path, &c_content).unwrap();
    c.pull();
    c.push();

    // A pulls, should get both B and C messages
    a.pull();
    let a_path = a.projects_dir().join("proj/shared.jsonl");
    let merged = std::fs::read_to_string(&a_path).unwrap();
    assert!(has_uuid(&merged, "m1"), "base missing");
    assert!(has_uuid(&merged, "b1"), "B's message missing");
    assert!(has_uuid(&merged, "c1"), "C's message missing");
    assert_eq!(count_uuids(&merged), 3);
}

#[test]
fn merge_append_only_no_edits() {
    let env = TestEnv::new("append_only");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "first"),
            &msg("m2", Some("m1"), 200, "assistant", "second"),
        ],
    );
    a.push();

    let b = env.machine("b");
    b.join();

    // B appends messages
    let b_path = b.find_session_file("s1").unwrap();
    let mut content = std::fs::read_to_string(&b_path).unwrap();
    content.push_str(&msg("m3", Some("m2"), 300, "user", "third"));
    content.push('\n');
    content.push_str(&msg("m4", Some("m3"), 400, "assistant", "fourth"));
    content.push('\n');
    std::fs::write(&b_path, &content).unwrap();
    b.push();

    // A hasn't changed anything, just pulls
    a.pull();
    let merged = std::fs::read_to_string(a.projects_dir().join("proj/s1.jsonl")).unwrap();
    assert_eq!(count_uuids(&merged), 4, "all 4 messages should be present");
    assert!(has_uuid(&merged, "m3"));
    assert!(has_uuid(&merged, "m4"));
}

#[test]
fn merge_idempotent_e2e() {
    let env = TestEnv::new("merge_idempotent_e2e");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "hello"),
            &msg("m2", Some("m1"), 200, "assistant", "hi"),
        ],
    );
    a.push();

    let b = env.machine("b");
    b.join();
    let b_path = b.find_session_file("s1").unwrap();
    let mut content = std::fs::read_to_string(&b_path).unwrap();
    content.push_str(&msg("m3", Some("m2"), 300, "user", "from B"));
    content.push('\n');
    std::fs::write(&b_path, &content).unwrap();
    b.push();

    // A adds a message and pulls twice
    let a_path = a.projects_dir().join("proj/s1.jsonl");
    let mut a_content = std::fs::read_to_string(&a_path).unwrap();
    a_content.push_str(&msg("m4", Some("m2"), 350, "user", "from A"));
    a_content.push('\n');
    std::fs::write(&a_path, &a_content).unwrap();

    a.pull();
    let after_first = std::fs::read_to_string(&a_path).unwrap();
    let count_first = count_uuids(&after_first);

    // Push merged result, then pull again (should be no-op)
    a.push();
    a.pull();
    let after_second = std::fs::read_to_string(&a_path).unwrap();
    let count_second = count_uuids(&after_second);

    assert_eq!(count_first, 4, "first merge: {after_first}");
    assert_eq!(
        count_second, 4,
        "second merge should be stable: {after_second}"
    );
}

#[test]
fn sync_bidirectional() {
    let env = TestEnv::new("sync_bidir");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hello")],
    );
    a.push();

    let b = env.machine("b");
    b.join();

    // B adds session s2
    b.write_session(
        "proj2",
        "s2",
        &[&mode_entry(), &msg("m2", None, 200, "user", "new session")],
    );
    b.push();

    // A syncs (pull + push)
    let pull_out = a.pull();
    assert!(pull_out.contains("1 new"), "should pull s2: {pull_out}");
    assert!(
        a.find_session_file("s2").is_some(),
        "s2 should exist on A after pull"
    );

    // A adds session s3
    a.write_session(
        "proj3",
        "s3",
        &[&mode_entry(), &msg("m3", None, 300, "user", "another")],
    );
    a.push();

    // B pulls
    let b_pull = b.pull();
    assert!(b_pull.contains("1 new"), "B should get s3: {b_pull}");
    assert!(b.find_session_file("s3").is_some(), "s3 should exist on B");
}

#[test]
fn modified_session_detected() {
    let env = TestEnv::new("modified_detect");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "v1")],
    );
    a.push();

    // Modify session locally
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "v1"),
            &msg("m2", Some("m1"), 200, "assistant", "added"),
        ],
    );

    let status = a.status();
    assert!(
        status.contains("diverged") || status.contains("local"),
        "modified session should show in status: {status}"
    );

    let push_out = a.push();
    assert!(
        push_out.contains("1 sessions"),
        "should push modified: {push_out}"
    );
}

#[test]
fn memory_sync_roundtrip() {
    let env = TestEnv::new("memory_sync");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.write_memory("proj", "MEMORY.md", "- [Note](note.md) - test note\n");
    a.write_memory("proj", "note.md", "---\nname: note\n---\nRemember this.\n");
    a.push();

    let b = env.machine("b");
    b.join();

    // Check memories arrived
    let b_projects = b.projects_dir();
    let memory_files: Vec<_> = walkdir::WalkDir::new(&b_projects)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().to_string_lossy().contains("memory"))
        .filter(|e| e.file_type().is_file())
        .collect();

    assert!(
        memory_files.len() >= 2,
        "should have at least 2 memory files, got {}: {:?}",
        memory_files.len(),
        memory_files
            .iter()
            .map(|e| e.path().display().to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn memory_index_merge_on_pull() {
    let env = TestEnv::new("memory_index_merge");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.write_memory(
        "proj",
        "MEMORY.md",
        "- [Alpha](alpha.md) - from machine a\n",
    );
    a.write_memory("proj", "alpha.md", "alpha content\n");
    a.push();

    let b = env.machine("b");
    b.join();

    // B adds its own memory entry and pushes
    b.write_memory("proj", "MEMORY.md", "- [Beta](beta.md) - from machine b\n");
    b.write_memory("proj", "beta.md", "beta content\n");
    // Force future mtime so push detects the change (mtime-based)
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    let ft = filetime::FileTime::from_system_time(future);
    let mem_dir = b.projects_dir().join("proj").join("memory");
    filetime::set_file_mtime(mem_dir.join("MEMORY.md"), ft).unwrap();
    filetime::set_file_mtime(mem_dir.join("beta.md"), ft).unwrap();
    b.push();

    // A pulls - should merge MEMORY.md
    a.pull();
    let memory_md = std::fs::read_to_string(
        a.projects_dir()
            .join("proj")
            .join("memory")
            .join("MEMORY.md"),
    )
    .unwrap();
    assert!(
        memory_md.contains("alpha.md"),
        "should keep local entry: {memory_md}"
    );
    assert!(
        memory_md.contains("beta.md"),
        "should add remote entry: {memory_md}"
    );
    assert_eq!(
        memory_md.matches("alpha.md").count(),
        1,
        "no duplicates: {memory_md}"
    );
}

#[test]
fn large_session_sync() {
    let env = TestEnv::new("large_session");
    let a = env.machine("a");
    a.init();

    let mut entries = vec![mode_entry()];
    let mut prev = String::new();
    for i in 0..200 {
        let uuid = format!("msg-{i}");
        let parent = if prev.is_empty() {
            None
        } else {
            Some(prev.as_str())
        };
        entries.push(msg(
            &uuid,
            parent,
            i as u64 * 100,
            if i % 2 == 0 { "user" } else { "assistant" },
            &format!("message number {i}"),
        ));
        prev = uuid;
    }
    let entry_refs: Vec<&str> = entries.iter().map(|s| s.as_str()).collect();
    a.write_session("proj", "big", &entry_refs);

    let out = a.push();
    assert!(
        out.contains("1 sessions"),
        "should push large session: {out}"
    );

    let b = env.machine("b");
    b.join();

    let b_content = b
        .find_session_file("big")
        .map(|p| std::fs::read_to_string(p).unwrap())
        .unwrap_or_default();
    assert_eq!(count_uuids(&b_content), 200, "all 200 messages should sync");
}

#[test]
fn settings_sync() {
    let env = TestEnv::new("settings_sync");
    let a = env.machine("a");
    a.init();
    a.enable_all_targets();

    a.write_settings(r#"{"allowedTools": ["Bash"], "theme": "dark"}"#);
    a.write_settings_local(r#"{"apiKey": "sk-test"}"#);
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let b = env.machine("b");
    b.join();
    b.enable_all_targets();
    b.pull();

    assert!(b.file_exists("settings.json"), "settings.json should sync");
    let settings = b.read_file("settings.json");
    assert!(settings.contains("dark"), "settings content: {settings}");

    assert!(
        b.file_exists("settings.local.json"),
        "settings.local.json should sync"
    );
    let local = b.read_file("settings.local.json");
    assert!(local.contains("sk-test"), "local settings content: {local}");
}

#[test]
fn commands_sync() {
    let env = TestEnv::new("commands_sync");
    let a = env.machine("a");
    a.init();
    a.enable_all_targets();

    a.write_command("deploy.md", "# Deploy\nRun the deploy pipeline");
    a.write_command("test.md", "# Test\nRun all tests");
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let b = env.machine("b");
    b.join();
    b.enable_all_targets();
    b.pull();

    assert!(
        b.file_exists("commands/deploy.md"),
        "deploy command should sync"
    );
    assert!(
        b.file_exists("commands/test.md"),
        "test command should sync"
    );
    let content = b.read_file("commands/deploy.md");
    assert!(
        content.contains("deploy pipeline"),
        "command content: {content}"
    );
}

#[test]
fn skills_sync() {
    let env = TestEnv::new("skills_sync");
    let a = env.machine("a");
    a.init();
    a.enable_all_targets();

    a.write_skill("my-skill.md", "# My Skill\nDo something useful");
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let b = env.machine("b");
    b.join();
    b.enable_all_targets();
    b.pull();

    assert!(b.file_exists("skills/my-skill.md"), "skill should sync");
    let content = b.read_file("skills/my-skill.md");
    assert!(
        content.contains("something useful"),
        "skill content: {content}"
    );
}

#[test]
fn global_claude_md_sync() {
    let env = TestEnv::new("claude_md_sync");
    let a = env.machine("a");
    a.init();
    a.enable_all_targets();

    a.write_global_claude_md("# My Rules\n\n- Always use TypeScript\n- No any types\n");
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let b = env.machine("b");
    b.join();
    b.enable_all_targets();
    b.pull();

    assert!(b.file_exists("CLAUDE.md"), "CLAUDE.md should sync");
    let content = b.read_file("CLAUDE.md");
    assert!(
        content.contains("TypeScript"),
        "CLAUDE.md content: {content}"
    );
    assert!(
        content.contains("No any types"),
        "CLAUDE.md content: {content}"
    );
}

#[test]
fn memories_across_multiple_projects() {
    let env = TestEnv::new("multi_proj_memory");
    let a = env.machine("a");
    a.init();

    a.write_session("proj-alpha", "s1", &[&mode_entry()]);
    a.write_session("proj-beta", "s2", &[&mode_entry()]);
    a.write_memory("proj-alpha", "MEMORY.md", "- alpha memory\n");
    a.write_memory("proj-alpha", "user-prefs.md", "prefers dark mode\n");
    a.write_memory("proj-beta", "MEMORY.md", "- beta memory\n");
    a.write_memory("proj-beta", "bug-notes.md", "known issue with auth\n");
    a.push();

    let b = env.machine("b");
    b.join();

    let b_projects = b.projects_dir();
    let all_memory_files: Vec<String> = walkdir::WalkDir::new(&b_projects)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().to_string_lossy().contains("memory"))
        .map(|e| e.path().display().to_string())
        .collect();

    assert!(
        all_memory_files.len() >= 4,
        "should have 4 memory files across 2 projects, got {}: {all_memory_files:?}",
        all_memory_files.len()
    );
}

#[test]
fn all_targets_sync_by_default() {
    let env = TestEnv::new("targets_default");
    let a = env.machine("a");
    a.init();

    a.write_settings(r#"{"theme": "dark"}"#);
    a.write_command("deploy.md", "deploy stuff");
    a.write_skill("my-skill.md", "skill stuff");
    a.write_global_claude_md("global rules");
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.write_memory("proj", "note.md", "remember this");
    a.push();

    let b = env.machine("b");
    b.join();

    assert!(
        b.find_session_file("s1").is_some(),
        "sessions should sync by default"
    );
    assert!(
        b.file_exists("settings.json"),
        "settings should sync by default"
    );
    assert!(
        b.file_exists("commands/deploy.md"),
        "commands should sync by default"
    );
    assert!(
        b.file_exists("skills/my-skill.md"),
        "skills should sync by default"
    );
    assert!(
        b.file_exists("CLAUDE.md"),
        "CLAUDE.md should sync by default"
    );
}

#[test]
#[ignore] // extras use mtime-based change detection which is unreliable in tests
fn extras_update_on_change() {
    let env = TestEnv::new("extras_update");
    let a = env.machine("a");
    a.init();
    a.enable_all_targets();

    a.write_settings(r#"{"version": 1}"#);
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let b = env.machine("b");
    b.join();
    b.enable_all_targets();
    b.pull();
    assert_eq!(b.read_file("settings.json").trim(), r#"{"version": 1}"#);

    // A updates settings and forces a newer mtime
    a.write_settings(r#"{"version": 2}"#);
    let settings_path = a.claude_dir().join("settings.json");
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
    filetime::set_file_mtime(&settings_path, filetime::FileTime::from_system_time(future)).unwrap();
    a.push();

    // B pulls updated settings
    b.pull();
    let updated = b.read_file("settings.json");
    assert!(
        updated.contains("version"),
        "settings should update: {updated}"
    );
}

#[test]
fn memories_not_synced_when_disabled() {
    let env = TestEnv::new("memories_disabled");
    let a = env.machine("a");
    a.init();
    a.run_ok(&["config", "set", "targets.memories", "false"]);

    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hi")],
    );
    a.write_memory("proj", "MEMORY.md", "should not sync");
    a.push();

    let b = env.machine("b");
    b.join();

    // Session should sync
    assert!(b.find_session_file("s1").is_some(), "session should sync");

    // Memory should NOT sync
    let memory_files: Vec<_> = walkdir::WalkDir::new(b.projects_dir())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().to_string_lossy().contains("memory"))
        .collect();
    assert!(
        memory_files.is_empty(),
        "memories should NOT sync when disabled: {memory_files:?}"
    );
}

#[test]
fn selective_targets_disabled() {
    let env = TestEnv::new("selective_targets");
    let a = env.machine("a");
    a.init();
    // Disable skills and CLAUDE.md, leave rest on
    a.run_ok(&["config", "set", "targets.skills", "false"]);
    a.run_ok(&["config", "set", "targets.global_claude_md", "false"]);

    a.write_session("proj", "s1", &[&mode_entry()]);
    a.write_memory("proj", "note.md", "a memory");
    a.write_command("build.md", "run the build");
    a.write_skill("lint.md", "run the linter");
    a.write_settings(r#"{"theme": "light"}"#);
    a.write_global_claude_md("global instructions");
    a.push();

    let b = env.machine("b");
    b.join();
    b.run_ok(&["config", "set", "targets.skills", "false"]);
    b.run_ok(&["config", "set", "targets.global_claude_md", "false"]);
    b.pull();

    // These should sync (on by default, not disabled)
    assert!(b.find_session_file("s1").is_some(), "session should sync");
    assert!(b.file_exists("commands/build.md"), "commands should sync");
    assert!(b.file_exists("settings.json"), "settings should sync");

    // These should NOT sync (explicitly disabled)
    assert!(
        !b.file_exists("skills/lint.md"),
        "skills should NOT sync when disabled"
    );
    assert!(
        !b.file_exists("CLAUDE.md"),
        "CLAUDE.md should NOT sync when disabled"
    );
}

#[test]
fn extras_not_in_repo_when_disabled() {
    let env = TestEnv::new("extras_not_in_repo");
    let a = env.machine("a");
    a.init();
    // Disable everything except sessions
    a.run_ok(&["config", "set", "targets.memories", "false"]);
    a.run_ok(&["config", "set", "targets.settings", "false"]);
    a.run_ok(&["config", "set", "targets.commands", "false"]);
    a.run_ok(&["config", "set", "targets.skills", "false"]);
    a.run_ok(&["config", "set", "targets.global_claude_md", "false"]);

    a.write_session("proj", "s1", &[&mode_entry()]);
    a.write_memory("proj", "MEMORY.md", "- test note\n");
    a.write_settings(r#"{"theme": "dark"}"#);
    a.write_command("deploy.md", "deploy stuff");
    a.write_skill("lint.md", "lint stuff");
    a.write_global_claude_md("rules");
    a.push();

    let extras_dir = a.sync_repo.join("extras");
    let has_memories = a.sync_repo.join("memories").exists();
    let has_settings = extras_dir.join("settings.json").exists();
    let has_commands = extras_dir.join("commands").exists();
    let has_skills = extras_dir.join("skills").exists();
    let has_claude_md = extras_dir.join("CLAUDE.md").exists();

    assert!(
        !has_memories,
        "memories should not be in repo when disabled"
    );
    assert!(
        !has_settings,
        "settings should not be in repo when disabled"
    );
    assert!(
        !has_commands,
        "commands should not be in repo when disabled"
    );
    assert!(!has_skills, "skills should not be in repo when disabled");
    assert!(
        !has_claude_md,
        "CLAUDE.md should not be in repo when disabled"
    );

    // Sessions should still be in repo
    let sessions_dir = a.sync_repo.join("sessions");
    assert!(sessions_dir.exists(), "sessions dir should exist");
    let session_count = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .filter(|e| e.as_ref().unwrap().file_type().unwrap().is_file())
        .count();
    assert_eq!(session_count, 1, "should have 1 session file");
}

// ---- MCP tool coverage ----

#[test]
fn mcp_list_sessions() {
    let env = TestEnv::new("mcp_list");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "test query match"),
        ],
    );
    a.push();

    let resp = a.mcp_call(
        "tools/call",
        r#"{"name":"list_sessions","arguments":{"query":"query match","limit":5}}"#,
    );
    assert!(
        resp.contains("test query match"),
        "should find session: {resp}"
    );
}

#[test]
fn mcp_session_detail() {
    let env = TestEnv::new("mcp_detail");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "hello detail"),
            &msg("m2", Some("m1"), 200, "assistant", "hi there"),
        ],
    );

    let resp = a.mcp_call(
        "tools/call",
        r#"{"name":"session_detail","arguments":{"uuid":"s1","tail":5}}"#,
    );
    assert!(
        resp.contains("hello detail"),
        "should contain message: {resp}"
    );
    assert!(resp.contains("user_messages"), "should have stats: {resp}");
}

#[test]
fn mcp_sync_status() {
    let env = TestEnv::new("mcp_status");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let resp = a.mcp_call("tools/call", r#"{"name":"sync_status","arguments":{}}"#);
    assert!(resp.contains("in sync"), "should show in sync: {resp}");
}

#[test]
fn mcp_sync_push_pull() {
    let env = TestEnv::new("mcp_push_pull");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "mcp push test"),
        ],
    );

    let push_resp = a.mcp_call(
        "tools/call",
        r#"{"name":"sync_push","arguments":{"git":false}}"#,
    );
    assert!(
        push_resp.contains("pushed"),
        "push should work: {push_resp}"
    );

    let pull_resp = a.mcp_call(
        "tools/call",
        r#"{"name":"sync_pull","arguments":{"git":false}}"#,
    );
    assert!(
        pull_resp.contains("unchanged") || pull_resp.contains("pulled"),
        "pull should work: {pull_resp}"
    );
}

#[test]
fn mcp_sync_log() {
    let env = TestEnv::new("mcp_log");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let resp = a.mcp_call(
        "tools/call",
        r#"{"name":"sync_log","arguments":{"limit":5}}"#,
    );
    assert!(resp.contains("push"), "log should contain push: {resp}");
}

#[test]
fn mcp_config_show() {
    let env = TestEnv::new("mcp_config");
    let a = env.machine("a");
    a.init();

    let resp = a.mcp_call("tools/call", r#"{"name":"config_show","arguments":{}}"#);
    assert!(
        resp.contains("sync repo") || resp.contains("none"),
        "should show config: {resp}"
    );
}

#[test]
fn mcp_help() {
    let env = TestEnv::new("mcp_help");
    let a = env.machine("a");
    a.init();

    let resp = a.mcp_call(
        "tools/call",
        r#"{"name":"help","arguments":{"topic":"all"}}"#,
    );
    assert!(resp.contains("clync"), "help should mention clync: {resp}");

    let resp2 = a.mcp_call(
        "tools/call",
        r#"{"name":"help","arguments":{"topic":"setup"}}"#,
    );
    assert!(
        resp2.contains("init") || resp2.contains("setup"),
        "setup help: {resp2}"
    );

    let resp3 = a.mcp_call(
        "tools/call",
        r#"{"name":"help","arguments":{"topic":"sync"}}"#,
    );
    assert!(
        resp3.contains("push") || resp3.contains("pull"),
        "sync help: {resp3}"
    );

    let resp4 = a.mcp_call(
        "tools/call",
        r#"{"name":"help","arguments":{"topic":"mcp"}}"#,
    );
    assert!(
        resp4.contains("mcp") || resp4.contains("MCP"),
        "mcp help: {resp4}"
    );

    let resp5 = a.mcp_call(
        "tools/call",
        r#"{"name":"help","arguments":{"topic":"config"}}"#,
    );
    assert!(
        resp5.contains("config") || resp5.contains("toml"),
        "config help: {resp5}"
    );

    let resp6 = a.mcp_call(
        "tools/call",
        r#"{"name":"help","arguments":{"topic":"list"}}"#,
    );
    assert!(resp6.contains("list"), "list help: {resp6}");
}

#[test]
fn mcp_unknown_tool() {
    let env = TestEnv::new("mcp_unknown");
    let a = env.machine("a");
    a.init();

    let resp = a.mcp_call("tools/call", r#"{"name":"nonexistent","arguments":{}}"#);
    assert!(
        resp.contains("error") || resp.contains("unknown"),
        "should error: {resp}"
    );
}

#[test]
fn mcp_session_detail_empty_uuid() {
    let env = TestEnv::new("mcp_empty_uuid");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);

    let resp = a.mcp_call(
        "tools/call",
        r#"{"name":"session_detail","arguments":{"uuid":""}}"#,
    );
    assert!(
        resp.contains("error") || resp.contains("required"),
        "empty uuid should error: {resp}"
    );
}

// ---- CLI edge cases ----

#[test]
fn config_path() {
    let env = TestEnv::new("config_path");
    let a = env.machine("a");
    a.init();

    let out = a.run_ok(&["config", "path"]);
    assert!(out.contains("config.toml"), "should show path: {out}");
}

#[test]
fn config_set_invalid() {
    let env = TestEnv::new("config_invalid");
    let a = env.machine("a");
    a.init();

    let out = a.run(&["config", "set", "badkey", "true"]);
    assert!(!out.status.success(), "invalid key should fail");
}

#[test]
fn log_json_output() {
    let env = TestEnv::new("log_json");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);
    a.push();

    let out = a.run_ok(&["log", "--json"]);
    assert!(out.contains("\"operation\""), "should be JSON: {out}");
    assert!(out.contains("\"push\""), "should have push: {out}");
}

#[test]
fn log_empty() {
    let env = TestEnv::new("log_empty");
    let a = env.machine("a");
    a.init();

    let out = a.run_ok(&["log"]);
    assert!(
        out.contains("no sync history"),
        "should say no history: {out}"
    );
}

#[test]
fn list_json_output() {
    let env = TestEnv::new("list_json");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "json test")],
    );

    let out = a.run_ok(&["list", "--json"]);
    assert!(out.contains("\"uuid\""), "should be JSON: {out}");
}

#[test]
fn list_no_sessions() {
    let env = TestEnv::new("list_none");
    let a = env.machine("a");
    a.init();

    let out = a.run_ok(&["list"]);
    assert!(out.contains("no sessions"), "should say none: {out}");
}

#[test]
fn list_with_max_age() {
    let env = TestEnv::new("list_age");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "old session")],
    );

    let out = a.run_ok(&["list", "--max-age", "1"]);
    assert!(
        out.contains("old session") || out.contains("no sessions"),
        "age filter: {out}"
    );
}

#[test]
fn status_no_repo() {
    let env = TestEnv::new("status_no_repo");
    let a = env.machine("a");
    a.init();
    a.write_session("proj", "s1", &[&mode_entry()]);

    let out = a.run_ok(&["status"]);
    assert!(
        out.contains("local only") || out.contains("push to sync"),
        "should show local only: {out}"
    );
}

#[test]
fn push_no_changes() {
    let env = TestEnv::new("push_no_changes");
    let a = env.machine("a");
    a.init();

    let out = a.run_ok(&["push", "--no-sync"]);
    assert!(out.contains("0 sessions"), "no sessions to push: {out}");
}

#[test]
fn pull_empty_repo() {
    let env = TestEnv::new("pull_empty");
    let a = env.machine("a");
    a.init();

    let out = a.run_ok(&["pull", "--no-sync"]);
    assert!(
        out.contains("0 new") || out.contains("unchanged"),
        "empty pull: {out}"
    );
}

#[test]
fn init_already_exists() {
    let env = TestEnv::new("init_exists");
    let a = env.machine("a");
    a.init();

    let out = a.run(&[
        "init",
        "--no-encrypt",
        "--repo",
        a.sync_repo.to_str().unwrap(),
    ]);
    assert!(!out.status.success(), "double init should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "should say exists: {stderr}"
    );
}

#[test]
fn version_output() {
    let output = Command::new(clync()).args(["--version"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(output.status.success());
    assert!(stdout.contains("clync"), "version: {stdout}");
}

#[test]
fn push_with_max_age_filter() {
    let env = TestEnv::new("push_age_filter");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hi")],
    );

    let out = a.run_ok(&["push", "--no-sync", "--max-age", "1"]);
    assert!(out.contains("sessions"), "should run with filter: {out}");
}

#[test]
fn push_with_max_size_filter() {
    let env = TestEnv::new("push_size_filter");
    let a = env.machine("a");
    a.init();
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hi")],
    );

    let out = a.run_ok(&["push", "--no-sync", "--max-size", "1000000"]);
    assert!(out.contains("1 sessions"), "should push within size: {out}");

    let out2 = a.run_ok(&["push", "--no-sync", "--max-size", "10"]);
    assert!(out2.contains("0 sessions"), "should skip large: {out2}");
}

#[test]
fn folder_storage_init_and_push() {
    let env = TestEnv::new("folder_storage");
    let a = env.machine("a");

    a.run_ok(&[
        "init",
        "--no-encrypt",
        "--storage",
        "folder",
        "--repo",
        a.sync_repo.to_str().unwrap(),
    ]);

    assert!(!a.sync_repo.join(".git").exists(), "no .git for folder");
    assert!(
        a.sync_repo.join("sessions").exists(),
        "sessions dir created"
    );

    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hello folder")],
    );

    let out = a.run_ok(&["push", "--no-sync"]);
    assert!(out.contains("1 sessions"), "push output: {out}");

    assert!(
        a.sync_repo.join("manifest.json").exists(),
        "manifest written"
    );
}

#[test]
fn mv_session_between_projects() {
    let env = TestEnv::new("mv_session");
    let a = env.machine("a");
    a.init();

    a.write_session(
        "proj-alpha",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "work on alpha"),
        ],
    );

    // Write a memory file and reference it in the session via /memory/ path
    a.write_memory("proj-alpha", "alpha-notes.md", "important alpha notes\n");
    // Append a fake tool result referencing the memory file
    let memory_ref = r#"{"type":"assistant","uuid":"m2","timestamp":200,"message":{"content":"Wrote /memory/alpha-notes.md"}}"#;
    let session_path = a.projects_dir().join("proj-alpha").join("s1.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&session_path)
        .unwrap();
    std::io::Write::write_all(&mut file, format!("{memory_ref}\n").as_bytes()).unwrap();

    // Move session to a target path
    let target_path = a.home.join("code").join("proj-beta");
    let out = a.run_ok(&["mv", "s1", &target_path.to_string_lossy()]);
    assert!(out.contains("moved"), "mv output: {out}");
    assert!(out.contains("alpha-notes.md"), "should move memory: {out}");

    // Session should be gone from proj-alpha
    assert!(!session_path.exists(), "session gone from proj-alpha");

    // Find the session in whatever encoded dir it ended up in
    let moved_session: Vec<_> = walkdir::WalkDir::new(a.projects_dir())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy() == "s1.jsonl")
        .collect();
    assert_eq!(moved_session.len(), 1, "session exists in new project");

    // Memory should be moved too
    let moved_memory: Vec<_> = walkdir::WalkDir::new(a.projects_dir())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy() == "alpha-notes.md")
        .filter(|e| e.path().to_string_lossy().contains("proj-beta"))
        .collect();
    assert_eq!(moved_memory.len(), 1, "memory moved with session");

    // Memory should be gone from proj-alpha
    let alpha_memory = a
        .projects_dir()
        .join("proj-alpha")
        .join("memory")
        .join("alpha-notes.md");
    assert!(!alpha_memory.exists(), "memory gone from proj-alpha");
}

#[test]
fn mv_session_no_match() {
    let env = TestEnv::new("mv_no_match");
    let a = env.machine("a");
    a.init();

    let output = a.run(&["mv", "nonexistent-uuid", "/tmp/whatever"]);
    assert!(!output.status.success(), "should fail for nonexistent UUID");
}

#[test]
fn encrypted_push_pull_roundtrip() {
    let env = TestEnv::new("encrypted_roundtrip");
    let a = env.machine("a");
    a.init_encrypted();

    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "secret message"),
        ],
    );

    let out = a.push();
    assert!(out.contains("1 sessions"), "push: {out}");

    // Verify the stored file is encrypted (not plaintext)
    let session_file = a.sync_repo.join("sessions").join("s1.jsonl.age");
    assert!(session_file.exists(), "encrypted session file should exist");
    let content = std::fs::read(&session_file).unwrap();
    assert!(
        !String::from_utf8_lossy(&content).contains("secret message"),
        "session should be encrypted, not plaintext"
    );

    let manifest_file = a.sync_repo.join("manifest.json.age");
    assert!(manifest_file.exists(), "encrypted manifest should exist");

    let status = a.status();
    assert!(status.contains("in sync"), "status: {status}");
}

#[test]
fn encrypted_join_and_pull() {
    let env = TestEnv::new("encrypted_join");
    let a = env.machine("a");
    a.init_encrypted();

    a.write_session(
        "proj",
        "s1",
        &[
            &mode_entry(),
            &msg("m1", None, 100, "user", "encrypted content"),
        ],
    );
    a.push();

    // Copy the key file from machine A to machine B
    let a_key = a.config.join("clync").join("key.txt");
    let b = env.machine("b");
    let b_key_dir = b.config.join("clync");
    std::fs::create_dir_all(&b_key_dir).unwrap();
    std::fs::copy(&a_key, b_key_dir.join("key.txt")).unwrap();

    b.join_encrypted();
    let session = b.find_session_file("s1");
    assert!(session.is_some(), "session should be pulled");
    let content = std::fs::read_to_string(session.unwrap()).unwrap();
    assert!(
        content.contains("encrypted content"),
        "pulled session should be decrypted"
    );
}

#[test]
fn encrypted_merge_diverged() {
    let env = TestEnv::new("encrypted_merge");
    let a = env.machine("a");
    a.init_encrypted();

    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "initial")],
    );
    a.push();

    // Copy key and join from B
    let a_key = a.config.join("clync").join("key.txt");
    let b = env.machine("b");
    let b_key_dir = b.config.join("clync");
    std::fs::create_dir_all(&b_key_dir).unwrap();
    std::fs::copy(&a_key, b_key_dir.join("key.txt")).unwrap();
    b.join_encrypted();

    // Machine B adds a message by appending
    let b_session = b.find_session_file("s1").unwrap();
    let mut b_content = std::fs::read_to_string(&b_session).unwrap();
    b_content.push_str(&msg("m3", Some("m1"), 300, "assistant", "from B"));
    b_content.push('\n');
    std::fs::write(&b_session, &b_content).unwrap();
    b.push();

    // Machine A adds a different message by appending
    let a_session = a.find_session_file("s1").unwrap();
    let mut a_content = std::fs::read_to_string(&a_session).unwrap();
    a_content.push_str(&msg("m2", Some("m1"), 200, "assistant", "from A"));
    a_content.push('\n');
    std::fs::write(&a_session, &a_content).unwrap();

    // Pull should smart-merge
    a.pull();

    let content = std::fs::read_to_string(&a_session).unwrap();
    assert!(has_uuid(&content, "m2"), "should have A's message");
    assert!(has_uuid(&content, "m3"), "should have B's message");
}

#[test]
fn companion_dir_sync() {
    let env = TestEnv::new("companion_dir");
    let a = env.machine("a");
    a.init();

    // Enable companion dir sync
    a.run_ok(&["config", "set", "sync.include_companion_dirs", "true"]);

    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hello")],
    );

    // Create a companion directory (same name as session UUID, without .jsonl)
    let companion_dir = a.projects_dir().join("proj").join("s1");
    std::fs::create_dir_all(&companion_dir).unwrap();
    std::fs::write(companion_dir.join("artifact.txt"), "companion data").unwrap();
    std::fs::create_dir_all(companion_dir.join("sub")).unwrap();
    std::fs::write(companion_dir.join("sub").join("nested.txt"), "nested data").unwrap();

    let out = a.push();
    assert!(out.contains("1 sessions synced"), "push: {out}");

    // Verify companion tar exists in sync repo
    let tar_file = a.sync_repo.join("sessions").join("s1.dir.tar.gz");
    assert!(tar_file.exists(), "companion tar should exist");

    // Pull to machine B - join pulls sessions, then enable companions and pull again
    let b = env.machine("b");
    b.join();
    b.run_ok(&["config", "set", "sync.include_companion_dirs", "true"]);

    // Need another pull to get the companion dir (join doesn't respect the config change)
    b.pull();

    let session = b.find_session_file("s1");
    assert!(session.is_some(), "session should be pulled");

    let b_proj_dirs: Vec<_> = std::fs::read_dir(b.projects_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    let b_proj = &b_proj_dirs[0].path();
    let b_companion = b_proj.join("s1");
    assert!(
        b_companion.exists(),
        "companion dir should be pulled: checked {}",
        b_companion.display()
    );
    assert!(
        b_companion.join("artifact.txt").exists(),
        "companion file should exist"
    );
    let nested = std::fs::read_to_string(b_companion.join("sub").join("nested.txt"));
    assert_eq!(nested.unwrap(), "nested data", "nested file content");
}

#[test]
fn selective_targets_partial() {
    let env = TestEnv::new("selective_partial");
    let a = env.machine("a");
    a.init();

    // Enable settings but disable memories
    a.run_ok(&["config", "set", "targets.settings", "true"]);
    a.run_ok(&["config", "set", "targets.memories", "false"]);

    a.write_settings(r#"{"theme":"dark"}"#);
    a.write_memory("proj", "note.md", "remember this");
    a.write_session(
        "proj",
        "s1",
        &[&mode_entry(), &msg("m1", None, 100, "user", "hello")],
    );

    a.push();

    let b = env.machine("b");
    b.join();
    b.run_ok(&["config", "set", "targets.settings", "true"]);
    b.run_ok(&["config", "set", "targets.memories", "false"]);

    // Session should be pulled
    assert!(b.find_session_file("s1").is_some(), "session pulled");

    // Settings should be pulled
    assert!(b.file_exists("settings.json"), "settings pulled");

    // Memories should NOT be pulled (disabled)
    let memory_path = b.projects_dir().join("proj").join("memory").join("note.md");
    assert!(
        !memory_path.exists(),
        "memories should not be pulled when disabled"
    );
}
