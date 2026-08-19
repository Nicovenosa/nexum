//! Nexum Voice Identity (F2.2): la voz de Nexum es una IDENTIDAD
//! configurable, no "el TTS instalado". Daniela = fallback local actual,
//! jamás hardcode conceptual. Config: ~/.nexum/voice/profile.json (0600).
//! Baseline low-cost CPU-first; engines cloud/GPU = metadata para F3+.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceProfile {
    pub id: String,
    pub display_name: String,
    pub engine: String,   // "piper" hoy; openvoice/xtts/riva = F3+
    pub voice_id: String, // .onnx stem para piper
    pub language: String,
    pub speed: f32, // 1.0 normal (piper: length_scale = 1/speed)
    pub pitch: f32, // semitonos; piper NO lo soporta (metadata honesta)
    pub volume: f32,
    pub tone: String, // "neutral"|"serious"|"soft"|"energetic"
    #[serde(default)]
    pub style_prompt: Option<String>, // para engines con style control (F3)
    #[serde(default)]
    pub supports_style_control: bool,
    #[serde(default)]
    pub supports_voice_clone: bool,
    #[serde(default)]
    pub fallback_profile: Option<String>,
    #[serde(default)]
    pub previous_profile: Option<Box<VoiceProfile>>,
    #[serde(default)]
    pub selection_reason: Option<String>,
    #[serde(default)]
    pub capability_degradations: Vec<String>,
}

/// Entrada local y compilada que asocia un locale con un perfil builtin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalVoiceManifestEntry {
    pub locale: &'static str,
    pub profile_id: &'static str,
}

/// El bootstrap no consulta catálogo ni red: este manifiesto es su única
/// fuente para elegir el perfil inicial.
pub const LOCAL_VOICE_MANIFEST: &[LocalVoiceManifestEntry] = &[LocalVoiceManifestEntry {
    locale: "es-AR",
    profile_id: "nexum_default",
}];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileLoadError {
    Read(String),
    Corrupt(String),
}

/// Perfiles builtin. `nexum_default` ES la identidad de Nexum: hoy rinde
/// sobre la mejor voz local disponible; cuando exista la voz propia
/// (NEXUM_VOICE_IDENTITY.md), solo cambia el voice_id de estos perfiles.
pub fn builtin_profiles() -> Vec<VoiceProfile> {
    let base = |id: &str, name: &str, tone: &str, speed: f32| VoiceProfile {
        id: id.into(),
        display_name: name.into(),
        engine: "piper".into(),
        voice_id: "es_AR-daniela-high".into(), // portador actual, no identidad
        language: "es".into(),
        speed,
        pitch: 0.0,
        volume: 1.0,
        tone: tone.into(),
        style_prompt: None,
        supports_style_control: false, // piper: solo speed
        supports_voice_clone: false,
        fallback_profile: Some("piper_daniela_fallback".into()),
        previous_profile: None,
        selection_reason: None,
        capability_degradations: vec![],
    };
    vec![
        base("nexum_default", "Nexum", "neutral", 1.0),
        base("nexum_serious", "Nexum (serio)", "serious", 0.92),
        base("nexum_soft", "Nexum (suave)", "soft", 0.96),
        base("nexum_fast", "Nexum (rápido)", "energetic", 1.18),
        VoiceProfile {
            fallback_profile: None,
            ..base(
                "piper_daniela_fallback",
                "Daniela (fallback local)",
                "neutral",
                1.0,
            )
        },
    ]
}

fn profile_dir() -> PathBuf {
    nexum_agent::config_home::nexum_home().join("voice")
}
fn profile_path() -> PathBuf {
    profile_dir().join("profile.json")
}

/// Carga un perfil persistido. La ausencia es normal durante el bootstrap;
/// JSON inválido es corrupción y debe quedar visible al caller.
pub fn load_from_path(path: &Path) -> Result<Option<VoiceProfile>, ProfileLoadError> {
    match std::fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|error| ProfileLoadError::Corrupt(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ProfileLoadError::Read(error.to_string())),
    }
}

/// Guarda por reemplazo atómico dentro del mismo directorio. El archivo queda
/// con permisos de dueño antes de hacerse visible como perfil activo.
pub fn save_to_path(path: &Path, profile: &VoiceProfile) -> Result<(), String> {
    let parent = path.parent().ok_or("profile path sin directorio")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }

    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        let json = serde_json::to_vec_pretty(profile).map_err(|error| error.to_string())?;
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temp, path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    write_result
}

/// Devuelve el perfil inicial de un locale sin E/S ni resolución remota.
pub fn resolve_local_default_profile(locale: &str) -> VoiceProfile {
    let locale = locale.replace('_', "-").to_ascii_lowercase();
    let entry = LOCAL_VOICE_MANIFEST
        .iter()
        .find(|entry| entry.locale.eq_ignore_ascii_case(&locale))
        .unwrap_or(&LOCAL_VOICE_MANIFEST[0]);
    builtin_profiles()
        .into_iter()
        .find(|profile| profile.id == entry.profile_id)
        .expect("el manifiesto local referencia un perfil builtin")
}

pub fn load() -> VoiceProfile {
    load_from_path(&profile_path())
        .ok()
        .flatten()
        .unwrap_or_else(|| resolve_local_default_profile("es-AR"))
}

pub fn save(p: &VoiceProfile) -> Result<(), String> {
    save_to_path(&profile_path(), p)
}

/// VoiceStyleController: órdenes de estilo → mutan el perfil, no el código.
pub struct VoiceStyleController;
impl VoiceStyleController {
    /// VoiceDirective natural ("más grave y tranquilo", "hablá más serio").
    /// Capability-aware: piper solo soporta speed → grave/suave mapean al
    /// perfil más cercano (pitch queda registrado para engines F3, sin
    /// fingir). 100% local, 0 tokens. Devuelve (perfil, respuesta corta).
    pub fn apply_natural(phrase: &str) -> Result<(VoiceProfile, String), String> {
        let f = phrase.to_lowercase();
        let has = |ws: &[&str]| ws.iter().any(|w| f.contains(w));
        let mut p = load();
        let mut changed = false;
        if has(&["normal", "reset", "como antes", "tu voz de siempre"]) {
            let np = Self::apply_style("reset")?;
            return Ok((np, "Listo, volví a mi voz normal.".into()));
        }
        if has(&["grave", "serio", "seria", "presentaci", "formal"]) {
            let cur_voice = p.voice_id.clone();
            p = builtin_profiles()
                .into_iter()
                .find(|b| b.id == "nexum_serious")
                .unwrap();
            p.voice_id = cur_voice;
            if f.contains("grave") {
                p.pitch = -2.0;
            } // engines F3; piper: perfil serio
            changed = true;
        }
        if has(&["suave", "tranquil", "calma", "dulce"]) {
            let cur_voice = p.voice_id.clone();
            let base = builtin_profiles()
                .into_iter()
                .find(|b| b.id == "nexum_soft")
                .unwrap();
            p.tone = base.tone;
            p.id = base.id;
            p.display_name = base.display_name;
            p.speed = p.speed.min(base.speed);
            p.voice_id = cur_voice;
            changed = true;
        }
        if has(&["energ", "rapid", "rápid", "animad"]) {
            p.speed = (p.speed + 0.15).clamp(0.5, 2.0);
            p.tone = "energetic".into();
            changed = true;
        }
        if has(&["lento", "más despacio", "despacio", "pausad"]) {
            p.speed = (p.speed - 0.12).clamp(0.5, 2.0);
            changed = true;
        }
        if !changed {
            return Err(format!("no entendí el estilo pedido: '{phrase}'"));
        }
        save(&p)?;
        Ok((
            p.clone(),
            format!(
                "Listo, ajusté mi voz: {} (velocidad {:.2}).",
                p.tone, p.speed
            ),
        ))
    }

    /// "serious"/"soft"/"fast"/"normal"|"reset" o alias hablados.
    pub fn apply_style(style: &str) -> Result<VoiceProfile, String> {
        let id = match style.to_ascii_lowercase().as_str() {
            "serious" | "serio" | "grave" | "presentacion" | "presentación" => "nexum_serious",
            "soft" | "suave" => "nexum_soft",
            "fast" | "rapido" | "rápido" => "nexum_fast",
            "normal" | "default" | "reset" => "nexum_default",
            other => {
                return Err(format!(
                    "estilo desconocido '{other}' (serious|soft|fast|normal)"
                ));
            }
        };
        let mut p = builtin_profiles().into_iter().find(|p| p.id == id).unwrap();
        // Conservar la voz elegida por el usuario (identidad ≠ estilo).
        let current = load();
        p.voice_id = current.voice_id;
        save(&p)?;
        Ok(p)
    }

    pub fn set_speed(speed: f32) -> Result<VoiceProfile, String> {
        let mut p = load();
        p.speed = speed.clamp(0.5, 2.0);
        save(&p)?;
        Ok(p)
    }

    pub fn set_pitch(pitch: f32) -> Result<VoiceProfile, String> {
        let mut p = load();
        p.pitch = pitch.clamp(-12.0, 12.0);
        save(&p)?;
        // Honesto: piper no aplica pitch; queda en el perfil para engines F3.
        Ok(p)
    }
}

/// Aplica una entrada del catálogo al perfil activo (guarda previous +
/// razón auditable + degradaciones honestas).
pub fn apply_catalog_entry(
    e: &super::catalog::VoiceCatalogEntry,
    reason: &str,
    directive: &super::catalog::VoiceDirective,
) -> Result<VoiceProfile, String> {
    let mut cur = load();
    let prev = cur.clone();
    cur.previous_profile = Some(Box::new(VoiceProfile {
        previous_profile: None,
        ..prev
    }));
    cur.engine = e.engine.to_string();
    cur.voice_id = e.engine_voice_id.to_string();
    cur.id = e.id.to_string();
    cur.display_name = e.display_name.to_string();
    cur.selection_reason = Some(reason.to_string());
    cur.capability_degradations.clear();
    if directive.desired_pitch.is_some() && !e.supports_pitch {
        cur.capability_degradations
            .push("pitch no soportado por el motor: se eligió otra voz".into());
    }
    if let Some(dp) = directive.desired_pace {
        cur.speed = (cur.speed + 0.12 * dp as f32).clamp(0.5, 2.0);
    }
    save(&cur)?;
    Ok(cur)
}

/// "Volvé a la voz anterior".
pub fn restore_previous() -> Result<VoiceProfile, String> {
    let mut cur = load();
    match cur.previous_profile.take() {
        Some(prev) => {
            let mut p = *prev;
            p.previous_profile = Some(Box::new(cur)); // cur ya sin previous
            save(&p)?;
            Ok(p)
        }
        None => Err("no hay voz anterior guardada".into()),
    }
}

/// Metadata de engines TTS (regla: todo motor declara su costo real).
pub struct EngineInfo {
    pub id: &'static str,
    pub local: bool,
    pub cost: &'static str,
    pub needs_gpu: bool,
    pub ram: &'static str,
    pub latency: &'static str,
    pub style_control: bool,
    pub cloning: bool,
    pub profile_tier: &'static str, // low|medium|power|cloud-opt-in
    pub status: &'static str,
}

pub fn engines_catalog() -> Vec<EngineInfo> {
    vec![
        EngineInfo {
            id: "piper",
            local: true,
            cost: "gratis",
            needs_gpu: false,
            ram: "~100MB",
            latency: "~0.5-1s",
            style_control: false,
            cloning: false,
            profile_tier: "low",
            status: "ACTIVO (baseline)",
        },
        EngineInfo {
            id: "piper-custom-voice",
            local: true,
            cost: "gratis (entrenar aparte)",
            needs_gpu: false,
            ram: "~100MB",
            latency: "~0.5-1s",
            style_control: false,
            cloning: true,
            profile_tier: "low",
            status: "F3: voz propia Nexum",
        },
        EngineInfo {
            id: "openvoice",
            local: true,
            cost: "gratis",
            needs_gpu: false,
            ram: "~2-3GB",
            latency: "2-6s CPU",
            style_control: true,
            cloning: true,
            profile_tier: "medium",
            status: "F3 experimental (medir en 8GB)",
        },
        EngineInfo {
            id: "xtts-v2",
            local: true,
            cost: "gratis (licencia CPML no comercial)",
            needs_gpu: false,
            ram: "~4-6GB",
            latency: "5-15s CPU",
            style_control: true,
            cloning: true,
            profile_tier: "medium/power",
            status: "F3: evaluar licencia+RAM",
        },
        EngineInfo {
            id: "elevenlabs",
            local: false,
            cost: "pago por uso",
            needs_gpu: false,
            ram: "-",
            latency: "~1s red",
            style_control: true,
            cloning: true,
            profile_tier: "cloud-opt-in",
            status: "OFF por defecto (jamás requisito)",
        },
        EngineInfo {
            id: "openai-realtime",
            local: false,
            cost: "pago",
            needs_gpu: false,
            ram: "-",
            latency: "~1s red",
            style_control: true,
            cloning: false,
            profile_tier: "cloud-opt-in",
            status: "OFF por defecto",
        },
        EngineInfo {
            id: "nvidia-riva",
            local: true,
            cost: "gratis (stack pesado)",
            needs_gpu: true,
            ram: "GB+ VRAM",
            latency: "<1s GPU",
            style_control: true,
            cloning: true,
            profile_tier: "power",
            status: "futuro perfil power",
        },
    ]
}

/// Voces Piper instaladas (dirs estándar + NEXUM_PIPER_VOICES_DIR).
pub fn list_voices() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("NEXUM_PIPER_VOICES_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(h) = dirs_next::home_dir() {
        dirs.push(h.join(".local/share/piper/voices"));
    }
    dirs.push(nexum_agent::config_home::nexum_home().join("voices"));
    let mut out: Vec<String> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "onnx")).then(|| {
                p.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Resuelve el .onnx del perfil (voice_id → fallback_profile → daniela → None).
pub fn resolve_voice_path(profile: &VoiceProfile) -> Option<PathBuf> {
    let fallback_vid = profile
        .fallback_profile
        .as_ref()
        .and_then(|fid| builtin_profiles().into_iter().find(|b| &b.id == fid))
        .map(|b| b.voice_id);
    let candidates = [
        Some(profile.voice_id.clone()),
        fallback_vid,
        Some("es_AR-daniela-high".to_string()),
    ];
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("NEXUM_PIPER_VOICES_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Some(h) = dirs_next::home_dir() {
        dirs.push(h.join(".local/share/piper/voices"));
    }
    dirs.push(nexum_agent::config_home::nexum_home().join("voices"));
    for vid in candidates.into_iter().flatten() {
        for d in &dirs {
            let p = d.join(format!("{vid}.onnx"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_builtins_y_daniela_solo_fallback() {
        let ps = builtin_profiles();
        assert_eq!(ps[0].id, "nexum_default", "la identidad es Nexum");
        assert!(ps.iter().any(|p| p.id == "piper_daniela_fallback"));
        assert_eq!(
            ps[0].fallback_profile.as_deref(),
            Some("piper_daniela_fallback"),
            "Daniela es fallback, no identidad"
        );
    }

    #[test]
    fn test_style_controller_cambia_perfil_no_codigo() {
        let _g = crate::hormiguero::bridge::test_env_lock();
        let p = VoiceStyleController::apply_style("serio").unwrap();
        assert_eq!(p.id, "nexum_serious");
        assert!(p.speed < 1.0, "serio = más pausado");
        let p2 = VoiceStyleController::apply_style("reset").unwrap();
        assert_eq!(p2.id, "nexum_default");
        assert!(VoiceStyleController::apply_style("alienígena").is_err());
    }

    #[test]
    fn test_speed_pitch_clamps_y_persistencia() {
        let _g = crate::hormiguero::bridge::test_env_lock();
        let p = VoiceStyleController::set_speed(9.0).unwrap();
        assert!(p.speed <= 2.0, "clamp");
        let p = VoiceStyleController::set_pitch(-99.0).unwrap();
        assert!(p.pitch >= -12.0);
        let _ = VoiceStyleController::apply_style("reset");
    }

    #[test]
    fn test_engines_catalog_declara_costos() {
        let cat = engines_catalog();
        let piper = cat.iter().find(|e| e.id == "piper").unwrap();
        assert!(piper.local && !piper.needs_gpu, "baseline CPU local");
        assert!(
            cat.iter().any(|e| e.profile_tier == "power" && e.needs_gpu),
            "GPU declarada solo en power"
        );
        assert!(
            cat.iter()
                .filter(|e| !e.local)
                .all(|e| e.profile_tier == "cloud-opt-in"),
            "cloud siempre opt-in"
        );
    }

    #[test]
    fn test_load_from_path_distingue_ausencia_de_perfil_corrupto() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("profile.json");
        assert!(matches!(load_from_path(&path), Ok(None)));
        std::fs::write(&path, "{perfil invalido").unwrap();
        assert!(matches!(
            load_from_path(&path),
            Err(ProfileLoadError::Corrupt(_))
        ));
    }

    #[test]
    fn test_save_to_path_es_atomico_y_restrictivo() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("voice/profile.json");
        let profile = resolve_local_default_profile("es_AR");
        save_to_path(&path, &profile).unwrap();
        assert_eq!(load_from_path(&path).unwrap(), Some(profile));
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .all(|entry| entry.unwrap().path() == path),
            "el temporal debe desaparecer tras el rename atomico"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn test_resolve_local_default_profile_usa_manifest_estatico_y_fallback_determinista() {
        assert_eq!(resolve_local_default_profile("es-AR").id, "nexum_default");
        assert_eq!(resolve_local_default_profile("es_AR").id, "nexum_default");
        assert_eq!(
            resolve_local_default_profile("fr-FR").id,
            resolve_local_default_profile("desconocido").id
        );
    }
}
