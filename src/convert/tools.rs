//! Tool name normalization and mapping between formats.

/// Normalize tool name to lowercase for the IR.
pub fn normalize_tool_name(name: &str) -> String {
    name.to_lowercase()
}

/// Map normalized tool name to Claude format (PascalCase).
pub fn to_claude_tool_name(normalized: &str) -> String {
    // Known tools get PascalCase
    let pascal_case = match normalized {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "grep" => "Grep",
        "webfetch" => "WebFetch",
        "agent" => "Agent",
        _ => return normalized.to_string(), // Pass through unknown tools unchanged
    };
    pascal_case.to_string()
}

/// Map normalized tool name to opencode format (lowercase).
pub fn to_opencode_tool_name(normalized: &str) -> String {
    // Known tools map specifically
    let mapped = match normalized {
        "webfetch" => "fetch",
        "agent" => "task",
        _ => normalized,
    };
    mapped.to_string()
}

/// Map normalized tool name to pi format (lowercase).
pub fn to_pi_tool_name(normalized: &str) -> String {
    // Known tools map specifically
    let mapped = match normalized {
        "webfetch" => "fetch",
        "agent" => "task",
        _ => normalized,
    };
    mapped.to_string()
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
    }

    #[test]
    fn claude_mapping_known_tools() {
        assert_eq!(to_claude_tool_name("bash"), "Bash");
        assert_eq!(to_claude_tool_name("read"), "Read");
        assert_eq!(to_claude_tool_name("webfetch"), "WebFetch");
        assert_eq!(to_claude_tool_name("agent"), "Agent");
    }

    #[test]
    fn claude_mapping_unknown_preserves() {
        assert_eq!(to_claude_tool_name("mcp__chrome"), "mcp__chrome");
    }

    #[test]
    fn opencode_mapping_known_tools() {
        assert_eq!(to_opencode_tool_name("bash"), "bash");
        assert_eq!(to_opencode_tool_name("webfetch"), "fetch");
        assert_eq!(to_opencode_tool_name("agent"), "task");
    }

    #[test]
    fn opencode_mapping_unknown_preserves() {
        assert_eq!(to_opencode_tool_name("read"), "read");
    }
}
