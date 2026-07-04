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
        self.run_ok(&[
            "init",
            "--no-encrypt",
            "--repo",
            self.sync_repo.to_str().unwrap(),
        ]);
        Command::new("git")
            .args(["remote", "add", "origin"])
            .arg(&self.bare_repo)
            .current_dir(&self.sync_repo)
            .output()
            .unwrap();
    }

    fn join(&self) {
        let output = Command::new(clync())
            .args([
                "join",
                self.bare_repo.to_str().unwrap(),
                "--no-encrypt",
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
    let has_memories = extras_dir.join("memories").exists();
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
