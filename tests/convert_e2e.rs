//! Converts sessions between Claude Code, opencode and pi in an isolated HOME,
//! then opens the results with the real tools.
//!
//! Needs `opencode` and `pi` on PATH, so it is ignored by default:
//!     cargo test --test convert_e2e -- --ignored --nocapture
//!
//! With `CLYNC_E2E_CLAUDE=1` it also resumes the converted Claude session with
//! `claude -p` (one small Haiku call). Claude's login lives in the real config,
//! so that step copies the session into the real `~/.claude/projects` under a
//! unique temp project folder and removes the folder afterwards.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

const CLAUDE_USER_TEXT: &str = "What is in the fixture repo?";
const CLAUDE_TOOL_OUTPUT: &str = "fixture-output-123";
const PI_USER_TEXT: &str = "pi fixture question";
const PI_TOOL_OUTPUT: &str = "pi-tool-output-456";
const HOME_VAR: &str = "HOME";
const CLAUDE_OPT_IN_VAR: &str = "CLYNC_E2E_CLAUDE";

struct Env {
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Env {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join("clync-convert-e2e").join(format!(
            "{}-{}",
            std::process::id(),
            nanos
        ));
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        // macOS hands out /var/... temp paths that resolve to /private/var/...;
        // tools record the resolved path, so use it everywhere.
        let project = project.canonicalize().unwrap();
        let env = Env {
            root,
            home: home.canonicalize().unwrap(),
            project,
        };
        env.git(&["init", "-q"]);
        env.git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        // opencode derives the project ID from the origin remote when there is one.
        env.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/example/clync-fixture.git",
        ]);
        env
    }

    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.project)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            stderr(&out)
        );
    }

    /// A command that only sees the sandbox HOME and XDG dirs.
    fn cmd(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_STATE_HOME", self.home.join(".local/state"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_CACHE_HOME", self.home.join(".cache"))
            .env("PI_OFFLINE", "1");
        cmd
    }

    fn convert(&self, from: &str, to: &str, extra: &[&str]) -> (u64, u64) {
        let out = self
            .cmd(env!("CARGO_BIN_EXE_clync"))
            .args(["convert", "--from", from, "--to", to, "--all"])
            .args(extra)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "convert {from} -> {to} failed\n{stdout}\n{}",
            stderr(&out)
        );
        let done = stdout
            .lines()
            .find(|l| l.starts_with("Done:"))
            .unwrap_or_default();
        let nums: Vec<u64> = done
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|s| s.parse().ok())
            .collect();
        (nums[0], nums[1])
    }

    fn opencode_column(&self, sql: &str) -> Vec<String> {
        let conn = rusqlite::Connection::open(self.home.join(".local/share/opencode/opencode.db"))
            .unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn require(program: &str) {
    let found = Command::new("which")
        .arg(program)
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(found, "{program} must be on PATH for this test");
}

fn write_jsonl(path: &Path, lines: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(path, body).unwrap();
}

/// A Claude session with text, thinking, a completed tool call, a failed one,
/// and a final call that never got a result (interrupted turn).
fn write_claude_fixture(env: &Env) {
    let cwd = env.project.to_string_lossy().to_string();
    let folder = claude_folder(&env.project);
    let sid = "aaaaaaaa-1111-2222-3333-444444444444";
    let entry = |uuid: &str, parent: Option<&str>, ty: &str, ts: &str, message: Value| {
        json!({"type": ty, "uuid": uuid, "parentUuid": parent, "sessionId": sid, "cwd": cwd,
               "isSidechain": false, "timestamp": ts, "message": message})
    };
    let assistant = |content: Value| json!({"role": "assistant", "model": "claude-haiku-5-5", "content": content});
    let tool_result = |id: &str, out: &str, err: bool| json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": out, "is_error": err}]});
    write_jsonl(
        &env.home
            .join(".claude/projects")
            .join(folder)
            .join(format!("{sid}.jsonl")),
        &[
            json!({"type": "ai-title", "aiTitle": "Fixture session", "sessionId": sid}),
            entry(
                "u1",
                None,
                "user",
                "2026-01-01T10:00:00.000Z",
                json!({"role": "user", "content": CLAUDE_USER_TEXT}),
            ),
            entry(
                "a1",
                Some("u1"),
                "assistant",
                "2026-01-01T10:00:01.000Z",
                assistant(
                    json!([{"type": "thinking", "thinking": "pondering", "signature": "sig"}]),
                ),
            ),
            entry(
                "a2",
                Some("a1"),
                "assistant",
                "2026-01-01T10:00:02.000Z",
                assistant(json!([{"type": "text", "text": "Let me look."}])),
            ),
            entry(
                "a3",
                Some("a2"),
                "assistant",
                "2026-01-01T10:00:03.000Z",
                assistant(
                    json!([{"type": "tool_use", "id": "toolu_ok", "name": "Bash", "input": {"command": "ls"}}]),
                ),
            ),
            entry(
                "r1",
                Some("a3"),
                "user",
                "2026-01-01T10:00:04.000Z",
                tool_result("toolu_ok", CLAUDE_TOOL_OUTPUT, false),
            ),
            entry(
                "a4",
                Some("r1"),
                "assistant",
                "2026-01-01T10:00:05.000Z",
                assistant(
                    json!([{"type": "tool_use", "id": "toolu_err", "name": "Read", "input": {"file_path": "/nope"}}]),
                ),
            ),
            entry(
                "r2",
                Some("a4"),
                "user",
                "2026-01-01T10:00:06.000Z",
                tool_result("toolu_err", "File does not exist.", true),
            ),
            entry(
                "a5",
                Some("r2"),
                "assistant",
                "2026-01-01T10:00:07.000Z",
                assistant(json!([{"type": "text", "text": "The repo is empty."}])),
            ),
            entry(
                "u2",
                Some("a5"),
                "user",
                "2026-01-01T10:00:08.000Z",
                json!({"role": "user", "content": "Run it again"}),
            ),
            entry(
                "a6",
                Some("u2"),
                "assistant",
                "2026-01-01T10:00:09.000Z",
                assistant(
                    json!([{"type": "tool_use", "id": "toolu_lost", "name": "Bash", "input": {"command": "ls"}}]),
                ),
            ),
        ],
    );
}

/// A pi session shaped like what pi 1.x writes.
fn write_pi_fixture(env: &Env) {
    let cwd = env.project.to_string_lossy().to_string();
    let folder = format!("--{}--", cwd.trim_start_matches('/').replace('/', "-"));
    let msg = |id: &str, parent: &str, ts: &str, message: Value| json!({"type": "message", "id": id, "parentId": parent, "timestamp": ts, "message": message});
    write_jsonl(
        &env.home
            .join(".pi/agent/sessions")
            .join(folder)
            .join("2026-01-02T10-00-00-000Z_bbbbbbbb-1111-2222-3333-444444444444.jsonl"),
        &[
            json!({"type": "session", "version": 3, "id": "bbbbbbbb-1111-2222-3333-444444444444",
                   "timestamp": "2026-01-02T10:00:00.000Z", "cwd": cwd}),
            json!({"type": "model_change", "id": "m0000001", "parentId": null,
                   "timestamp": "2026-01-02T10:00:00.000Z", "provider": "ollama", "modelId": "qwen3"}),
            msg(
                "p0000001",
                "m0000001",
                "2026-01-02T10:00:01.000Z",
                json!({"role": "user", "content": [{"type": "text", "text": PI_USER_TEXT}], "timestamp": 1}),
            ),
            msg(
                "p0000002",
                "p0000001",
                "2026-01-02T10:00:02.000Z",
                json!({"role": "assistant", "model": "qwen3", "provider": "ollama", "stopReason": "toolUse",
                       "content": [{"type": "thinking", "thinking": "hmm"},
                                   {"type": "toolCall", "id": "call_1|fc_item_1", "name": "bash", "arguments": {"command": "pwd"}}]}),
            ),
            msg(
                "p0000003",
                "p0000002",
                "2026-01-02T10:00:03.000Z",
                json!({"role": "toolResult", "toolCallId": "call_1|fc_item_1", "toolName": "bash",
                       "content": [{"type": "text", "text": PI_TOOL_OUTPUT}], "isError": false}),
            ),
            msg(
                "p0000004",
                "p0000003",
                "2026-01-02T10:00:04.000Z",
                json!({"role": "assistant", "model": "qwen3", "provider": "ollama", "stopReason": "stop",
                       "content": [{"type": "text", "text": "Done."}]}),
            ),
        ],
    );
}

fn files_with(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && std::fs::read_to_string(path).unwrap().contains(needle)
        {
            found.push(path.to_path_buf());
        }
    }
    found
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// opencode must decode every converted session and list it under the project.
fn check_opencode(env: &Env, opencode_project: &str) {
    let ids = env.opencode_column("SELECT id FROM session_v2 WHERE version = 'clync'");
    assert_eq!(
        ids.len(),
        2,
        "expected one opencode session per origin, got {ids:?}"
    );

    let list = env
        .cmd("opencode")
        .args(["session", "list", "--standalone"])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&list.stdout).to_string();
    for id in &ids {
        assert!(
            listed.contains(id.as_str()),
            "opencode session list is missing {id}:\n{listed}"
        );
        let out = env
            .cmd("opencode")
            .args(["session", "export", id.as_str(), "--standalone"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "opencode export {id} failed: {text}{}",
            stderr(&out)
        );
        let export: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("export {id} isn't JSON ({e}): {text}"));
        assert_eq!(export["info"]["id"], id.as_str());
        assert!(
            text.contains(CLAUDE_TOOL_OUTPUT) || text.contains(PI_TOOL_OUTPUT),
            "opencode export {id} lost the tool output"
        );
        // opencode allocates the next message seq from event_sequence, so it
        // must sit at the last written seq for the session to be continuable.
        let watermark = env.opencode_column(&format!(
            "SELECT CAST((SELECT seq FROM event_sequence WHERE aggregate_id = '{id}') = \
             (SELECT max(seq) FROM session_message WHERE session_id = '{id}') AS TEXT)"
        ));
        assert_eq!(watermark, ["1"], "event_sequence watermark wrong for {id}");
    }

    // The fixture repo has an origin remote, so opencode keys its project by the
    // remote's hash; converted sessions must land in that same project.
    let used =
        env.opencode_column("SELECT DISTINCT project_id FROM session_v2 WHERE version = 'clync'");
    assert_eq!(
        used,
        [opencode_project],
        "converted sessions landed in another project"
    );
}

/// Open the session in pi's TUI under a pseudo-terminal. With stdin at EOF pi
/// quits right after the first render, printing how to resume; a session it
/// can't render crashes first.
fn check_pi_opens(env: &Env, file: &Path) {
    let log = env.root.join("pi-tty.log");
    let mut cmd = env.cmd("script");
    if cfg!(target_os = "macos") {
        cmd.arg("-q")
            .arg(&log)
            .args(["pi", "--offline", "--session"])
            .arg(file);
    } else {
        let inner = format!("pi --offline --session '{}'", file.display());
        cmd.args(["-q", "-e", "-c", &inner]).arg(&log);
    }
    let status = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    let screen = String::from_utf8_lossy(&std::fs::read(&log).unwrap()).to_string();
    assert!(
        status.success() && screen.contains("To resume this session"),
        "pi failed to open {}:\n{}",
        file.display(),
        screen
    );
}

/// pi must load each converted file and resume from its last message.
fn check_pi(env: &Env) {
    let files = files_with(&env.home.join(".pi"), "\"customType\":\"clync-provenance\"");
    // The pi fixture is native, so only the Claude-origin session is converted.
    assert_eq!(
        files.len(),
        1,
        "expected one converted pi session, got {files:?}"
    );
    for file in files {
        let html = env.root.join("pi-export.html");
        let out = env
            .cmd("pi")
            .arg("--export")
            .arg(&file)
            .arg(&html)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "pi export of {} failed: {}",
            file.display(),
            stderr(&out)
        );

        let page = std::fs::read_to_string(&html).unwrap();
        let start_tag = r#"<script id="session-data" type="application/json">"#;
        let start = page.find(start_tag).expect("pi export has no session data") + start_tag.len();
        let end = start + page[start..].find("</script>").unwrap();
        let mut decode = Command::new("base64")
            .arg("--decode")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        decode
            .stdin
            .take()
            .unwrap()
            .write_all(page[start..end].trim().as_bytes())
            .unwrap();
        let data: Value =
            serde_json::from_slice(&decode.wait_with_output().unwrap().stdout).unwrap();

        let entries = read_jsonl(&file);
        let last_id = entries.iter().rev().find_map(|e| e["id"].as_str()).unwrap();
        assert_eq!(
            data["leafId"],
            last_id,
            "pi picked the wrong leaf for {}",
            file.display()
        );
        assert_eq!(data["entries"].as_array().unwrap().len(), entries.len() - 1);
        check_pi_opens(env, &file);
    }
}

/// Claude Code needs one root, an unbroken parentUuid chain, a leaf pointer,
/// and every tool_use answered, or resuming fails.
fn check_claude_structure(file: &Path) {
    let entries = read_jsonl(file);
    let chain: Vec<&Value> = entries.iter().filter(|e| e["uuid"].is_string()).collect();
    let ids: HashSet<&str> = chain.iter().filter_map(|e| e["uuid"].as_str()).collect();
    assert_eq!(
        chain.iter().filter(|e| e["parentUuid"].is_null()).count(),
        1,
        "{}",
        file.display()
    );
    for e in &chain {
        if let Some(parent) = e["parentUuid"].as_str() {
            assert!(
                ids.contains(parent),
                "dangling parentUuid in {}",
                file.display()
            );
        }
    }
    let leaf = entries.iter().find(|e| e["type"] == "last-prompt").unwrap()["leafUuid"].clone();
    assert_eq!(leaf, chain.last().unwrap()["uuid"]);

    let blocks = |ty: &str, key: &str| -> Vec<String> {
        let mut v: Vec<String> = chain
            .iter()
            .filter_map(|e| e["message"]["content"].as_array())
            .flatten()
            .filter(|b| b["type"] == ty)
            .filter_map(|b| b[key].as_str().map(str::to_string))
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        blocks("tool_use", "id"),
        blocks("tool_result", "tool_use_id")
    );
}

/// Resume the converted session in real Claude Code. Opt-in: costs one API call.
fn check_claude_resume(env: &Env, file: &Path) {
    let real_home = dirs_home();
    let folder = claude_folder(&env.project);
    let real_dir = real_home.join(".claude/projects").join(&folder);
    assert!(
        !real_dir.exists(),
        "refusing to touch existing {}",
        real_dir.display()
    );
    std::fs::create_dir_all(&real_dir).unwrap();
    std::fs::copy(file, real_dir.join(file.file_name().unwrap())).unwrap();
    let session_id = file.file_stem().unwrap().to_string_lossy().to_string();

    let out = Command::new("claude")
        .current_dir(&env.project)
        .args([
            "-p",
            "--resume",
            &session_id,
            "--model",
            "haiku",
            "Reply with exactly: RESUMED",
        ])
        .output();
    std::fs::remove_dir_all(&real_dir).ok();

    let out = out.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "claude --resume failed: {stdout}{}",
        stderr(&out)
    );
    assert!(
        stdout.contains("RESUMED"),
        "unexpected claude reply: {stdout}"
    );
}

fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var_os(HOME_VAR).expect("HOME is not set"))
}

#[test]
#[ignore = "needs opencode and pi installed; run with --ignored"]
fn converted_sessions_open_in_each_tool() {
    require("opencode");
    require("pi");
    let env = Env::new();

    // opencode creates its database and registers the project on first run.
    let init = env
        .cmd("opencode")
        .args(["session", "list", "--standalone"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "opencode init failed: {}",
        stderr(&init)
    );
    let opencode_project = env.opencode_column(&format!(
        "SELECT id FROM project WHERE worktree = '{}'",
        env.project.display()
    ));
    assert_eq!(
        opencode_project.len(),
        1,
        "opencode didn't register the fixture repo"
    );

    write_claude_fixture(&env);
    write_pi_fixture(&env);

    // Each step's (converted, skipped) shows dedupe working across chains:
    // a session is never converted twice, nor back into the tool it came from.
    let steps = [
        ("claude", "opencode", (1, 0)),
        ("opencode", "pi", (1, 0)), // claude-origin, via opencode
        ("claude", "pi", (0, 1)),   // already there through opencode
        ("pi", "claude", (1, 1)),   // the native pi session; the claude-origin one is skipped
        ("claude", "opencode", (1, 1)),
        ("pi", "opencode", (0, 2)),
        ("opencode", "claude", (0, 2)),
    ];
    for (from, to, expected) in steps {
        assert_eq!(env.convert(from, to, &[]), expected, "{from} -> {to}");
    }
    for (from, to, _) in steps {
        assert_eq!(
            env.convert(from, to, &[]).0,
            0,
            "rerun {from} -> {to} converted again"
        );
    }
    // --force replaces instead of duplicating.
    assert_eq!(env.convert("claude", "opencode", &["--force"]), (2, 0));

    check_opencode(&env, &opencode_project[0]);
    check_pi(&env);

    let claude_files = files_with(&env.home.join(".claude"), "\"type\":\"clync-provenance\"");
    assert_eq!(
        claude_files.len(),
        1,
        "expected the pi session in Claude, got {claude_files:?}"
    );
    check_claude_structure(&claude_files[0]);

    let opencode_origin = env.opencode_column(
        "SELECT json_extract(metadata, '$.clync_source') FROM session_v2 WHERE version = 'clync' ORDER BY 1",
    );
    assert_eq!(opencode_origin, ["claude", "pi"]);

    if std::env::var(CLAUDE_OPT_IN_VAR).as_deref() == Ok("1") {
        require("claude");
        check_claude_resume(&env, &claude_files[0]);
    }
}

/// Claude Code's project folder name: every non-alphanumeric ASCII char becomes '-'.
fn claude_folder(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
