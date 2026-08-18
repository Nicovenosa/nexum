use tui_textarea::{Input, Key};

use super::super::Action;
use crate::app::{App, PendingAttachment};

/// Normal mode key handling: main match block arm bodies
pub(super) fn handle_normal_keys(app: &mut App, input: Input) -> anyhow::Result<Option<Action>> {
    use super::update_slash_hint_detection;
    use super::{inject_at_mention_path, update_at_mention_detection};

    match input {
        // Ctrl+C: always copy last Nexum response (never cancel/quit)
        Input {
            key: Key::Char('c'),
            ctrl: true,
            ..
        } => {
            if !super::super::mouse::copy_last_response_to_clipboard(app) {
                let lc = &app.services.lc;
                let msg = lc.tr("statusbar-no-response-to-copy");
                app.session_mgr.current_mut().ui.copy_char_count = 0;
                app.session_mgr.current_mut().ui.copy_message_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(2000));
                app.push_system_note(msg);
            }
        }

        // Ctrl+Q: quit (double-tap)
        Input {
            key: Key::Char('q'),
            ctrl: true,
            ..
        } => {
            if let Some(since) = app.global_ui.quit_pending_since {
                if since.elapsed() < std::time::Duration::from_secs(2) {
                    return Ok(Some(Action::Quit));
                } else {
                    app.global_ui.quit_pending_since = Some(std::time::Instant::now());
                }
            } else {
                app.global_ui.quit_pending_since = Some(std::time::Instant::now());
            }
        }

        // ESC: cancel generation when loading
        Input { key: Key::Esc, .. } if app.session_mgr.current_mut().ui.loading => {
            if !app
                .session_mgr
                .current_mut()
                .messages
                .pending_messages
                .is_empty()
            {
                app.session_mgr
                    .current_mut()
                    .messages
                    .pending_messages
                    .clear();
            }
            app.interrupt();
            app.global_ui.quit_pending_since = None;
        }

        // Esc: 关闭 @ 提及弹窗
        Input { key: Key::Esc, .. } if app.session_mgr.current_mut().ui.at_mention.active => {
            app.session_mgr.current_mut().ui.at_mention.close();
            app.session_mgr.current_mut().ui.at_mention.close();
        }
        // Esc: 关闭 slash hint 弹窗
        Input { key: Key::Esc, .. } if app.session_mgr.current_mut().ui.slash_hint.active => {
            app.session_mgr.current_mut().ui.slash_hint.deactivate();
            app.session_mgr.current_mut().ui.hint_cursor = None;
        }

        // Esc: 双击触发 rewind 选择器（仅空闲时）
        Input { key: Key::Esc, .. } if !app.session_mgr.current().ui.loading => {
            if let Some(since) = app.global_ui.rewind_pending_since {
                if since.elapsed() < std::time::Duration::from_secs(2) {
                    // 双击 ESC → 打开 rewind 选择器
                    app.global_ui.rewind_pending_since = None;
                    app.open_rewind_prompt();
                } else {
                    app.global_ui.rewind_pending_since = Some(std::time::Instant::now());
                }
            } else {
                app.global_ui.rewind_pending_since = Some(std::time::Instant::now());
            }
        }

        // Up: @ 提及导航 > hint navigation > history browse (only first row) > textarea cursor
        Input { key: Key::Up, .. } => handle_up(app),

        // Down: @ 提及导航 > hint navigation > history restore (only last row) > textarea cursor
        Input { key: Key::Down, .. } => handle_down(app),

        // Ctrl+V: try pasting clipboard image first, fallback to text paste
        // Loading 时同样允许——粘贴的文本/图片会进入 textarea / pending_attachments，
        // 后续 Enter 把消息 push 到 pending_messages 队列。
        Input {
            key: Key::Char('v'),
            ctrl: true,
            ..
        } => handle_ctrl_v(app),

        // Tab: @ 提及补全 > hint overlay candidate navigation and completion
        Input {
            key: Key::Tab,
            shift: false,
            ..
        } => {
            handle_tab(app);
            return Ok(Some(Action::Redraw));
        }

        // Shift+Tab / BackTab: modo visual anterior cuando ningún popup/panel lo consumió.
        Input {
            key: Key::Tab,
            shift: true,
            ..
        } => {
            app.global_ui.agent_mode = app.global_ui.agent_mode.previous();
            app.global_ui.agent_mode_highlight_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(1500));
            return Ok(Some(Action::Redraw));
        }

        // Enter with @ mention active and candidates: inject selected path
        Input {
            key: Key::Enter, ..
        } if app.session_mgr.current_mut().ui.at_mention.active
            && !app
                .session_mgr
                .current_mut()
                .ui
                .at_mention
                .candidates
                .is_empty() =>
        {
            inject_at_mention_path(app);
        }

        // Enter with hints available: confirm selection (defaults to first if none selected)
        Input {
            key: Key::Enter, ..
        } if app.hint_candidates_count() > 0 => {
            if app.session_mgr.current_mut().ui.hint_cursor.is_none() {
                app.session_mgr.current_mut().ui.hint_cursor = Some(0);
            }
            app.hint_complete();
        }

        // Shift+Enter / Alt+Enter: insert newline (Shift works everywhere; Alt (Option) for macOS)
        Input {
            key: Key::Enter, ..
        } if input.shift || input.alt => {
            app.session_mgr.current_mut().ui.textarea.input(Input {
                key: Key::Enter,
                ctrl: false,
                alt: false,
                shift: false,
            });
        }

        // Enter: submit (non-loading) or buffer (loading)
        Input {
            key: Key::Enter, ..
        } => {
            // 关闭可能残留的 @ mention 弹窗
            if app.session_mgr.current_mut().ui.at_mention.active {
                app.session_mgr.current_mut().ui.at_mention.close();
            }
            let text = app.session_mgr.current_mut().ui.textarea.lines().join("\n");
            let text = text.trim().to_string();
            if !text.is_empty() {
                if app.session_mgr.current_mut().ui.loading {
                    // Loading state: buffer message
                    app.session_mgr
                        .current_mut()
                        .messages
                        .pending_messages
                        .push(text);
                    app.session_mgr.current_mut().ui.textarea = crate::app::build_textarea(false);
                    app.update_textarea_hint();
                } else if text.starts_with('/') {
                    app.session_mgr.current_mut().ui.textarea = crate::app::build_textarea(false);
                    // SAFETY: command_registry is nested inside App; dispatch needs &mut App
                    let registry = std::mem::take(
                        &mut app.session_mgr.current_mut().commands.command_registry,
                    );
                    let known = registry.dispatch(app, &text);
                    app.session_mgr.current_mut().commands.command_registry = registry;
                    if known {
                        // Command matched, done
                    } else {
                        // Command not matched, try Skill matching
                        let skill_name: String = text
                            .trim_start_matches('/')
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                            .collect();
                        if let Some(_skill) = app
                            .session_mgr
                            .current_mut()
                            .commands
                            .skills
                            .iter()
                            .find(|s| s.name == skill_name)
                        {
                            // Skill matched: submit full message to agent
                            return Ok(Some(Action::Submit(text)));
                        } else if app
                            .session_mgr
                            .current_mut()
                            .commands
                            .agent_commands
                            .contains(&skill_name)
                        {
                            // Agent command matched (from ACP AvailableCommandsUpdate): submit to agent
                            tracing::debug!(skill_name, "Matched agent command, submitting to ACP");
                            return Ok(Some(Action::Submit(text)));
                        } else {
                            // 未知命令/Skill：作为普通输入提交给 Agent
                            tracing::debug!(
                                skill_name,
                                "Unknown slash command, submitting as normal input"
                            );
                            return Ok(Some(Action::Submit(text)));
                        }
                    }
                } else {
                    app.session_mgr.current_mut().ui.textarea = crate::app::build_textarea(false);
                    return Ok(Some(Action::Submit(text)));
                }
            }
        }

        // VS Code terminal maps Option+Backspace to PageUp; perform word-delete when textarea has content
        Input {
            key: Key::PageUp, ..
        } if std::env::var("TERM_PROGRAM").as_deref() == Ok("vscode") => {
            let session = &mut app.session_mgr.current_mut();
            let has_content = session
                .ui
                .textarea
                .lines()
                .iter()
                .any(|line| !line.is_empty());
            if has_content {
                session.ui.textarea.delete_word();
            }
        }

        // PageUp/PageDown: scroll del transcript (fix chat 2026-07-06).
        // El input NUNCA consume estas teclas — el scrollback es del
        // transcript, el textarea tiene su propio scroll implícito.
        Input {
            key: Key::PageUp, ..
        } => {
            for _ in 0..4 {
                app.scroll_up();
            }
        }
        Input {
            key: Key::PageDown, ..
        } => {
            for _ in 0..4 {
                app.scroll_down();
            }
        }

        // Home/End con input VACÍO: inicio/final del transcript. Con texto
        // en el input, Home/End siguen siendo movimiento de cursor del
        // textarea (caen al catch-all de abajo).
        Input { key: Key::Home, .. }
            if app
                .session_mgr
                .current()
                .ui
                .textarea
                .lines()
                .iter()
                .all(|l| l.is_empty()) =>
        {
            app.scroll_to_top();
        }
        Input { key: Key::End, .. }
            if app
                .session_mgr
                .current()
                .ui
                .textarea
                .lines()
                .iter()
                .all(|l| l.is_empty()) =>
        {
            // Volver al final = re-engancha el live-follow (message_area
            // re-activa scroll_follow al llegar al fondo).
            app.scroll_to_bottom();
        }

        // Ctrl+U / Ctrl+D: half-page scroll
        Input {
            key: Key::Char('u'),
            ctrl: true,
            ..
        } => {
            let session = &app.session_mgr.current_mut();
            let has_content = session
                .ui
                .textarea
                .lines()
                .iter()
                .any(|line| !line.is_empty());
            if has_content {
                app.session_mgr
                    .current_mut()
                    .ui
                    .textarea
                    .delete_line_by_head();
            } else {
                for _ in 0..20 {
                    app.scroll_up();
                }
            }
        }
        Input {
            key: Key::Char('d'),
            ctrl: true,
            ..
        } => {
            for _ in 0..20 {
                app.scroll_down();
            }
        }

        // Del: remove last pending attachment
        Input {
            key: Key::Delete, ..
        } if !app.session_mgr.current_mut().ui.loading
            && !app
                .session_mgr
                .current_mut()
                .metadata
                .pending_attachments
                .is_empty() =>
        {
            app.pop_pending_attachment();
        }

        // Fix chat 2026-07-07: caracteres de control que llegan como
        // Key::Char (no como Event::Paste — sanitize_paste_text en
        // event/mod.rs no los ve) se descartan ANTES de tocar el textarea.
        // Origen real en Konsole: secuencias de escape a medias que el
        // terminal reporta como eventos de tecla individuales en vez de un
        // Paste — sin este guard, cada control char aparecía como un
        // cuadradito en la caja de input. \t sí se deja pasar (indentación
        // legítima); el resto de control chars nunca es texto de usuario.
        //
        // Escape sequence absorber (two-phase):
        // Phase 1: ESC arrives → esc_pending = true
        //   If next char is '[' or 'O' → esc_sequence_active = true (phase 2)
        //   If next char is anything else → clear pending, let char through
        // Phase 2: absorbing until terminator (letter A-Z/a-z or ~)

        // ESC: enter phase 1 (pending)
        Input {
            key: Key::Char('\x1b'), ..
        } => {
            app.session_mgr.current_mut().ui.esc_pending = true;
            app.session_mgr.current_mut().ui.esc_sequence_active = false;
        }

        // Phase 2 active: absorb until terminator
        Input {
            key: Key::Char(c), ..
        } if app.session_mgr.current().ui.esc_sequence_active => {
            if c.is_ascii_alphabetic() || c == '~' {
                // Terminator reached — exit sequence
                app.session_mgr.current_mut().ui.esc_pending = false;
                app.session_mgr.current_mut().ui.esc_sequence_active = false;
            }
            // Otherwise keep absorbing
        }

        // Phase 1 pending: check if this char starts a CSI/SS3 sequence
        Input {
            key: Key::Char(c), ..
        } if app.session_mgr.current().ui.esc_pending => {
            if c == '[' || c == 'O' {
                // CSI ([) or SS3 (O) prefix — enter phase 2
                app.session_mgr.current_mut().ui.esc_sequence_active = true;
            } else {
                // Not a sequence prefix — ESC was stray, let this char through
                app.session_mgr.current_mut().ui.esc_pending = false;
                // Fall through to normal handling (re-process this char)
                // We need to handle it now since we can't "un-match"
                if !c.is_control() || c == '\t' {
                    app.session_mgr.current_mut().ui.textarea.input(Input {
                        key: Key::Char(c),
                        ctrl: false,
                        alt: false,
                        shift: false,
                    });
                }
            }
        }

        // Control chars (no ESC, no \n, no \r — already handled above).
        // This catches: C0 (BEL, BS, NULL, etc.), C1 (U+009B etc.), and any
        // other control codepoint that isn't a legitimate text input.
        Input {
            key: Key::Char(c), ..
        } if c.is_control() && c != '\t' => {}

        // Intercept plain Enter to avoid textarea default newline; allow input during loading
        input if input.key != Key::Enter => {
            // Exit history browsing
            if app.session_mgr.current_mut().ui.history_index.is_some() {
                app.exit_history();
            }
            let len_before: usize = app.session_mgr.current().ui.textarea.lines().iter().map(|l| l.len()).sum();
            app.session_mgr.current_mut().ui.textarea.input(input);
            app.session_mgr.current_mut().ui.log_mutation(
                crate::app::InputMutationSource::UserKey,
                len_before,
            );
            // 任意输入清除 prediction
            app.session_mgr.current_mut().ui.prediction = None;
            // When input changes: reset cursor (don't pre-select; wait for user to press Tab/Up/Down)
            // Loading 时也需更新——用户在 queue 下一条消息时同样期望 slash hint / @mention 弹窗。
            app.session_mgr.current_mut().ui.hint_cursor = None;
            update_at_mention_detection(app);
            update_slash_hint_detection(app);
        }
        _ => {
            // Any other key cancels quit-pending state (Ctrl+C double-tap)
            app.global_ui.quit_pending_since = None;
            // Note: do NOT reset rewind_pending_since here. The fallback arm
            // captures keys like Key::Enter (with unmatched modifiers) and
            // terminal-generated sequences (e.g. focus events, unknown keys).
            // Resetting here would break the ESC double-tap detection because
            // spurious key events between two ESC presses would clear the state.
            // rewind_pending_since is naturally reset when the user types actual
            // content (the `input if input.key != Key::Enter` arm above).
        }
    }

    Ok(Some(Action::Redraw))
}

// ── Per-arm helper functions ──────────────────────────────────────────────

fn handle_up(app: &mut App) {
    let hint_count = app.hint_candidates_count();
    if app.session_mgr.current_mut().ui.at_mention.active {
        app.session_mgr.current_mut().ui.at_mention.move_up();
    } else if hint_count > 0 {
        let cur = app.session_mgr.current_mut().ui.hint_cursor.unwrap_or(0);
        app.session_mgr.current_mut().ui.hint_cursor = if cur == 0 {
            Some(hint_count - 1)
        } else {
            Some(cur - 1)
        };
    } else {
        let (row, _col) = app.session_mgr.current_mut().ui.textarea.cursor();
        if row == 0 {
            app.history_up();
        } else {
            app.session_mgr.current_mut().ui.textarea.input(Input {
                key: Key::Up,
                ctrl: false,
                alt: false,
                shift: false,
            });
        }
    }
}

fn handle_down(app: &mut App) {
    let hint_count = app.hint_candidates_count();
    if app.session_mgr.current_mut().ui.at_mention.active {
        app.session_mgr.current_mut().ui.at_mention.move_down();
    } else if hint_count > 0 {
        let cur = app
            .session_mgr
            .current_mut()
            .ui
            .hint_cursor
            .unwrap_or(hint_count - 1);
        app.session_mgr.current_mut().ui.hint_cursor = if cur + 1 >= hint_count {
            Some(0)
        } else {
            Some(cur + 1)
        };
    } else if app.session_mgr.current_mut().ui.history_index.is_some() {
        app.history_down();
    } else {
        let (row, _col) = app.session_mgr.current_mut().ui.textarea.cursor();
        let last_row = app
            .session_mgr
            .current_mut()
            .ui
            .textarea
            .lines()
            .len()
            .saturating_sub(1);
        if row >= last_row {
            app.history_down();
        } else {
            app.session_mgr.current_mut().ui.textarea.input(Input {
                key: Key::Down,
                ctrl: false,
                alt: false,
                shift: false,
            });
        }
    }
}

fn handle_ctrl_v(app: &mut App) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(img) = clipboard.get_image() {
            let (w, h) = (img.width as u32, img.height as u32);
            if let Ok((b64, sz)) = super::super::mouse::rgba_to_png_base64(w, h, &img.bytes) {
                let n = app
                    .session_mgr
                    .current_mut()
                    .metadata
                    .pending_attachments
                    .len()
                    + 1;
                app.add_pending_attachment(PendingAttachment {
                    label: format!("clipboard_{}.png", n),
                    media_type: "image/png".to_string(),
                    base64_data: b64,
                    size_bytes: sz,
                });
            }
        } else if let Ok(text) = clipboard.get_text() {
            let text = text.replace('\r', "\n");
            let len_before: usize = app.session_mgr.current().ui.textarea.lines().iter().map(|l| l.len()).sum();
            app.session_mgr.current_mut().ui.textarea.insert_str(&text);
            app.session_mgr.current_mut().ui.log_mutation(
                crate::app::InputMutationSource::Paste,
                len_before,
            );
        }
    }
}

fn handle_tab(app: &mut App) {
    use super::inject_at_mention_path;

    // Prediction 接受优先级最高
    if let Some(pred) = app.session_mgr.current_mut().ui.prediction.take() {
        let len_before: usize = app.session_mgr.current().ui.textarea.lines().iter().map(|l| l.len()).sum();
        app.session_mgr
            .current_mut()
            .ui
            .textarea
            .insert_str(&pred.text);
        app.session_mgr.current_mut().ui.log_mutation(
            crate::app::InputMutationSource::PredictionAccept,
            len_before,
        );
        return;
    }

    if app.session_mgr.current_mut().ui.at_mention.active {
        inject_at_mention_path(app);
    } else {
        let count = app.hint_candidates_count();
        if count > 0 {
            match app.session_mgr.current_mut().ui.hint_cursor {
                Some(cur) if cur + 1 < count => {
                    app.session_mgr.current_mut().ui.hint_cursor = Some(cur + 1);
                }
                Some(_) => {
                    app.session_mgr.current_mut().ui.hint_cursor = Some(0);
                }
                None => {
                    app.session_mgr.current_mut().ui.hint_cursor = Some(0);
                }
            }
        } else {
            app.global_ui.agent_mode = app.global_ui.agent_mode.next();
            app.global_ui.agent_mode_highlight_until =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(1500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::build_textarea;
    use crate::event::Action;

    async fn make_app() -> App {
        let (app, _) = App::new_headless(80, 24).await;
        app
    }

    #[tokio::test]
    async fn test_ctrl_c_copia_respuesta_cuando_existe() {
        use crate::ui::message_view::MessageViewModel;
        let mut app = make_app().await;
        app.session_mgr
            .current_mut()
            .messages
            .view_messages
            .push(MessageViewModel::assistant_blocks(vec![
                crate::ui::message_view::ContentBlockView::Text {
                    raw: "respuesta de Nexum".to_string(),
                    rendered: ratatui::text::Text::raw("respuesta de Nexum"),
                    dirty: false,
                    rendered_prefix_len: 18,
                    rendered_prefix_lines: 1,
                    holdback_scanner: Default::default(),
                },
            ]));
        let input = Input {
            key: Key::Char('c'),
            ctrl: true,
            ..Default::default()
        };
        let result = handle_normal_keys(&mut app, input);
        assert!(result.is_ok());
        assert!(
            app.session_mgr.current().ui.copy_message_until.is_some(),
            "Ctrl+C debe setear copy_message_until cuando hay respuesta"
        );
    }

    #[tokio::test]
    async fn test_ctrl_c_sin_respuesta_muestra_nota() {
        let mut app = make_app().await;
        let input = Input {
            key: Key::Char('c'),
            ctrl: true,
            ..Default::default()
        };
        let result = handle_normal_keys(&mut app, input);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_y_se_escribe_normal_en_textarea() {
        let mut app = make_app().await;
        for c in "Yo necesito ayuda".chars() {
            let _ = handle_normal_keys(
                &mut app,
                Input {
                    key: Key::Char(c),
                    ..Default::default()
                },
            );
        }
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(
            lines,
            vec!["Yo necesito ayuda".to_string()],
            "y/Y deben escribirse normalmente, no disparar copy"
        );
    }

    // ─── Fix chat 2026-07-07: input queda limpio ante control chars ───────

    #[tokio::test]
    async fn test_control_char_como_key_no_se_inserta_en_textarea() {
        let mut app = make_app().await;
        // Simula el caso real de Konsole: una secuencia de escape a medias
        // llega como Key::Char con un codepoint de control (no como Paste).
        for c in ['\x1b', '\x07', '\u{009b}', '\x01'] {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert!(
            lines.iter().all(|l| l.is_empty()),
            "los control chars no deben aparecer en el input: {:?}",
            lines
        );
    }

    #[tokio::test]
    async fn test_texto_normal_y_control_chars_intercalados() {
        let mut app = make_app().await;
        for c in ['h', 'o', 'l', 'a', '\x1b', ' ', 'm', 'u', 'n', 'd', 'o'] {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(
            lines, vec!["hola mundo".to_string()],
            "el control char intercalado se descarta, el resto del texto queda intacto"
        );
    }

    #[tokio::test]
    async fn test_tab_sigue_pasando_al_textarea() {
        let mut app = make_app().await;
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('\t'), ..Default::default() });
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(lines, vec!["\t".to_string()], "tab no es un control char a filtrar");
    }

    #[tokio::test]
    async fn test_espanol_con_tildes_se_escribe_sin_corrupcion() {
        let mut app = make_app().await;
        for c in "auditoría filosóficamente".chars() {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(lines, vec!["auditoría filosóficamente".to_string()]);
    }

    #[tokio::test]
    async fn test_escape_sequence_csi_up_no_leak_to_textarea() {
        // Simula ESC[A (cursor up) como lo haría Konsole: 3 eventos Key::Char
        let mut app = make_app().await;
        // Primero escribimos algo para que no esté vacío
        for c in "hello".chars() {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        // Ahora simulamos ESC[A
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('\x1b'), ..Default::default() });
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('['), ..Default::default() });
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('A'), ..Default::default() });
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(
            lines, vec!["hello".to_string()],
            "ESC[A no debe insertar '[A' en el input"
        );
    }

    #[tokio::test]
    async fn test_escape_sequence_csi_scroll_no_leak() {
        // Simula ESC[<65;1;1M (mouse scroll up) como Konsole
        let mut app = make_app().await;
        for c in "test".chars() {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        let seq = ['\x1b', '[', '<', '6', '5', ';', '1', ';', '1', 'M'];
        for c in seq {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(
            lines, vec!["test".to_string()],
            "mouse scroll escape sequence no debe contaminar input"
        );
    }

    #[tokio::test]
    async fn test_bracket_after_escape_not_inserted() {
        // Verifica que '[' solo (sin terminar secuencia) no se inserte
        let mut app = make_app().await;
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('\x1b'), ..Default::default() });
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('['), ..Default::default() });
        // Ahora escribimos algo normal — la secuencia debe seguir activa
        // hasta ver un terminador
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('3'), ..Default::default() });
        let _ = handle_normal_keys(&mut app, Input { key: Key::Char('~'), ..Default::default() });
        // Ahora la secuencia terminó, escribimos texto real
        for c in "ok".chars() {
            let _ = handle_normal_keys(&mut app, Input { key: Key::Char(c), ..Default::default() });
        }
        let lines = app.session_mgr.current().ui.textarea.lines().to_vec();
        assert_eq!(
            lines, vec!["ok".to_string()],
            "secuencia ESC[3~ (Delete key) no debe insertar chars"
        );
    }
}
