    fn make_agent(id: &str, task: &str, tools: usize, error: bool) -> AgentSummary {
        AgentSummary {
            agent_id: id.to_string(),
            task_preview: task.to_string(),
            tool_count: tools,
            is_error: error,
            final_result: if error {
                Some("failed".to_string())
            } else {
                Some("done".to_string())
            },
        }
    }

    #[test]
    fn test_render_batch_summary_collapsed() {
        let agents = vec![
            make_agent("agent-1", "task one", 3, false),
            make_agent("agent-2", "task two", 5, false),
            make_agent("agent-3", "task three", 0, false),
        ];
        let lines = render_batch_summary(&agents, &true);
        // Header + 3 行 agent 摘要 = 4 行
        assert_eq!(lines.len(), 4, "折叠态应有 header + 3 行摘要");
        // Header 应包含 "3 agents finished"
        let header_text: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            header_text.contains("3 agents finished"),
            "header 应显示 agent 数量: {}",
            header_text
        );
    }

    #[test]
    fn test_render_batch_summary_expanded() {
        let agents = vec![
            make_agent("agent-1", "task one", 3, false),
            make_agent("agent-2", "task two", 5, false),
        ];
        let lines = render_batch_summary(&agents, &false);
        // Header + 2 * (task_preview + final_result) = 5 行
        assert_eq!(lines.len(), 5, "展开态应有 header + 2*(task+result)");
    }

    #[test]
    fn test_render_batch_summary_with_error() {
        let agents = vec![
            make_agent("agent-1", "task one", 3, false),
            make_agent("agent-2", "task two", 1, true),
            make_agent("agent-3", "task three", 2, true),
        ];
        let lines = render_batch_summary(&agents, &true);
        let header_text: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            header_text.contains("2 failed"),
            "header 应显示失败数: {}",
            header_text
        );
    }

    #[test]
    fn test_render_batch_summary_tree_connectors() {
        let agents = vec![
            make_agent("agent-1", "task one", 3, false),
            make_agent("agent-2", "task two", 5, false),
            make_agent("agent-3", "task three", 0, false),
        ];
        let lines = render_batch_summary(&agents, &true);
        // 第一个 agent 应使用 ├─
        let line1_text: String = lines[1].spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            line1_text.contains("├─"),
            "非最后一个 agent 应使用 ├─: {}",
            line1_text
        );
        // 最后一个 agent 应使用 └─
        let line3_text: String = lines[3].spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            line3_text.contains("└─"),
            "最后一个 agent 应使用 └─: {}",
            line3_text
        );
    }

    #[test]
    fn test_render_single_agent_unchanged() {
        // batch_agents 为空时走现有渲染路径，不经过 render_batch_summary
        // 此测试验证 render_batch_summary 对空 agents 列表的边界行为
        let agents: Vec<AgentSummary> = vec![];
        let lines = render_batch_summary(&agents, &true);
        assert_eq!(lines.len(), 1, "空 agents 应只有 header");
        let header_text: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            header_text.contains("0 agents"),
            "header 应包含 0 agents: {}",
            header_text
        );
    }

    // ─── 从 headless_test.rs 迁移的 render_view_model 测试 ──────────────────

    #[test]
    fn test_system_note_error_detection() {
        let error_content = "Compact failed: No LLM Provider";
        assert!(
            error_content.contains("failed") || error_content.contains("Compact failed"),
            "应检测到错误标记"
        );
        let warn_content = "⚠ Interrupted";
        assert!(warn_content.contains("⚠"), "应检测到警告标记");
        let info_content = "Configuration saved";
        assert!(
            !info_content.contains("❌")
                && !info_content.contains("failed")
                && !info_content.contains("⚠"),
            "普通消息不应被标记为错误"
        );
    }

    #[test]
    fn test_tool_block_error_visible_when_collapsed() {
        use crate::app::MessageViewModel;
        let vm = MessageViewModel::ToolBlock {
            tool_name: "Bash".to_string(),
            tool_call_id: "tc_err".to_string(),
            display_name: "Shell".to_string(),
            args_display: Some("bad_command".to_string()),
            content: "command not found: bad_command\nexit code 127".to_string(),
            is_error: true,
            collapsed: true,
            color: crate::ui::theme::ERROR,
            diff_lines: None,
            content_hash: 0,
        };
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        assert!(
            lines.len() >= 3,
            "collapsed error ToolBlock should have header + error lines, got {}",
            lines.len()
        );
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("command not found"),
            "error content should be visible: {}",
            text
        );
    }

    #[test]
    fn test_tool_block_success_no_summary_when_collapsed() {
        use crate::app::MessageViewModel;
        let vm = MessageViewModel::ToolBlock {
            tool_name: "Read".to_string(),
            tool_call_id: "tc_ok".to_string(),
            display_name: "Read".to_string(),
            args_display: Some("file.txt".to_string()),
            content: "file contents here".to_string(),
            is_error: false,
            collapsed: true,
            color: crate::ui::theme::SAGE,
            diff_lines: None,
            content_hash: 0,
        };
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        assert_eq!(
            lines.len(),
            1,
            "successful collapsed ToolBlock should have only header"
        );
    }

    #[test]
    fn test_tool_call_group_error_visible_when_collapsed() {
        use crate::app::MessageViewModel;
        use crate::ui::message_view::{ToolCategory, ToolEntry};

        let vm = MessageViewModel::ToolCallGroup {
            category: ToolCategory::Read,
            tools: vec![
                ToolEntry {
                    tool_name: "Read".to_string(),
                    display_name: "Read".to_string(),
                    args_display: Some("ok_file.txt".to_string()),
                    content: "ok content".to_string(),
                    is_error: false,
                },
                ToolEntry {
                    tool_name: "Read".to_string(),
                    display_name: "Read".to_string(),
                    args_display: Some("missing.txt".to_string()),
                    content: "Error: file not found".to_string(),
                    is_error: true,
                },
            ],
            collapsed: true,
            content_hash: 0,
        };
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("Error: file not found"),
            "error from failed tool should be visible: {}",
            text
        );
        assert!(
            !text.contains("ok content"),
            "successful tool content should NOT be visible: {}",
            text
        );
    }

    #[test]
    fn test_subagent_group_error_red_title_and_summary() {
        use crate::app::MessageViewModel;
        let vm = MessageViewModel::SubAgentGroup {
            agent_id: "test-agent".to_string(),
            task_preview: "do something risky".to_string(),
            total_steps: 3,
            recent_messages: Vec::new(),
            is_running: false,
            collapsed: true,
            final_result: Some("Agent failed: permission denied".to_string()),
            is_error: true,
            is_background: false,
            bg_hash: Some("abc123".to_string()),
            batch_agents: Vec::new(),
            instance_id: None,
            content_hash: 0,
        };
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        let title_color = lines
            .first()
            .and_then(|l| l.spans.get(1).and_then(|s| s.style.fg));
        assert_eq!(
            title_color,
            Some(crate::ui::theme::ERROR),
            "title should be red on error"
        );
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("Agent failed"),
            "error summary should be visible: {}",
            text
        );
    }

    #[test]
    fn test_render_system_reminder_user_bubble() {
        let mut vm = MessageViewModel::user("irrelevant content".to_string());
        if let MessageViewModel::UserBubble { system_reminder, .. } = &mut vm {
            *system_reminder = true;
        }
        vm.recompute_hash();
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        assert_eq!(lines.len(), 1, "系统提醒应只渲染一行");
        let text: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(text.contains("Context compacted"), "应显示压缩提示文字，实际: {}", text);
    }

    #[test]
    fn test_render_normal_user_bubble_unchanged() {
        let vm = MessageViewModel::user("Hello World".to_string());
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        let first_text: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(first_text.contains("\u{276f}"), "普通消息应有 ❯ 前缀");
        assert!(first_text.contains("Hello"), "应包含原始内容");
    }

    #[test]
    fn test_user_and_nexum_messages_are_visually_different() {
        let user_vm = MessageViewModel::user("User question".to_string());
        let nexum_vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Nexum answer".to_string(),
                rendered: ratatui::text::Text::raw("Nexum answer"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);

        let (user_lines, _) = render_view_model(&user_vm, Some(0), 80, false, "Ctrl+C copy", "copied", None, None);
        let (nexum_lines, _) = render_view_model(&nexum_vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);

        // User has ❯ prefix, Nexum has ◇ header
        let user_first: String = user_lines[0].spans.iter().map(|s| s.content.clone()).collect();
        let nexum_first: String = nexum_lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(user_first.contains("❯"), "user should have ❯ prefix");
        assert!(nexum_first.contains("◇"), "nexum should have ◇ prefix");
        assert!(nexum_first.contains("NEXUM"), "nexum header should say NEXUM");
        // They should not look the same
        assert_ne!(user_first, nexum_first, "user and nexum headers must differ");
    }

    #[test]
    fn test_assistant_raw_text_extracts_content() {
        let vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Hello from Nexum".to_string(),
                rendered: ratatui::text::Text::raw("Hello from Nexum"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        let text = vm.assistant_raw_text();
        assert_eq!(text, Some("Hello from Nexum".to_string()));
    }

    #[test]
    fn test_user_bubble_has_no_raw_text() {
        let vm = MessageViewModel::user("User says hi".to_string());
        assert!(vm.assistant_raw_text().is_none(), "UserBubble should not have assistant_raw_text");
    }

    #[test]
    fn test_nexum_header_has_left_border() {
        let vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "response".to_string(),
                rendered: ratatui::text::Text::raw("response"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        // First line should start with ▌ (left border accent)
        let first_span = &lines[0].spans[0];
        assert_eq!(first_span.content, "▌", "Nexum header should have left border ▌");
    }

    #[test]
    fn test_user_bubble_has_left_border() {
        let vm = MessageViewModel::user("test".to_string());
        let (lines, _) = render_view_model(&vm, Some(0), 80, false, "Ctrl+C copy", "copied", None, None);
        // First line should start with ▌ (left border)
        let first_span = &lines[0].spans[0];
        assert_eq!(first_span.content, "▌", "User message should have left border ▌");
    }

    #[test]
    fn test_assistant_header_says_nexum_uppercase() {
        let vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Hello".to_string(),
                rendered: ratatui::text::Text::raw("Hello"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        let header_text: String = lines[0].spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            header_text.contains("NEXUM"),
            "header should say NEXUM, got {}",
            header_text
        );
        assert!(
            !header_text.contains("Nexum"),
            "header should not say Nexum, got {}",
            header_text
        );
    }

    #[test]
    fn test_assistant_reasoning_not_in_body() {
        let vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Answer".to_string(),
                rendered: ratatui::text::Text::raw("Answer"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
            crate::ui::message_view::ContentBlockView::Reasoning {
                char_count: 211,
                text: "thinking...".to_string(),
                tail_lines: None,
            },
        ]);
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            !text.contains("Thought for"),
            "reasoning should not appear in body: {}",
            text
        );
    }

    /// Sprint copy-parcial 2026-07-07: el footer por-bloque YA NO repite un
    /// label persistente "Ctrl+C copy" (confundía y no es botón). El
    /// recordatorio del atajo vive UNA vez en el footer global de la
    /// statusbar. El footer por-bloque sólo muestra separador + metadata.
    #[test]
    fn test_assistant_footer_no_repite_copy_label_persistente() {
        let vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Hello".to_string(),
                rendered: ratatui::text::Text::raw("Hello"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        // copied_label_until = None (estado normal, sin copiar recién).
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copiado", None, None);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            !text.contains("Ctrl+C") && !text.contains("copy"),
            "el footer por-bloque NO debe repetir el label de copy en estado \
             normal; texto: {}",
            text
        );
    }

    /// Invariante de honestidad (sprint copy-hitbox 2026-07-07): el footer NO
    /// es un botón clickeable — es un recordatorio del atajo. Por lo tanto el
    /// label DEBE nombrar la tecla real que copia (Ctrl+C), no prometer un
    /// click. Este test guarda contra que el label derive a algo que no sea la
    /// tecla que efectivamente ejecuta la copia (copy_last_response_to_clipboard,
    /// bindeada a Ctrl+C en normal_keys.rs).
    #[test]
    fn test_footer_copy_label_nombra_la_tecla_real() {
        use crate::ui::message_render::copy_labels;
        for locale in ["es-AR", "zh-CN", "en-US", "desconocido"] {
            let (copy_label, _copied) = copy_labels(locale);
            assert!(
                copy_label.contains("Ctrl+C"),
                "el label del footer debe nombrar la tecla real (Ctrl+C), no \
                 prometer un botón; locale {locale} dio {copy_label:?}"
            );
        }
    }

    #[test]
    fn test_assistant_footer_shows_char_count() {
        let mut vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Hello world".to_string(),
                rendered: ratatui::text::Text::raw("Hello world"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        if let MessageViewModel::AssistantBubble { char_count, .. } = &mut vm {
            *char_count = 11;
        }
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "copied", None, None);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("11 chars"),
            "footer should show char count: {}",
            text
        );
    }

    #[test]
    fn test_assistant_footer_shows_copied_when_set() {
        // Sprint copy-button 2026-07-08: el feedback "copiado" vive EN el botón.
        let mut vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: "Hello".to_string(),
                rendered: ratatui::text::Text::raw("Hello"),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        if let MessageViewModel::AssistantBubble { copied_label_until, char_count, .. } = &mut vm {
            *copied_label_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
            *char_count = 5;
        }
        let (lines, _) = render_view_model_with_copy_button(
            &vm,
            Some(1),
            80,
            false,
            "Ctrl+C copy",
            "Copied",
            Some("Copy"),
            None,
        );
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("✓ Copied"),
            "footer debe mostrar el botón en estado copiado: {}",
            text
        );
    }
    // ─── Turn card render (sprint turn-card 2026-07-07) ──────────────────────

    fn assistant_bubble_flags(raw: &str, show_header: bool, show_footer: bool) -> MessageViewModel {
        let char_count = raw.chars().count();
        let mut vm = MessageViewModel::assistant_blocks(vec![
            crate::ui::message_view::ContentBlockView::Text {
                raw: raw.to_string(),
                rendered: ratatui::text::Text::raw(raw.to_string()),
                dirty: false,
                rendered_prefix_len: 0,
                rendered_prefix_lines: 0,
                holdback_scanner: Default::default(),
            },
        ]);
        if let MessageViewModel::AssistantBubble {
            show_header: h,
            show_footer: f,
            char_count: c,
            elapsed_ms: e,
            ..
        } = &mut vm
        {
            *h = show_header;
            *f = show_footer;
            *c = char_count;
            *e = 1234; // 1s+ para que meta incluya tiempo
        }
        vm
    }

    fn render_text(vm: &MessageViewModel) -> String {
        render_view_model(vm, Some(1), 80, false, "Ctrl+C copy", "copiado", None, None)
            .0
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn test_continuation_bubble_no_repite_header_nexum() {
        // Preamble (primera del turno) muestra NEXUM.
        let first = assistant_bubble_flags("Voy a explorar", true, false);
        assert!(render_text(&first).contains("NEXUM"), "la primera muestra NEXUM");
        // Continuación (no primera) NO repite el header.
        let cont = assistant_bubble_flags("Aclaración", false, false);
        assert!(
            !render_text(&cont).contains("NEXUM"),
            "la continuación NO repite el header NEXUM: {}",
            render_text(&cont)
        );
    }

    #[test]
    fn test_solo_ultima_burbuja_muestra_footer_metadata() {
        // Con char_count seteado, solo la que tiene show_footer muestra la metadata.
        let mut mid = assistant_bubble_flags("intermedia", false, false);
        let mut last = assistant_bubble_flags("final", false, true);
        for vm in [&mut mid, &mut last] {
            if let MessageViewModel::AssistantBubble { char_count, .. } = vm {
                *char_count = 42;
            }
        }
        assert!(
            !render_text(&mid).contains("42 chars"),
            "la intermedia NO muestra footer/metadata: {}",
            render_text(&mid)
        );
        assert!(
            render_text(&last).contains("42 chars"),
            "la última SÍ muestra la metadata: {}",
            render_text(&last)
        );
    }

    // ── Botón de copia minimalista (sprint copy-button 2026-07-08) ─────────

    fn render_with_button(vm: &MessageViewModel) -> (Vec<Line<'static>>, Option<CopyButtonRel>) {
        render_view_model_with_copy_button(
            vm,
            Some(1),
            80,
            false,
            "Ctrl+C copy",
            "Copiado",
            Some("Copiar"),
            None,
        )
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn test_boton_copiar_aparece_al_final_del_turno() {
        // Burbuja final del turno (show_footer=true), streaming terminado.
        let vm = assistant_bubble_flags("respuesta final", true, true);
        let (lines, rel) = render_with_button(&vm);
        let rel = rel.expect("turno completado con footer debe tener botón");
        assert_eq!(
            rel.line_offset,
            lines.len() - 1,
            "el botón es la ÚLTIMA línea del card"
        );
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            btn_line.contains("📋 Copiar"),
            "la línea del botón muestra el label minimalista: {btn_line}"
        );
        assert!(
            !btn_line.contains('[') && !btn_line.contains(']'),
            "no debe tener corchetes: {btn_line}"
        );
        assert!(
            rel.col_start > 10,
            "botón inline alineado a la derecha; col_start={}",
            rel.col_start
        );
        // El hitbox incluye padding horizontal alrededor del texto.
        let text_width = unicode_width::UnicodeWidthStr::width("📋 Copiar") as u16;
        assert!(
            rel.width > text_width,
            "ancho del hitbox incluye padding horizontal: width={} text_width={}",
            rel.width,
            text_width
        );
    }

    #[test]
    fn test_boton_no_aparece_durante_streaming() {
        let mut vm = assistant_bubble_flags("respuesta a medias", true, true);
        if let MessageViewModel::AssistantBubble { is_streaming, .. } = &mut vm {
            *is_streaming = true;
        }
        let (lines, rel) = render_with_button(&vm);
        assert!(rel.is_none(), "sin botón mientras Nexum sigue respondiendo");
        assert!(
            !lines.iter().any(|l| line_text(l).contains("Copiar")),
            "ninguna línea contiene el label durante streaming"
        );
    }

    #[test]
    fn test_boton_no_aparece_en_burbuja_intermedia() {
        // show_footer=false → burbuja intermedia del turno.
        let vm = assistant_bubble_flags("bloque intermedio", false, false);
        let (lines, rel) = render_with_button(&vm);
        assert!(rel.is_none(), "las burbujas intermedias no llevan botón");
        assert!(
            !lines.iter().any(|l| line_text(l).contains("Copiar")),
            "sin label en burbujas intermedias"
        );
    }

    #[test]
    fn test_boton_uno_solo_por_turno() {
        // Turno de 2 burbujas: solo la última (show_footer) tiene botón.
        let first = assistant_bubble_flags("preámbulo", true, false);
        let last = assistant_bubble_flags("respuesta final", false, true);
        let (_, rel_first) = render_with_button(&first);
        let (_, rel_last) = render_with_button(&last);
        assert!(rel_first.is_none(), "la primera burbuja del turno sin botón");
        assert!(rel_last.is_some(), "solo la última burbuja lleva el botón");
    }

    #[test]
    fn test_render_view_model_plano_no_genera_boton() {
        // El path sin label (recursión de SubAgentGroup, callers legacy)
        // nunca dibuja el botón — no hay botones dentro de tools/subagents.
        let vm = assistant_bubble_flags("respuesta", true, true);
        let (lines, _) = render_view_model(&vm, Some(1), 80, false, "Ctrl+C copy", "Copiado", None, None);
        assert!(
            !lines.iter().any(|l| line_text(l).contains("Copiar")),
            "render_view_model sin label no genera botón"
        );
    }

    #[test]
    fn test_copy_button_inline_position_wide_terminal() {
        // Ancho 80: meta + botón caben en la misma línea.
        let vm = assistant_bubble_flags("respuesta final", true, true);
        let (lines, rel) = render_view_model_with_copy_button(
            &vm,
            Some(1),
            80,
            false,
            "Ctrl+C copy",
            "Copied",
            Some("Copy"),
            None,
        );
        let rel = rel.expect("turno completado con footer debe tener botón");
        let footer_line = line_text(&lines[rel.line_offset]);
        assert!(
            footer_line.contains("📋 Copy"),
            "línea del footer debe contener el botón: {footer_line}"
        );
        assert!(
            footer_line.contains("chars"),
            "en ancho wide, meta y botón comparten la misma línea: {footer_line}"
        );
        assert!(
            rel.col_start > 10,
            "botón inline alineado a la derecha; col_start={}",
            rel.col_start
        );
    }

    #[test]
    fn test_copy_button_fallback_narrow_terminal() {
        // Ancho 25: meta y botón NO caben inline → botón en línea propia.
        let vm = assistant_bubble_flags("respuesta final", true, true);
        let (lines, rel) = render_view_model_with_copy_button(
            &vm,
            Some(1),
            25,
            false,
            "Ctrl+C copy",
            "Copied",
            Some("Copy"),
            None,
        );
        let rel = rel.expect("botón presente");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            btn_line.contains("📋 Copy"),
            "línea del botón muestra el label: {btn_line}"
        );
        assert!(
            !btn_line.contains("chars"),
            "en narrow, botón está en su propia línea: {btn_line}"
        );
        let meta_line = line_text(&lines[rel.line_offset - 1]);
        assert!(
            meta_line.contains("chars"),
            "la línea anterior es el meta: {meta_line}"
        );
        assert!(
            !meta_line.contains("Copy"),
            "meta no contiene el botón: {meta_line}"
        );
        assert!(rel.col_start > 2, "col_start a la derecha en línea propia");
    }

    #[test]
    fn test_copy_button_narrow_hides_icon() {
        // Ancho 9, contenido corto: icon+text no cabe, texto solo sí.
        let vm = assistant_bubble_flags("ok", true, true);
        let (lines, rel) = render_view_model_with_copy_button(
            &vm,
            Some(1),
            9,
            false,
            "Ctrl+C copy",
            "Copied",
            Some("Copy"),
            None,
        );
        let rel = rel.expect("botón presente en modo texto solo");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            btn_line.contains("Copy"),
            "muestra texto sin ícono: {btn_line}"
        );
        assert!(
            !btn_line.contains('📋'),
            "no muestra ícono si no entra: {btn_line}"
        );
    }

    #[test]
    fn test_copy_button_too_narrow_hides() {
        // Ancho 6: ni siquiera el texto solo cabe con padding.
        let vm = assistant_bubble_flags("ok", true, true);
        let (lines, rel) = render_view_model_with_copy_button(
            &vm,
            Some(1),
            6,
            false,
            "Ctrl+C copy",
            "Copied",
            Some("Copy"),
            None,
        );
        assert!(rel.is_none(), "botón se oculta si no entra");
        assert!(
            !lines.iter().any(|l| line_text(l).contains("Copy")),
            "no queda rastro del botón"
        );
    }

    #[test]
    fn test_copy_button_no_brackets() {
        let vm = assistant_bubble_flags("respuesta final", true, true);
        let (lines, rel) = render_with_button(&vm);
        let rel = rel.expect("botón presente");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            !btn_line.contains('[') && !btn_line.contains(']'),
            "el botón no debe tener corchetes: {btn_line}"
        );
    }

    #[test]
    fn test_copy_button_no_long_text() {
        let vm = assistant_bubble_flags("respuesta final", true, true);
        let (lines, rel) = render_with_button(&vm);
        let rel = rel.expect("botón presente");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            !btn_line.contains("Copiar respuesta") && !btn_line.contains("Copy response"),
            "el botón usa texto corto: {btn_line}"
        );
    }

    #[test]
    fn test_copy_button_copied_state_es() {
        let mut vm = assistant_bubble_flags("respuesta final", true, true);
        if let MessageViewModel::AssistantBubble { copied_label_until, .. } = &mut vm {
            *copied_label_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
        }
        let (lines, rel) = render_with_button(&vm);
        let rel = rel.expect("botón presente");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            btn_line.contains("✓ Copiado"),
            "tras copiar muestra estado copiado: {btn_line}"
        );
        assert!(!btn_line.contains('📋'), "no muestra ícono normal en estado copiado");
    }

    #[test]
    fn test_copy_button_copied_state_en() {
        let mut vm = assistant_bubble_flags("final answer", true, true);
        if let MessageViewModel::AssistantBubble { copied_label_until, .. } = &mut vm {
            *copied_label_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
        }
        let (lines, rel) = render_view_model_with_copy_button(
            &vm,
            Some(1),
            80,
            false,
            "Ctrl+C copy",
            "Copied",
            Some("Copy"),
            None,
        );
        let rel = rel.expect("botón presente");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            btn_line.contains("✓ Copied"),
            "tras copiar muestra estado copiado EN: {btn_line}"
        );
    }

    #[test]
    fn test_copy_button_timeout_returns_to_normal() {
        let mut vm = assistant_bubble_flags("respuesta final", true, true);
        if let MessageViewModel::AssistantBubble { copied_label_until, .. } = &mut vm {
            *copied_label_until = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        }
        let (lines, rel) = render_with_button(&vm);
        let rel = rel.expect("botón presente");
        let btn_line = line_text(&lines[rel.line_offset]);
        assert!(
            btn_line.contains("📋 Copiar"),
            "tras expirar el timeout vuelve al estado normal: {btn_line}"
        );
        assert!(!btn_line.contains('✓'), "no muestra check en estado normal");
    }

    #[test]
    fn test_copy_button_label_matches_locale_es() {
        let lc = crate::i18n::LcRegistry::new(Some("es-AR"));
        assert_eq!(
            lc.tr("copy-response-button"),
            "Copiar",
            "label ES corto resuelto por LcRegistry"
        );
        assert_eq!(
            lc.tr("copy-response-copied"),
            "Copiado",
            "label ES copiado resuelto por LcRegistry"
        );
    }

    #[test]
    fn test_copy_button_label_matches_locale_en() {
        let lc = crate::i18n::LcRegistry::new(Some("en"));
        assert_eq!(
            lc.tr("copy-response-button"),
            "Copy",
            "label EN corto resuelto por LcRegistry"
        );
        assert_eq!(
            lc.tr("copy-response-copied"),
            "Copied",
            "label EN copiado resuelto por LcRegistry"
        );
    }

    #[test]
    fn test_copy_button_label_matches_locale_zh() {
        let lc = crate::i18n::LcRegistry::new(Some("zh-CN"));
        assert_eq!(
            lc.tr("copy-response-button"),
            "复制",
            "label ZH corto resuelto por LcRegistry"
        );
        assert_eq!(
            lc.tr("copy-response-copied"),
            "已复制",
            "label ZH copiado resuelto por LcRegistry"
        );
    }
