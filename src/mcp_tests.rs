use super::*;

#[test]
fn truncate_str_short() {
    assert_eq!(truncate_str("hello", 10), "hello");
}

#[test]
fn truncate_str_exact() {
    assert_eq!(truncate_str("hello", 5), "hello");
}

#[test]
fn truncate_str_long() {
    let result = truncate_str("hello world", 8);
    assert!(result.ends_with("..."));
    assert!(result.len() <= 8);
}

#[test]
fn truncate_str_multibyte() {
    let s = "hélló wörld";
    let result = truncate_str(s, 8);
    assert!(result.ends_with("..."));
    assert!(result.is_char_boundary(0));
}

#[test]
fn handle_initialize() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(1)),
        method: "initialize".into(),
        params: json!({}),
    };
    let resp = handle_request(&req);
    let result = resp.result.unwrap();
    assert_eq!(result["capabilities"]["tools"], json!({}));
    assert_eq!(result["serverInfo"]["name"], "clync");
    assert!(resp.error.is_none());
}

#[test]
fn handle_tools_list() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(2)),
        method: "tools/list".into(),
        params: json!({}),
    };
    let resp = handle_request(&req);
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
    assert!(!tools.is_empty());
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"list_sessions"));
    assert!(names.contains(&"sync_push"));
    assert!(names.contains(&"sync_pull"));
    assert!(names.contains(&"help"));
}

#[test]
fn handle_unknown_method() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(3)),
        method: "nonexistent".into(),
        params: json!({}),
    };
    let resp = handle_request(&req);
    assert!(resp.result.is_none());
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap()["code"], -32601);
}

#[test]
fn handle_unknown_tool() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(4)),
        method: "tools/call".into(),
        params: json!({"name": "nonexistent_tool", "arguments": {}}),
    };
    let resp = handle_request(&req);
    let result = resp.result.unwrap();
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("unknown tool"));
}

#[test]
fn handle_help_tool() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(5)),
        method: "tools/call".into(),
        params: json!({"name": "help", "arguments": {"topic": "all"}}),
    };
    let resp = handle_request(&req);
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("clync"));
}

#[test]
fn handle_null_id() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: None,
        method: "initialize".into(),
        params: json!({}),
    };
    let resp = handle_request(&req);
    assert_eq!(resp.id, Value::Null);
    assert!(resp.result.is_some());
}

#[test]
fn tool_definitions_have_required_fields() {
    let tools = tool_definitions();
    let arr = tools.as_array().unwrap();
    for tool in arr {
        assert!(tool.get("name").is_some(), "tool missing name");
        assert!(
            tool.get("description").is_some(),
            "tool missing description"
        );
        assert!(
            tool.get("inputSchema").is_some(),
            "tool missing inputSchema"
        );
    }
}
