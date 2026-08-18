use super::ToolCallStatus;

pub fn format_indicator(status: ToolCallStatus, tick: u64) -> &'static str {
    match status {
        ToolCallStatus::Pending => "●",
        ToolCallStatus::Running => {
            if (tick / 4).is_multiple_of(2) {
                "●"
            } else {
                " "
            }
        }
        ToolCallStatus::Completed => "●",
        ToolCallStatus::Failed => "✗",
    }
}

pub fn format_args_summary(args: &str, max_width: usize) -> String {
    if args.len() <= max_width {
        args.to_string()
    } else {
        let mut truncated: String = args.chars().take(max_width.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    }
}

#[cfg(test)]
#[path = "display_test.rs"]
mod tests;
