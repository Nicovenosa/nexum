#[test]
fn test_format_tool_name_new_names() {
    assert_eq!(format_tool_name("Read"), "Read");
    assert_eq!(format_tool_name("Write"), "Write");
    assert_eq!(format_tool_name("Edit"), "Edit");
    assert_eq!(format_tool_name("Glob"), "Glob");
    assert_eq!(format_tool_name("Grep"), "Grep");
    assert_eq!(format_tool_name("Bash"), "Shell");
    assert_eq!(format_tool_name("TodoWrite"), "Todo");
    assert_eq!(format_tool_name("AskUserQuestion"), "Ask");
    assert_eq!(format_tool_name("Agent"), "Agent");
}

#[test]
fn test_format_tool_args_grep_uses_pattern() {
    let input = serde_json::json!({"pattern": "needle", "output_mode": "content"});
    let result = format_tool_args("Grep", &input, None);
    assert!(result.is_some(), "Grep 工具应返回 pattern 摘要");
    assert!(result.unwrap().contains("needle"), "应包含 pattern 内容");
}

#[test]
fn test_format_tool_args_bash_uses_command() {
    let input = serde_json::json!({"command": "cargo test"});
    let result = format_tool_args("Bash", &input, None);
    assert!(result.is_some());
    assert!(result.unwrap().contains("cargo test"));
}

#[test]
fn test_old_tool_names_not_matched() {
    // 验证旧工具名不再被匹配（fallback 到 to_pascal）
    assert_eq!(format_tool_name("bash"), "Bash"); // fallback
    assert_eq!(format_tool_name("read_file"), "ReadFile"); // fallback to_pascal
    assert_eq!(format_tool_name("write_file"), "WriteFile"); // fallback to_pascal
    assert_eq!(format_tool_name("search_files_rg"), "SearchFilesRg"); // fallback to_pascal
    assert_eq!(format_tool_name("launch_agent"), "LaunchAgent"); // fallback to_pascal
}

#[test]
fn test_read_write_edit_file_path_not_truncated() {
    // Read/Write/Edit 的 file_path 完整显示不截断
    let path = "/home/user/projects/my-app/src/components/header.rs";
    let input = serde_json::json!({"file_path": path});
    let result = format_tool_args("Read", &input, Some("/home/user/projects/my-app/"));
    assert_eq!(
        result.unwrap(),
        "src/components/header.rs",
        "file_path 应完整显示不截断"
    );

    let result = format_tool_args("Write", &input, Some("/home/user/"));
    assert_eq!(
        result.unwrap(),
        "projects/my-app/src/components/header.rs",
        "file_path 应完整显示不截断"
    );

    let result = format_tool_args("Edit", &input, None);
    // 无 cwd 时 fallback 取末段文件名
    assert_eq!(result.unwrap(), "header.rs");
}

#[test]
fn test_bash_truncates_at_400() {
    let cmd = "a".repeat(500);
    let input = serde_json::json!({"command": cmd});
    let result = format_tool_args("Bash", &input, None).unwrap();
    assert_eq!(
        result.chars().count(),
        401,
        "Bash 命令应截断到 400 字符 + …"
    );
    assert!(result.ends_with('…'), "超长 Bash 命令应以 … 结尾");
    assert!(result.starts_with('a'), "Bash 命令应保留前 400 字符");
}

#[test]
fn test_glob_truncates_at_200() {
    let pattern = "p".repeat(300);
    let input = serde_json::json!({"pattern": pattern, "path": "/tmp"});
    let result = format_tool_args("Glob", &input, None).unwrap();
    assert_eq!(
        result.chars().count(),
        201,
        "Glob pattern 应截断到 200 字符 + …"
    );
    assert!(result.ends_with('…'), "超长 Glob pattern 应以 … 结尾");
}

#[test]
fn test_grep_truncates_at_200() {
    let pattern = "r".repeat(300);
    let input = serde_json::json!({"pattern": pattern});
    let result = format_tool_args("Grep", &input, None).unwrap();
    assert_eq!(
        result.chars().count(),
        201,
        "Grep pattern 应截断到 200 字符 + …"
    );
    assert!(result.ends_with('…'), "超长 Grep pattern 应以 … 结尾");
}

#[test]
fn test_should_auto_expand_agent_result() {
    assert!(should_auto_expand_tool("AgentResult", false));
    // 错误结果不展开（走 error_summary_lines）
    assert!(!should_auto_expand_tool("AgentResult", true));
}

#[test]
fn test_should_auto_expand_execute_extra_tool() {
    // ExecuteExtraTool 包装 artifact/WebFetch 等工具，结果含 URL 等关键信息
    assert!(
        should_auto_expand_tool("ExecuteExtraTool", false),
        "ExecuteExtraTool 应默认展开显示 URL 等关键结果"
    );
    assert!(!should_auto_expand_tool("ExecuteExtraTool", true));
}

#[test]
fn test_should_auto_expand_other_tools_false() {
    // Read/Grep/LSP 等仍保持折叠（结果通过 ▸ 图标提示存在）
    assert!(!should_auto_expand_tool("Read", false));
    assert!(!should_auto_expand_tool("Grep", false));
    assert!(!should_auto_expand_tool("Bash", false));
    assert!(!should_auto_expand_tool("LSP", false));
}
