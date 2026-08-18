//! VoiceIntentRouter — transcript → Hormiguero → VoiceResponse.
//! La voz NUNCA llama al provider directamente: o resuelve local, o
//! (con allow_paid_ai) deja que el flujo normal escale con envelope.

use std::collections::BTreeMap;

use nexum_acp::task::{
    EvidencePolicy, ExecutionBudgetV1, OutputFormat, TaskEnvelopeV1, TaskEnvelopeVersion,
    TaskPriority, TaskRisk, TaskSource,
};

use super::{acp_turn::VoiceRouteDecision, make_speakable, HandledBy, HudHint, VoiceResponse};
use crate::hormiguero::{self, RouteOutcome};

pub struct VoiceIntentRouter;

impl VoiceIntentRouter {
    /// Ruta tipada para el primer turno remoto de voz. Hormiguero decide si se
    /// escala, pero la validación y la conversión al contrato ACP son locales,
    /// deterministas y fuerzan `source=Voice`.
    pub fn route_decision(transcript: &str, locale: &str) -> VoiceRouteDecision {
        if let Some(directive) = super::model_directive::parse(transcript) {
            return VoiceRouteDecision::ModelDirective { directive };
        }
        match hormiguero::bridge().route_text(transcript, locale) {
            RouteOutcome::LocalAnswer(answer) => VoiceRouteDecision::Local {
                response: VoiceResponse {
                    text_speakable: make_speakable(&answer),
                    text_full: answer,
                    handled_by: HandledBy::Local,
                    should_speak: true,
                    should_show_in_tui: true,
                    hud_hint: HudHint::None,
                },
                reason: "Hormiguero confirmó una respuesta local.".into(),
            },
            // Con envelope del sidecar (camino async futuro): úsalo si es válido.
            RouteOutcome::NeedsPaidAi(Some(envelope)) => {
                match to_acp_envelope(transcript, *envelope) {
                    Some(envelope) => VoiceRouteDecision::Escalate {
                        envelope,
                        reason: "Hormiguero clasificó la consulta como compleja.".into(),
                    },
                    None => escalate_from_transcript(transcript),
                }
            }
            // OMEGA Fase 4: el hot path in-process no trae envelope. Voice
            // construye uno mínimo desde el transcript (source=Voice) para que
            // la tarea compleja llegue al provider. Passthrough (Hormiguero
            // OFF) también escala: Voice funciona con o sin el pasillo.
            RouteOutcome::NeedsPaidAi(None) | RouteOutcome::Passthrough => {
                escalate_from_transcript(transcript)
            }
        }
    }

    /// Rutea un transcript (mock en F1). `allow_paid_ai:false` es la
    /// garantía dura de 0 llamadas pagas: needs_paid_ai ⇒ Blocked.
    pub fn route(transcript: &str, locale: &str, allow_paid_ai: bool) -> VoiceResponse {
        // Las directivas de modelo requieren el controlador ACP async. Esta
        // ruta de compatibilidad nunca lee/escribe settings ni pending files.
        if super::model_directive::parse(transcript).is_some() {
            return VoiceResponse::local(
                "Para cambiar el modelo usá una sesión de voz conectada por ACP.".into(),
                HudHint::None,
            );
        }
        // VoiceDirective de catálogo (F2.2.2): cambiar VOZ localmente.
        if let Some(d) = super::catalog::VoiceDirective::parse(transcript) {
            if d.previous {
                if let Ok(p) = super::profile::restore_previous() {
                    let msg = format!("Listo, volví a {}.", p.display_name);
                    return VoiceResponse::local(msg, HudHint::VoiceChanged);
                }
            } else if d.preview_requested && d.desired_pitch.is_none() && d.desired_warmth.is_none()
            {
                let cur = super::profile::load();
                let names: Vec<_> = super::catalog::preview_candidates(&cur.id)
                    .iter()
                    .map(|e| e.display_name.to_string())
                    .collect();
                let msg = format!(
                    "Puedo probarte estas opciones: {}. Decime cuál, o pedime una más grave o más cálida.",
                    names.join(", "));
                return VoiceResponse::local(msg, HudHint::None);
            } else if d.desired_pitch.is_some()
                || d.desired_warmth.is_some()
                || d.desired_energy.is_some()
            {
                let cur = super::profile::load();
                let (e, reason) = super::catalog::select(&d, &cur.id);
                if let Ok(p) = super::profile::apply_catalog_entry(&e, &reason, &d) {
                    let msg = format!("Listo, ahora hablo con {}.", p.display_name);
                    let mut r =
                        VoiceResponse::local(format!("{msg} ({reason})"), HudHint::VoiceChanged);
                    r.text_speakable = msg;
                    return r;
                }
            }
        }
        // VoiceDirective (F2.2.1): "hablá más grave/serio/suave…" se
        // resuelve 100% LOCAL (ni siquiera toca el sidecar): 0 tokens.
        let t = transcript.to_lowercase();
        if (t.contains("habl") || t.contains("voz") || t.contains("modo presentaci"))
            && (t.contains("grave")
                || t.contains("serio")
                || t.contains("seria")
                || t.contains("suave")
                || t.contains("tranquil")
                || t.contains("energ")
                || t.contains("rápid")
                || t.contains("rapid")
                || t.contains("lento")
                || t.contains("despacio")
                || t.contains("normal"))
        {
            if let Ok((_p, msg)) = super::profile::VoiceStyleController::apply_natural(transcript) {
                return VoiceResponse::local(msg, HudHint::VoiceChanged);
            }
        }
        match hormiguero::bridge().route_text(transcript, locale) {
            RouteOutcome::LocalAnswer(answer) => VoiceResponse {
                text_speakable: make_speakable(&answer),
                text_full: answer,
                handled_by: HandledBy::Local,
                should_speak: true,
                should_show_in_tui: true,
                hud_hint: HudHint::None,
            },
            RouteOutcome::NeedsPaidAi(envelope) if allow_paid_ai => {
                // F1: el escalado real lo hace el flujo normal del pasillo
                // (submit_message); acá solo se reporta la decisión + envelope.
                let reason = envelope
                    .as_ref()
                    .map(|e| e.escalation_reason.clone())
                    .unwrap_or_default();
                let full = format!(
                    "Esta tarea requiere el modelo avanzado ({reason}). \
                     Se generó un TaskEnvelope para escalar por el pasillo."
                );
                VoiceResponse {
                    text_speakable: make_speakable(&full),
                    text_full: full,
                    handled_by: HandledBy::PaidAi,
                    should_speak: true,
                    should_show_in_tui: true,
                    hud_hint: HudHint::None,
                }
            }
            RouteOutcome::NeedsPaidAi(_) => VoiceResponse {
                text_full: "Esa tarea requiere el modelo avanzado, y este modo \
                            de voz tiene la IA paga deshabilitada (allow_paid_ai=false). \
                            No se gastaron tokens. Podés pedirlo por texto en Nexum."
                    .to_string(),
                text_speakable: "Eso requiere el modelo avanzado y está \
                                 deshabilitado en este modo. No gasté tokens."
                    .to_string(),
                handled_by: HandledBy::Blocked,
                should_speak: true,
                should_show_in_tui: true,
                hud_hint: HudHint::None,
            },
            _ => VoiceResponse {
                // Passthrough/NeedsConfirmation/etc.: sidecar no disponible
                // o sin decisión local confiable → degradado honesto.
                text_full: "La voz local no está disponible ahora (el puente \
                            Hormiguero no responde). Nexum sigue funcionando \
                            normalmente por texto."
                    .to_string(),
                text_speakable: "La voz local no está disponible ahora.".to_string(),
                handled_by: HandledBy::Blocked,
                should_speak: false,
                should_show_in_tui: true,
                hud_hint: HudHint::None,
            },
        }
    }
}

/// Escalación de voz sin envelope del sidecar: construye uno mínimo desde el
/// transcript. Si el transcript está vacío, pide más contexto (nunca escala
/// una tarea vacía).
fn escalate_from_transcript(transcript: &str) -> VoiceRouteDecision {
    match minimal_voice_envelope(transcript) {
        Some(envelope) => VoiceRouteDecision::Escalate {
            envelope,
            reason: "Hormiguero derivó la consulta al modelo principal.".into(),
        },
        None => VoiceRouteDecision::NeedMoreContext {
            missing_fields: vec!["objetivo de la tarea".into()],
        },
    }
}

/// TaskEnvelopeV1 mínimo, determinístico e idempotente, para escalar un turno
/// de voz al provider cuando el hot path in-process decidió `Escalate`. Fuerza
/// `source=Voice` y no inventa contexto: objective = transcript.
fn minimal_voice_envelope(transcript: &str) -> Option<TaskEnvelopeV1> {
    let objective = transcript.trim().to_string();
    if objective.is_empty() {
        return None;
    }
    let mut metadata = BTreeMap::new();
    metadata.insert("classifier".into(), "hormiguero-fastpath".into());
    Some(TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: format!("voice-{:016x}", stable_id(transcript, &objective)),
        source: TaskSource::Voice,
        objective,
        user_input: transcript.trim().to_string(),
        session_id: String::new(),
        thread_id: String::new(),
        workspace: None,
        constraints: Vec::new(),
        allowed_tools: Vec::new(),
        evidence_refs: Vec::new(),
        success_criteria: Vec::new(),
        output_format: OutputFormat::Text,
        execution_budget: ExecutionBudgetV1::default(),
        evidence_policy: EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: TaskPriority::Normal,
        risk: TaskRisk::Low,
        sanitized_metadata: nexum_acp::task::sanitize_metadata(metadata),
    })
}

fn to_acp_envelope(
    transcript: &str,
    envelope: crate::hormiguero::TaskEnvelope,
) -> Option<TaskEnvelopeV1> {
    let objective = if envelope.normalized_request.trim().is_empty() {
        envelope.user_intent.trim().to_string()
    } else {
        envelope.normalized_request.trim().to_string()
    };
    if objective.is_empty() {
        return None;
    }
    if transcript.trim().is_empty() {
        return None;
    }
    let output_format = match envelope
        .required_output_format
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => OutputFormat::Json,
        "markdown" | "md" => OutputFormat::Markdown,
        _ => OutputFormat::Text,
    };
    let mut metadata = BTreeMap::new();
    metadata.insert("classifier".into(), "hormiguero".into());
    if !envelope.escalation_reason.trim().is_empty() {
        metadata.insert(
            "escalation_reason".into(),
            envelope.escalation_reason.trim().to_string(),
        );
    }
    Some(TaskEnvelopeV1 {
        version: TaskEnvelopeVersion::V1,
        envelope_id: format!("voice-{:016x}", stable_id(transcript, &objective)),
        source: TaskSource::Voice,
        objective,
        user_input: transcript.trim().to_string(),
        session_id: String::new(),
        thread_id: String::new(),
        workspace: None,
        constraints: envelope.constraints,
        allowed_tools: envelope.allowed_tools,
        evidence_refs: Vec::new(),
        success_criteria: Vec::new(),
        output_format,
        execution_budget: ExecutionBudgetV1::default(),
        evidence_policy: EvidencePolicy {
            require_evidence: false,
            minimum_evidence_refs: 0,
            allow_unverified_output: true,
        },
        priority: TaskPriority::Normal,
        risk: if envelope.safety_notes.is_empty() {
            TaskRisk::Low
        } else {
            TaskRisk::Medium
        },
        sanitized_metadata: nexum_acp::task::sanitize_metadata(metadata),
    })
}

fn stable_id(input: &str, objective: &str) -> u64 {
    // FNV-1a evita generar IDs aleatorios: el mismo clasificador e input dan
    // exactamente el mismo envelope, simplificando retries y pruebas.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input.bytes().chain([0]).chain(objective.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;
    use crate::hormiguero::http_testutil::mock_route_server;

    fn setup(port: u16) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hormiguero.port"), port.to_string()).unwrap();
        std::fs::write(dir.path().join("hormiguero.token"), "tok").unwrap();
        std::env::set_var("NEXUM_HORMIGUERO_RUNTIME_DIR", dir.path());
        std::env::set_var("NEXUM_HORMIGUERO", "on");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        dir
    }
    fn teardown() {
        std::env::remove_var("NEXUM_HORMIGUERO_RUNTIME_DIR");
        std::env::remove_var("NEXUM_HORMIGUERO");
    }

    const LOCAL: &str = r#"{"ok":true,"decision":"local_answer","answer":"Sí, estoy activo.","should_call_paid_ai":false,"task_envelope":null,"confidence":0.95}"#;
    const COMPLEX: &str = r#"{"ok":true,"decision":"needs_paid_ai","answer":null,"should_call_paid_ai":true,"task_envelope":{"user_intent":"x","normalized_request":"x","escalation_reason":"tarea compleja","confidence":0.9,"source":"voice"},"confidence":0.9}"#;

    #[test]
    fn test_trivial_es_local_con_speakable() {
        let _g = test_env_lock();
        crate::hormiguero::bridge().reset_for_test();
        let _d = setup(mock_route_server(200, LOCAL, 0));
        let r = VoiceIntentRouter::route("hola nexum estás", "es", false);
        teardown();
        assert_eq!(r.handled_by, HandledBy::Local);
        assert!(r.should_speak && r.should_show_in_tui);
        assert!(!r.text_speakable.is_empty(), "speakable_answer existe");
    }

    #[test]
    fn test_allow_paid_ai_false_bloquea_sin_gastar() {
        let _g = test_env_lock();
        crate::hormiguero::bridge().reset_for_test();
        let _d = setup(mock_route_server(200, COMPLEX, 0));
        let r = VoiceIntentRouter::route("analizá la arquitectura", "es", false);
        teardown();
        assert_eq!(r.handled_by, HandledBy::Blocked, "false ⇒ jamás paid_ai");
        assert!(r.text_full.contains("modelo avanzado"));
        assert!(r.text_full.contains("No se gastaron tokens"));
    }

    #[test]
    fn test_allow_paid_ai_true_reporta_envelope() {
        let _g = test_env_lock();
        crate::hormiguero::bridge().reset_for_test();
        let _d = setup(mock_route_server(200, COMPLEX, 0));
        let r = VoiceIntentRouter::route("analizá la arquitectura", "es", true);
        teardown();
        assert_eq!(r.handled_by, HandledBy::PaidAi);
        assert!(r.text_full.contains("TaskEnvelope"));
    }

    #[test]
    fn test_route_decision_preserva_envelope_tipado_de_voz() {
        let _g = test_env_lock();
        crate::hormiguero::bridge().reset_for_test();
        let _d = setup(mock_route_server(200, COMPLEX, 0));
        let decision = VoiceIntentRouter::route_decision("analizá la arquitectura", "es");
        teardown();
        let VoiceRouteDecision::Escalate { envelope, .. } = decision else {
            panic!("el clasificador complejo debe escalar");
        };
        assert_eq!(envelope.source, TaskSource::Voice);
        assert_eq!(envelope.user_input, "analizá la arquitectura");
        assert!(envelope.session_id.is_empty() && envelope.thread_id.is_empty());
        assert!(!envelope.envelope_id.is_empty());
    }

    /// OMEGA Fase 4: con el router in-process, un sidecar MUERTO ya no degrada
    /// los triviales de voz — se resuelven localmente, sin red, sin crash.
    #[test]
    fn test_sidecar_muerto_no_degrada_trivial_in_process() {
        let _g = test_env_lock();
        crate::hormiguero::bridge().reset_for_test();
        let dir = tempfile::tempdir().unwrap(); // sin port/token = sidecar muerto
        std::env::set_var("NEXUM_HORMIGUERO_RUNTIME_DIR", dir.path());
        std::env::set_var("NEXUM_HORMIGUERO", "on");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        let r = VoiceIntentRouter::route("hola", "es", false);
        teardown();
        assert_eq!(
            r.handled_by,
            HandledBy::Local,
            "trivial se resuelve in-process aunque el sidecar esté muerto"
        );
        assert!(r.should_speak, "el trivial local sí habla");
        assert!(!r.text_speakable.is_empty());
    }

    /// Una tarea compleja por voz escala al provider construyendo un envelope
    /// local (source=Voice) aunque el sidecar no publique uno.
    #[test]
    fn test_complejo_escala_con_envelope_local() {
        let _g = test_env_lock();
        crate::hormiguero::bridge().reset_for_test();
        std::env::set_var("NEXUM_HORMIGUERO", "on");
        std::env::remove_var("NEXUM_PUBLIC_DEMO");
        let decision = VoiceIntentRouter::route_decision("implementá un parser recursivo", "es");
        teardown();
        let VoiceRouteDecision::Escalate { envelope, .. } = decision else {
            panic!("compleja debe escalar con envelope local");
        };
        assert_eq!(envelope.source, TaskSource::Voice);
        assert_eq!(envelope.user_input, "implementá un parser recursivo");
        assert!(!envelope.envelope_id.is_empty());
    }
}
