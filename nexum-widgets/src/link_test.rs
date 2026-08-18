use super::{wrap_osc8, LinkSpan, LinkWidget};
use ratatui::style::Modifier;

#[test]
fn wrap_osc8_wraps_url_and_text() {
    let result = wrap_osc8("Click me", "https://example.com");
    assert!(result.starts_with("\x1b]8;;https://example.com\x1b\\"));
    assert!(result.ends_with("\x1b]8;;\x1b\\"));
    assert!(result.contains("Click me"));
}

#[test]
fn wrap_osc8_empty_url_returns_plain_text() {
    let result = wrap_osc8("No URL", "");
    assert_eq!(result, "No URL");
}

#[test]
fn link_span_to_span_wraps_with_osc8() {
    let link = LinkSpan::new("https://example.com", "Example");
    let span = link.to_span();
    assert!(span
        .content
        .starts_with("\x1b]8;;https://example.com\x1b\\"));
    assert!(span.content.contains("Example"));
    assert!(span.content.ends_with("\x1b]8;;\x1b\\"));
    assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn link_span_empty_url_skips_osc8() {
    let link = LinkSpan::new("", "No URL");
    let span = link.to_span();
    assert!(!span.content.contains("\x1b]8;;"));
    assert_eq!(span.content, "No URL");
}

#[test]
fn link_span_empty_text_uses_url_as_fallback() {
    let link = LinkSpan::new("https://example.com", "");
    let span = link.to_span();
    assert!(span.content.contains("https://example.com"));
}

#[test]
fn link_span_max_width_truncates_text() {
    let link = LinkSpan::new("https://example.com", "Very Long Text").max_width(4);
    let span = link.to_span();
    assert!(span.content.contains("Very…"));
    assert!(!span.content.contains("Very Long Text"));
}

#[test]
fn link_span_cjk_truncation() {
    let link = LinkSpan::new("https://example.com", "你好世界").max_width(2);
    let span = link.to_span();
    assert!(span.content.contains("你好…"));
}

#[test]
fn link_span_no_truncate_when_fits() {
    let link = LinkSpan::new("https://example.com", "hi").max_width(4);
    let span = link.to_span();
    assert!(span.content.contains("\x1b]8;;"));
    assert!(span.content.contains("hi"));
    assert!(!span.content.contains("…"));
}

#[test]
fn link_widget_renders() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::WidgetRef;

    let link = LinkSpan::new("https://example.com", "Click me");
    let widget = LinkWidget { link: &link };
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
    widget.render_ref(Rect::new(0, 0, 40, 1), &mut buf);
    // Link text should be visible in the buffer
    // (OSC 8 escape codes are terminal-level; \x1b is stripped by buffer but bracket text remains)
    let full_line: String = (0..40)
        .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol()))
        .collect();
    assert!(
        full_line.contains("Click me"),
        "Expected 'Click me' in: {full_line}"
    );
    assert!(
        full_line.contains("https://example.com"),
        "Expected URL in: {full_line}"
    );
}
