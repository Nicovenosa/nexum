use super::FieldTextarea;

fn char_input(c: char) -> tui_textarea::Input {
    tui_textarea::Input {
        key: tui_textarea::Key::Char(c),
        ctrl: false,
        alt: false,
        shift: false,
    }
}

fn backspace_input() -> tui_textarea::Input {
    tui_textarea::Input {
        key: tui_textarea::Key::Backspace,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

#[test]
fn test_single_line_input_char() {
    let mut ta = FieldTextarea::single_line();
    ta.input(char_input('a'));
    assert_eq!(ta.value(), "a");
}

#[test]
fn test_single_line_backspace() {
    let mut ta = FieldTextarea::single_line();
    ta.set_value("abc");
    ta.input(backspace_input());
    assert_eq!(ta.value(), "ab");
}

#[test]
fn test_set_value() {
    let mut ta = FieldTextarea::single_line();
    ta.set_value("hello");
    assert_eq!(ta.value(), "hello");
    ta.set_value("");
    assert!(ta.is_empty());
}

#[test]
fn test_multi_line_render_height() {
    let mut ta = FieldTextarea::multi_line(5);
    assert_eq!(ta.render_height(), 1);
    ta.set_value("a\nb\nc");
    assert_eq!(ta.render_height(), 3);
}

#[test]
fn test_multi_line_clamp_height() {
    let mut ta = FieldTextarea::multi_line(3);
    ta.set_value("a\nb\nc\nd\ne");
    assert_eq!(ta.render_height(), 3);
}

#[test]
fn test_clear() {
    let mut ta = FieldTextarea::single_line();
    ta.set_value("content");
    ta.clear();
    assert!(ta.is_empty());
}

#[test]
fn test_cursor_position_after_set_value() {
    let mut ta = FieldTextarea::single_line();
    ta.set_value("abc");
    ta.input(char_input('d'));
    assert_eq!(ta.value(), "abcd");
}

#[test]
fn test_clone() {
    let mut ta = FieldTextarea::single_line();
    ta.set_value("hello");
    let cloned = ta.clone();
    assert_eq!(cloned.value(), "hello");
}
