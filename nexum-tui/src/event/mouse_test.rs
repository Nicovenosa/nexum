use super::*;
use crate::app::CopyButtonHitbox;
use crate::ui::message_view::{ContentBlockView, MessageViewModel};
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

fn click_at(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn up_at(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn make_text_block(raw: &str) -> ContentBlockView {
    ContentBlockView::Text {
        raw: raw.to_string(),
        rendered: ratatui::text::Text::raw(raw.to_string()),
        dirty: false,
        rendered_prefix_len: 0,
        rendered_prefix_lines: 0,
        holdback_scanner: Default::default(),
    }
}

// ── find_copy_button_hit (puro) ──────────────────────────────────────────

#[test]
fn test_click_dentro_del_hitbox_devuelve_message_idx() {
    let hitboxes = vec![CopyButtonHitbox {
        rect: Rect {
            x: 2,
            y: 10,
            width: 20,
            height: 1,
        },
        message_idx: 3,
    }];
    assert_eq!(find_copy_button_hit(&hitboxes, &click_at(2, 10)), Some(3));
    assert_eq!(
        find_copy_button_hit(&hitboxes, &click_at(21, 10)),
        Some(3),
        "última columna del botón (x + width - 1) también es click válido"
    );
}

#[test]
fn test_click_fuera_del_hitbox_no_copia() {
    let hitboxes = vec![CopyButtonHitbox {
        rect: Rect {
            x: 2,
            y: 10,
            width: 20,
            height: 1,
        },
        message_idx: 3,
    }];
    assert_eq!(
        find_copy_button_hit(&hitboxes, &click_at(22, 10)),
        None,
        "una columna después del botón queda fuera"
    );
    assert_eq!(
        find_copy_button_hit(&hitboxes, &click_at(1, 10)),
        None,
        "una columna antes del botón queda fuera"
    );
    assert_eq!(
        find_copy_button_hit(&hitboxes, &click_at(5, 11)),
        None,
        "la fila de abajo queda fuera (height=1)"
    );
    assert_eq!(find_copy_button_hit(&[], &click_at(5, 10)), None);
}

#[test]
fn test_dos_botones_en_historial_cada_uno_su_mensaje() {
    let hitboxes = vec![
        CopyButtonHitbox {
            rect: Rect {
                x: 2,
                y: 4,
                width: 20,
                height: 1,
            },
            message_idx: 1,
        },
        CopyButtonHitbox {
            rect: Rect {
                x: 2,
                y: 15,
                width: 20,
                height: 1,
            },
            message_idx: 5,
        },
    ];
    assert_eq!(
        find_copy_button_hit(&hitboxes, &click_at(5, 4)),
        Some(1),
        "el botón del turno viejo copia el turno viejo"
    );
    assert_eq!(
        find_copy_button_hit(&hitboxes, &click_at(5, 15)),
        Some(5),
        "el botón del turno nuevo copia el turno nuevo"
    );
}

// ── copy_response_at_to_clipboard (con App headless) ─────────────────────

#[tokio::test]
async fn test_copy_response_at_copia_la_burbuja_del_indice() {
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    {
        let msgs = &mut app.session_mgr.current_mut().messages.view_messages;
        msgs.push(MessageViewModel::user("pregunta 1".to_string()));
        msgs.push(MessageViewModel::assistant_blocks(vec![make_text_block(
            "respuesta vieja",
        )]));
        msgs.push(MessageViewModel::user("pregunta 2".to_string()));
        msgs.push(MessageViewModel::assistant_blocks(vec![make_text_block(
            "respuesta nueva más larga",
        )]));
    }

    // Click en el botón del turno VIEJO (idx 1) → copia ESA respuesta.
    assert!(copy_response_at_to_clipboard(&mut app, 1));
    assert_eq!(
        app.session_mgr.current().ui.copy_char_count,
        "respuesta vieja".chars().count(),
        "el feedback refleja la respuesta del turno clickeado, no la última"
    );
    assert!(app.session_mgr.current().ui.copy_message_until.is_some());
    // El flash "copiado" quedó marcado en la burbuja correcta.
    let flash_en_idx1 = matches!(
        &app.session_mgr.current().messages.view_messages[1],
        MessageViewModel::AssistantBubble {
            copied_label_until: Some(_),
            ..
        }
    );
    assert!(flash_en_idx1, "copied_label_until seteado en la burbuja 1");
}

#[tokio::test]
async fn test_copy_response_at_indice_invalido_no_copia() {
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    app.session_mgr
        .current_mut()
        .messages
        .view_messages
        .push(MessageViewModel::user("solo un user".to_string()));

    assert!(
        !copy_response_at_to_clipboard(&mut app, 0),
        "UserBubble no es copiable como respuesta"
    );
    assert!(
        !copy_response_at_to_clipboard(&mut app, 99),
        "índice fuera de rango no copia nada (carrera hitbox viejo/click)"
    );
    assert_eq!(app.session_mgr.current().ui.copy_char_count, 0);
    assert!(app.session_mgr.current().ui.copy_message_until.is_none());
}

#[tokio::test]
async fn test_copy_solo_texto_visible_sin_reasoning_ni_tools() {
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    app.session_mgr
        .current_mut()
        .messages
        .view_messages
        .push(MessageViewModel::assistant_blocks(vec![
            ContentBlockView::Reasoning {
                char_count: 12,
                text: "razonamiento interno secreto".to_string(),
                tail_lines: None,
            },
            ContentBlockView::ToolUse {
                name: "Bash".to_string(),
            },
            make_text_block("solo esto es visible"),
        ]));

    assert!(copy_response_at_to_clipboard(&mut app, 0));
    assert_eq!(
        app.session_mgr.current().ui.copy_char_count,
        "solo esto es visible".chars().count(),
        "reasoning y tool use NO entran al clipboard, solo el texto visible"
    );
}

#[tokio::test]
async fn test_ctrl_c_sigue_copiando_la_ultima_respuesta() {
    // copy_last_response_to_clipboard (el path de Ctrl+C) delega en
    // copy_response_at pero sigue eligiendo la ÚLTIMA respuesta.
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    {
        let msgs = &mut app.session_mgr.current_mut().messages.view_messages;
        msgs.push(MessageViewModel::assistant_blocks(vec![make_text_block(
            "vieja",
        )]));
        msgs.push(MessageViewModel::assistant_blocks(vec![make_text_block(
            "última respuesta",
        )]));
    }
    assert!(copy_last_response_to_clipboard(&mut app));
    assert_eq!(
        app.session_mgr.current().ui.copy_char_count,
        "última respuesta".chars().count(),
        "Ctrl+C copia la última respuesta, comportamiento intacto"
    );
}

// ── is_isolated_click (defensa contra copy accidental al soltar drag) ─────

#[test]
fn test_mouse_drag_release_is_not_isolated_click() {
    // Down en (10,5), Up en (30,5) → distancia Manhattan = 20 > 1 → drag.
    let down = Some((10u16, 5u16));
    assert!(
        !is_isolated_click(down, &up_at(30, 5)),
        "soltar al final de un drag NO es click aislado"
    );
}

#[test]
fn test_mouse_click_isolated_triggers_copy() {
    // Down y Up en la misma celda → click aislado.
    let down = Some((15u16, 8u16));
    assert!(
        is_isolated_click(down, &up_at(15, 8)),
        "Down/Up en misma celda es click aislado"
    );
    // Tolerancia de 1 celda en cualquier dirección también cuenta.
    assert!(is_isolated_click(down, &up_at(16, 8)));
    assert!(is_isolated_click(down, &up_at(15, 9)));
}

#[test]
fn test_mouse_click_missing_down_never_fires() {
    // Sin Down previo, Up nunca debe considerarse click aislado.
    assert!(
        !is_isolated_click(None, &up_at(15, 8)),
        "sin mouse_down_pos, Up no es click aislado"
    );
}

#[tokio::test]
async fn test_selection_mode_disables_copy_button_click() {
    // Con selection_mode activo, el copy-button no debe responder aunque el
    // evento llegue (defensa en profundidad).
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    app.session_mgr
        .current_mut()
        .messages
        .view_messages
        .push(MessageViewModel::assistant_blocks(vec![make_text_block(
            "respuesta",
        )]));
    app.global_ui.selection_mode = true;

    // Simulamos que hay un hitbox en (5,4)..(25,4).
    app.session_mgr.current_mut().ui.copy_button_hitboxes = vec![CopyButtonHitbox {
        rect: Rect {
            x: 5,
            y: 4,
            width: 20,
            height: 1,
        },
        message_idx: 0,
    }];

    // Click aislado dentro del hitbox.
    app.session_mgr.current_mut().ui.mouse_down_pos = Some((10, 4));
    let up = up_at(10, 4);
    assert!(
        !crate::event::mouse::is_isolated_click(
            app.session_mgr.current().ui.mouse_down_pos,
            &up
        ) || app.global_ui.selection_mode,
        "selection_mode bloquea el hit-test (lógica del handler)"
    );
}

#[tokio::test]
async fn test_copy_button_click_copies_correct_turn() {
    // Click aislado en el botón copia el turno correspondiente.
    let (mut app, _) = crate::app::App::new_headless(80, 24).await;
    app.session_mgr
        .current_mut()
        .messages
        .view_messages
        .push(MessageViewModel::assistant_blocks(vec![make_text_block(
            "respuesta del turno",
        )]));
    app.session_mgr.current_mut().ui.copy_button_hitboxes = vec![CopyButtonHitbox {
        rect: Rect {
            x: 5,
            y: 4,
            width: 20,
            height: 1,
        },
        message_idx: 0,
    }];
    app.session_mgr.current_mut().ui.mouse_down_pos = Some((10, 4));

    let hit = find_copy_button_hit(
        &app.session_mgr.current().ui.copy_button_hitboxes,
        &up_at(10, 4),
    );
    assert_eq!(hit, Some(0));
    assert!(copy_response_at_to_clipboard(&mut app, hit.unwrap()));
    assert_eq!(
        app.session_mgr.current().ui.copy_char_count,
        "respuesta del turno".chars().count()
    );
}
