use super::*;

#[test]
fn test_indicator_running_blinks() {
    assert_eq!(format_indicator(ToolCallStatus::Running, 0), "●");
    assert_eq!(format_indicator(ToolCallStatus::Running, 4), " ");
}

#[test]
fn test_indicator_pending() {
    assert_eq!(format_indicator(ToolCallStatus::Pending, 0), "●");
}

#[test]
fn test_indicator_completed() {
    assert_eq!(format_indicator(ToolCallStatus::Completed, 0), "●");
}

#[test]
fn test_indicator_failed() {
    assert_eq!(format_indicator(ToolCallStatus::Failed, 0), "✗");
}

#[test]
fn test_format_args_summary_short() {
    assert_eq!(format_args_summary("hello", 40), "hello");
}

#[test]
fn test_format_args_summary_truncated() {
    let long = "a".repeat(50);
    let result = format_args_summary(&long, 10);
    assert_eq!(result.chars().count(), 10);
    assert!(result.ends_with('…'));
}

#[test]
fn test_format_args_summary_exact_width() {
    let s = "1234567890";
    assert_eq!(format_args_summary(s, 10), "1234567890");
}

#[test]
fn test_format_args_summary_empty() {
    assert_eq!(format_args_summary("", 10), "");
}
