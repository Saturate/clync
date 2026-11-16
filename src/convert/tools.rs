//! Tool name normalization and mapping between formats.

/// Normalize tool name to lowercase for the IR.
pub fn normalize_tool_name(name: &str) -> String {
    // MCP tool names are matched exactly against the server's registration
    if name.starts_with("mcp__") {
        return name.to_string();
    }
    let lower = name.to_lowercase();
    // opencode v2 uses "shell" for the command tool; normalize to "bash"
    match lower.as_str() {
        "shell" => "bash".to_string(),
        "subagent" => "agent".to_string(),
        "askuserquestion" => "question".to_string(),
        _ => lower,
    }
}

/// Map normalized tool name to Claude format (PascalCase).
pub fn to_claude_tool_name(normalized: &str) -> String {
    match normalized {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "grep" => "Grep",
        "glob" => "Glob",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "agent" => "Agent",
        "question" => "AskUserQuestion",
        "task" => "Task",
        "todowrite" => "TodoWrite",
        "multiedit" => "MultiEdit",
        "notebookedit" => "NotebookEdit",
        "exitplanmode" => "ExitPlanMode",
        "bashoutput" => "BashOutput",
        "killshell" => "KillShell",
        "skill" => "Skill",
        _ => return normalized.to_string(),
    }
    .to_string()
}

/// Map normalized tool name to opencode v2 format.
pub fn to_opencode_tool_name(normalized: &str) -> String {
    match normalized {
        "bash" => "shell",
        "webfetch" => "webfetch",
        "websearch" => "websearch",
        "agent" => "subagent",
        "question" => "question",
        _ => normalized,
    }
    .to_string()
}

/// Normalize a pi tool name: the inverse of [`to_pi_tool_name`]. Kept out of
/// [`normalize_tool_name`] because `task` means something else elsewhere.
pub fn normalize_pi_tool_name(name: &str) -> String {
    match name {
        "fetch" => "webfetch".to_string(),
        "task" => "agent".to_string(),
        _ => normalize_tool_name(name),
    }
}

/// Map normalized tool name to pi format (lowercase).
pub fn to_pi_tool_name(normalized: &str) -> String {
    match normalized {
        "webfetch" => "fetch",
        "agent" => "task",
        _ => normalized,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_preserves_lowercase() {
        assert_eq!(normalize_tool_name("bash"), "bash");
        assert_eq!(normalize_tool_name("read"), "read");
    }

    #[test]
    fn normalize_lowercases_uppercase() {
        assert_eq!(normalize_tool_name("Bash"), "bash");
        assert_eq!(normalize_tool_name("Read"), "read");
        assert_eq!(normalize_tool_name("WebFetch"), "webfetch");
    }

    #[test]
    fn normalize_shell_to_bash() {
        assert_eq!(normalize_tool_name("shell"), "bash");
    }

    #[test]
    fn normalize_subagent_to_agent() {
        assert_eq!(normalize_tool_name("subagent"), "agent");
    }

    #[test]
    fn normalize_askuserquestion() {
        assert_eq!(normalize_tool_name("AskUserQuestion"), "question");
    }

    #[test]
    fn claude_mapping_known_tools() {
        assert_eq!(to_claude_tool_name("bash"), "Bash");
        assert_eq!(to_claude_tool_name("read"), "Read");
        assert_eq!(to_claude_tool_name("glob"), "Glob");
        assert_eq!(to_claude_tool_name("webfetch"), "WebFetch");
        assert_eq!(to_claude_tool_name("agent"), "Agent");
        assert_eq!(to_claude_tool_name("question"), "AskUserQuestion");
    }

    #[test]
    fn claude_mapping_unknown_preserves() {
        assert_eq!(to_claude_tool_name("mcp__chrome"), "mcp__chrome");
    }

    #[test]
    fn opencode_mapping() {
        assert_eq!(to_opencode_tool_name("bash"), "shell");
        assert_eq!(to_opencode_tool_name("agent"), "subagent");
        assert_eq!(to_opencode_tool_name("read"), "read");
    }

    #[test]
    fn pi_mapping() {
        assert_eq!(to_pi_tool_name("bash"), "bash");
        assert_eq!(to_pi_tool_name("webfetch"), "fetch");
        assert_eq!(to_pi_tool_name("agent"), "task");
    }

    #[test]
    fn normalize_keeps_mcp_names_exact() {
        assert_eq!(
            normalize_tool_name("mcp__Claude_in_Chrome__navigate"),
            "mcp__Claude_in_Chrome__navigate"
        );
    }

    #[test]
    fn claude_builtins_roundtrip_casing() {
        for name in ["TodoWrite", "MultiEdit", "NotebookEdit", "Task", "Skill"] {
            assert_eq!(to_claude_tool_name(&normalize_tool_name(name)), name);
        }
    }

    #[test]
    fn pi_names_roundtrip() {
        for normalized in ["webfetch", "agent", "bash", "read"] {
            assert_eq!(
                normalize_pi_tool_name(&to_pi_tool_name(normalized)),
                normalized
            );
        }
    }
}
