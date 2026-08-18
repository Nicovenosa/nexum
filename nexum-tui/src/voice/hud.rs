//! HUD visual de voz v2 (F2.3): contenedor ÚNICO de la experiencia de
//! voz. Presencia LATERAL sutil (jamás alerta centrada), overlay real
//! LayerShellQt en KDE/Wayland, sin foco, sobre apps/juegos. Sin Electron.
//!
//! Renderer TONTO: toda la política (texto, contexto, acento, duración)
//! viene de `hud_model::frame_for`; el QML solo pinta el frame JSON.
//! Las notificaciones del sistema quedan SOLO como fallback sin QML.
//!
//! Config: NEXUM_VOICE_HUD_POSITION (bottom-right default | top-right |
//! bottom-left | top-left | center SOLO debug), _MARGIN (24), _MODE
//! (compact|minimal|debug), _REDUCE_MOTION (false).

use super::adapters::{OverlayState, VoiceOverlay};
use super::hud_model::{frame_for, ActiveRoute, EnginesInfo, HudContext, HudFrame, HudPhase};
use super::overlay_notify::NotificationOverlay;
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudPosition {
    BottomRight,
    TopRight,
    BottomLeft,
    TopLeft,
    Center, // SOLO debug/test — nunca default
}

pub struct HudConfig {
    pub position: HudPosition,
    pub margin: u32,
    pub minimal: bool,
    pub debug: bool,
    pub reduce_motion: bool,
}

impl HudConfig {
    pub fn from_env() -> Self {
        let position = match std::env::var("NEXUM_VOICE_HUD_POSITION")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "top-right" => HudPosition::TopRight,
            "bottom-left" => HudPosition::BottomLeft,
            "top-left" => HudPosition::TopLeft,
            // center: SOLO si además el modo es debug (doble llave).
            "center" => {
                if std::env::var("NEXUM_VOICE_HUD_MODE").as_deref() == Ok("debug") {
                    HudPosition::Center
                } else {
                    HudPosition::BottomRight
                }
            }
            _ => HudPosition::BottomRight,
        };
        let margin = std::env::var("NEXUM_VOICE_HUD_MARGIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24)
            .clamp(0, 200);
        let mode = std::env::var("NEXUM_VOICE_HUD_MODE").unwrap_or_default();
        let reduce_motion = matches!(
            std::env::var("NEXUM_VOICE_HUD_REDUCE_MOTION").as_deref(),
            Ok("1") | Ok("true") | Ok("on")
        );
        Self {
            position,
            margin,
            minimal: mode == "minimal",
            debug: mode == "debug",
            reduce_motion,
        }
    }

    /// (ancho, alto) compact ≤ 360×90 por spec (2 líneas); minimal 1 línea.
    pub fn size(&self) -> (u32, u32) {
        if self.minimal { (200, 44) } else { (310, 76) }
    }

    /// Anclas LayerShell: (anchor_flags_qml, margin). Testeable puro.
    pub fn anchors_qml(&self) -> &'static str {
        match self.position {
            HudPosition::BottomRight => "LayerShell.Window.AnchorBottom | LayerShell.Window.AnchorRight",
            HudPosition::TopRight => "LayerShell.Window.AnchorTop | LayerShell.Window.AnchorRight",
            HudPosition::BottomLeft => "LayerShell.Window.AnchorBottom | LayerShell.Window.AnchorLeft",
            HudPosition::TopLeft => "LayerShell.Window.AnchorTop | LayerShell.Window.AnchorLeft",
            HudPosition::Center => "0", // sin anclas → compositor centra (debug)
        }
    }
}

fn build_qml(cfg: &HudConfig) -> String {
    let (w, h) = cfg.size();
    let anchors = cfg.anchors_qml();
    let margin = cfg.margin;
    let anim = if cfg.reduce_motion { "false" } else { "true" };
    let ctx_visible = if cfg.minimal { "false" } else { "true" };
    let fade_ms = if cfg.reduce_motion { 0 } else { 500 };
    // Panel chico translúcido, borde 1px, dos líneas (estado + ruta),
    // punto pulsante, waveform mini (8 barras 3px), fade-out por ttl.
    format!(
        r##"
import QtQuick
import QtQuick.Window
import org.kde.layershell 1.0 as LayerShell

Window {{
    id: win
    width: {w}; height: {h}
    LayerShell.Window.layer: LayerShell.Window.LayerOverlay
    LayerShell.Window.anchors: {anchors}
    LayerShell.Window.margins.right: {margin}
    LayerShell.Window.margins.bottom: {margin}
    LayerShell.Window.margins.top: {margin}
    LayerShell.Window.margins.left: {margin}
    LayerShell.Window.keyboardInteractivity: LayerShell.Window.KeyboardInteractivityNone
    LayerShell.Window.exclusionZone: -1
    flags: Qt.FramelessWindowHint | Qt.WindowDoesNotAcceptFocus
    color: "transparent"
    visible: true

    property string state_id: "listening"
    property string label: "Nexum escuchando…"
    property string context_line: ""
    property color accent: "#7c5cff"
    property bool pulse: true
    property bool wave: false
    property bool animate: {anim}
    property int lastSeq: -1

    Rectangle {{
        id: panel
        anchors.fill: parent
        radius: 12
        color: Qt.rgba(0.086, 0.07, 0.13, 0.88)
        border.color: Qt.rgba(win.accent.r, win.accent.g, win.accent.b, 0.55)
        border.width: 1
        Behavior on opacity {{ NumberAnimation {{ duration: {fade_ms}; easing.type: Easing.InOutQuad }} }}

        Row {{
            anchors.verticalCenter: parent.verticalCenter
            anchors.left: parent.left
            anchors.leftMargin: 14
            spacing: 10
            Rectangle {{
                width: 8; height: 8; radius: 4
                anchors.verticalCenter: parent.verticalCenter
                color: win.accent
                SequentialAnimation on opacity {{
                    running: win.animate && win.pulse
                    loops: Animation.Infinite
                    NumberAnimation {{ to: 0.35; duration: 900; easing.type: Easing.InOutSine }}
                    NumberAnimation {{ to: 1.0; duration: 900; easing.type: Easing.InOutSine }}
                }}
            }}
            Column {{
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {{
                    text: win.label
                    color: "#ded9ec"
                    font.pixelSize: 13
                    elide: Text.ElideRight
                    width: {w} - 90
                }}
                Text {{
                    text: win.context_line
                    visible: {ctx_visible} && win.context_line !== ""
                    color: "#9a93b8"
                    font.pixelSize: 10
                    elide: Text.ElideRight
                    width: {w} - 90
                }}
            }}
            Row {{
                spacing: 3
                visible: win.wave
                anchors.verticalCenter: parent.verticalCenter
                Repeater {{
                    model: 8
                    Rectangle {{
                        width: 3; radius: 1.5
                        color: index % 2 ? "#25c8d8" : win.accent
                        height: 4
                        anchors.verticalCenter: parent.verticalCenter
                        SequentialAnimation on height {{
                            running: win.animate && win.wave
                            loops: Animation.Infinite
                            NumberAnimation {{ to: 4 + (index % 3) * 3; duration: 420 + index * 40; easing.type: Easing.InOutSine }}
                            NumberAnimation {{ to: 4; duration: 420 + index * 30; easing.type: Easing.InOutSine }}
                        }}
                    }}
                }}
            }}
        }}
    }}

    Timer {{
        id: hideTimer
        repeat: false
        onTriggered: panel.opacity = 0
    }}

    Timer {{
        interval: 180; running: true; repeat: true
        onTriggered: {{
            var xhr = new XMLHttpRequest()
            xhr.open("GET", "file://" + Qt.application.arguments[Qt.application.arguments.length-1] + "?" + Date.now())
            xhr.onreadystatechange = function() {{
                if (xhr.readyState !== XMLHttpRequest.DONE) return
                try {{
                    var st = JSON.parse(xhr.responseText)
                    if (st.state === "done") {{ Qt.quit(); return }}
                    if (st.seq !== win.lastSeq) {{
                        win.lastSeq = st.seq
                        panel.opacity = 1
                        hideTimer.stop()
                        if (st.ttl_ms > 0) {{ hideTimer.interval = st.ttl_ms; hideTimer.start() }}
                    }}
                    win.state_id = st.state
                    win.label = st.title
                    win.context_line = st.context
                    win.accent = st.accent
                    win.pulse = st.pulse
                    win.wave = st.waveform
                }} catch(e) {{ Qt.quit() }}
            }}
            xhr.send()
        }}
    }}
}}
"##
    )
}

fn qml_bin() -> Option<&'static str> {
    ["qml6", "qml"].into_iter().find(|b| {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(b).is_file()))
            .unwrap_or(false)
    })
}

pub struct VoiceHud {
    state_file: PathBuf,
    qml_child: Option<std::process::Child>,
    fallback: NotificationOverlay,
    ctx: HudContext,
    seq: u32,
    last_ttl: u32,
}

/// Mapa de compatibilidad F2.2 (VoiceOverlay sigue funcionando igual).
fn phase_for(state: OverlayState) -> HudPhase {
    match state {
        OverlayState::PreparingRuntime => HudPhase::PreparingRuntime,
        OverlayState::ReconnectingRuntime => HudPhase::ReconnectingRuntime,
        OverlayState::RuntimeUnavailable => HudPhase::RuntimeUnavailable,
        OverlayState::Listening => HudPhase::Listening,
        OverlayState::SpeechDetected => HudPhase::SpeechDetected,
        OverlayState::SilenceCountdown => HudPhase::SilenceCountdown,
        OverlayState::Transcribing => HudPhase::Transcribing,
        OverlayState::LocalHandling => HudPhase::LocalHandling,
        OverlayState::Escalating => HudPhase::Escalating,
        OverlayState::Speaking => HudPhase::Speaking,
        OverlayState::Error => HudPhase::Error,
        OverlayState::PrivacyBlocked => HudPhase::PrivacyBlocked,
    }
}

impl VoiceHud {
    pub fn spawn(runtime_dir: &Path) -> Self {
        let cfg = HudConfig::from_env();
        let ctx = HudContext {
            engines: EnginesInfo::detect(),
            route: ActiveRoute::Undecided,
            debug: cfg.debug,
        };
        let state_file = runtime_dir.join("voice-hud.json");
        let first = frame_for(HudPhase::Listening, &ctx, None);
        let _ = std::fs::write(&state_file, serde_json::to_string(&first).unwrap_or_default());
        let qml_child = qml_bin().and_then(|bin| {
            let qml_path = runtime_dir.join("voice-hud.qml");
            std::fs::write(&qml_path, build_qml(&cfg)).ok()?;
            std::process::Command::new(bin)
                .arg(&qml_path)
                .arg("--")
                .arg(&state_file)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()
        });
        Self {
            state_file,
            qml_child,
            fallback: NotificationOverlay::new(),
            ctx,
            seq: 0,
            last_ttl: 0,
        }
    }

    pub fn has_window(&self) -> bool {
        self.qml_child.is_some()
    }

    fn write_frame(&mut self, mut frame: HudFrame) {
        self.seq += 1;
        frame.seq = self.seq;
        self.last_ttl = frame.ttl_ms;
        let json = serde_json::to_string(&frame).unwrap_or_default();
        let tmp = self.state_file.with_extension("tmp");
        if std::fs::File::create(&tmp)
            .and_then(|mut f| f.write_all(json.as_bytes()))
            .is_ok()
        {
            let _ = std::fs::rename(&tmp, &self.state_file);
        }
    }

    /// API v2: fase + detalle opcional (respuesta corta, voz/modelo nuevo).
    pub fn show_phase(&mut self, phase: HudPhase, detail: Option<&str>) {
        // La fase actualiza la ruta activa del turno (línea 2 coherente).
        match phase {
            HudPhase::LocalHandling => self.ctx.route = ActiveRoute::Local,
            HudPhase::Escalating => self.ctx.route = ActiveRoute::Escalated,
            HudPhase::Listening => self.ctx.route = ActiveRoute::Undecided,
            _ => {}
        }
        let frame = frame_for(phase, &self.ctx, detail);
        self.write_frame(frame);
    }

    /// Respuesta final DENTRO del HUD (success, ttl elegante); la
    /// notificación del sistema es solo fallback sin ventana QML.
    pub fn show_answer(&mut self, text: &str, handled_by: &str) {
        if self.qml_child.is_some() {
            self.show_phase(HudPhase::Success, Some(&super::hud_model::hud_title(text)));
        } else {
            self.fallback.show_answer(text, handled_by);
        }
    }

    pub fn close(&mut self) {
        // Autodesaparición elegante: si el último frame tenía ttl, dejarlo
        // visible el tiempo prometido (cap 2.5s) antes de cerrar.
        if self.qml_child.is_some() && self.last_ttl > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.last_ttl.min(2500) as u64));
        }
        self.seq += 1;
        let json = format!(r#"{{"seq":{},"state":"done"}}"#, self.seq);
        let _ = std::fs::write(&self.state_file, json);
        if let Some(child) = self.qml_child.as_mut() {
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.state_file);
    }
}

impl VoiceOverlay for VoiceHud {
    fn show(&mut self, state: OverlayState) {
        self.show_phase(phase_for(state), None);
        if self.qml_child.is_none() {
            self.fallback.show(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hormiguero::bridge::test_env_lock;

    #[test]
    fn test_default_es_bottom_right_jamas_center() {
        let _g = test_env_lock();
        std::env::remove_var("NEXUM_VOICE_HUD_POSITION");
        std::env::remove_var("NEXUM_VOICE_HUD_MODE");
        let c = HudConfig::from_env();
        assert_eq!(c.position, HudPosition::BottomRight);
        assert!(c.anchors_qml().contains("AnchorBottom"));
        assert!(c.anchors_qml().contains("AnchorRight"));
    }

    #[test]
    fn test_center_solo_con_modo_debug() {
        let _g = test_env_lock();
        std::env::set_var("NEXUM_VOICE_HUD_POSITION", "center");
        std::env::remove_var("NEXUM_VOICE_HUD_MODE");
        assert_eq!(
            HudConfig::from_env().position,
            HudPosition::BottomRight,
            "center sin debug se degrada al default"
        );
        std::env::set_var("NEXUM_VOICE_HUD_MODE", "debug");
        assert_eq!(HudConfig::from_env().position, HudPosition::Center);
        assert!(HudConfig::from_env().debug);
        std::env::remove_var("NEXUM_VOICE_HUD_POSITION");
        std::env::remove_var("NEXUM_VOICE_HUD_MODE");
    }

    #[test]
    fn test_posiciones_laterales_y_compact_size() {
        let _g = test_env_lock();
        for (env, frag) in [
            ("top-right", "AnchorTop"),
            ("bottom-left", "AnchorLeft"),
            ("top-left", "AnchorTop"),
        ] {
            std::env::set_var("NEXUM_VOICE_HUD_POSITION", env);
            let c = HudConfig::from_env();
            assert!(c.anchors_qml().contains(frag), "{env}");
            let (w, h) = c.size();
            assert!(w <= 360 && h <= 90, "compact dentro del spec: {w}x{h}");
        }
        std::env::remove_var("NEXUM_VOICE_HUD_POSITION");
    }

    #[test]
    fn test_reduce_motion_y_minimal() {
        let _g = test_env_lock();
        std::env::set_var("NEXUM_VOICE_HUD_REDUCE_MOTION", "1");
        std::env::set_var("NEXUM_VOICE_HUD_MODE", "minimal");
        let c = HudConfig::from_env();
        assert!(c.reduce_motion && c.minimal);
        let qml = build_qml(&c);
        assert!(qml.contains("property bool animate: false"));
        assert!(qml.contains("visible: false && win.context_line"), "minimal oculta línea 2");
        let (w, h) = c.size();
        assert!(w <= 220 && h <= 48, "minimal más chico: {w}x{h}");
        std::env::remove_var("NEXUM_VOICE_HUD_REDUCE_MOTION");
        std::env::remove_var("NEXUM_VOICE_HUD_MODE");
    }

    #[test]
    fn test_qml_v2_lee_frame_completo_del_json() {
        let _g = test_env_lock();
        std::env::remove_var("NEXUM_VOICE_HUD_MODE");
        std::env::remove_var("NEXUM_VOICE_HUD_REDUCE_MOTION");
        let qml = build_qml(&HudConfig::from_env());
        for frag in ["st.seq", "st.ttl_ms", "st.title", "st.context", "st.accent", "st.pulse", "st.waveform", "hideTimer"] {
            assert!(qml.contains(frag), "renderer tonto consume {frag}");
        }
    }

    #[test]
    fn test_frames_escritos_con_seq_creciente_y_ruta() {
        let _g = test_env_lock();
        // Sin qml en PATH: no se abre ventana, pero los frames se escriben.
        let dir = tempfile::tempdir().unwrap();
        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", "/nonexistent");
        let mut hud = VoiceHud::spawn(dir.path());
        std::env::set_var("PATH", old_path.as_deref().unwrap_or_default());
        hud.show_phase(HudPhase::LocalHandling, None);
        let raw = std::fs::read_to_string(dir.path().join("voice-hud.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["state"], "local_handling");
        assert_eq!(v["seq"], 1);
        assert!(v["context"].as_str().unwrap().contains("Ruta activa: Hormiguero"), "{raw}");
        hud.show_phase(HudPhase::Escalating, None);
        let raw = std::fs::read_to_string(dir.path().join("voice-hud.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["seq"], 2, "seq crece por frame");
        assert!(v["context"].as_str().unwrap().starts_with("Ruta activa:"), "{raw}");
    }
}
