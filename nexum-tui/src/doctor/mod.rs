//! `nexum doctor` — diagnóstico productivo read-only (SPEC-DOCTOR-001, RID-1).
//!
//! Determinístico, sin red obligatoria, sin crash ante dependencias
//! faltantes, y **sin exponer secretos** (nunca API keys, tokens, contenido
//! de memoria, prompts, transcripciones ni archivos privados). Por defecto es
//! read-only; `--fix` solo aplica reparaciones seguras y explícitas con
//! confirmación.

use std::fmt::Write as _;
use std::path::PathBuf;

mod checks;
pub mod integrity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
    Unknown,
}

impl Status {
    fn glyph(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Warn => "WARN",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
            Status::Unknown => "UNKN",
        }
    }
}

/// Un resultado de check. `evidence` DEBE ser segura (sin secretos).
pub struct CheckResult {
    pub id: String,
    pub status: Status,
    pub description: String,
    pub evidence: String,
    pub recommendation: Option<String>,
    /// Reparación segura disponible (solo con --fix + confirmación).
    pub fixable: Option<Fix>,
}

/// Reparación segura y explícita (permisos, metadata stale, dirs faltantes).
pub struct Fix {
    pub describe: String,
    pub apply: Box<dyn Fn() -> Result<(), String>>,
}

impl CheckResult {
    pub fn new(id: &str, status: Status, description: &str, evidence: &str) -> Self {
        Self {
            id: id.to_string(),
            status,
            description: description.to_string(),
            evidence: evidence.to_string(),
            recommendation: None,
            fixable: None,
        }
    }
    pub fn rec(mut self, r: &str) -> Self {
        self.recommendation = Some(r.to_string());
        self
    }
    pub fn with_fix(mut self, describe: &str, apply: impl Fn() -> Result<(), String> + 'static) -> Self {
        self.fixable = Some(Fix {
            describe: describe.to_string(),
            apply: Box::new(apply),
        });
        self
    }
}

/// Contexto compartido de los checks (rutas, sin secretos).
pub struct DoctorCtx {
    pub cli_dir: PathBuf,
    pub binary: PathBuf,
    pub cache_home: PathBuf,
    pub config_home: PathBuf,
    pub data_home: PathBuf,
}

impl DoctorCtx {
    fn detect() -> Self {
        let cli_dir = std::env::var("NEXUM_CLI_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.ancestors().nth(3).map(|a| a.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        let xdg = |var: &str, def: &str| {
            std::env::var(var)
                .map(PathBuf::from)
                .unwrap_or_else(|_| cli_dir.join(def))
        };
        Self {
            binary: std::env::current_exe().unwrap_or_else(|_| cli_dir.join("nexum-runtime/target/release/nexum")),
            cache_home: xdg("XDG_CACHE_HOME", ".cache"),
            config_home: xdg("XDG_CONFIG_HOME", ".config"),
            data_home: xdg("XDG_DATA_HOME", ".local-share"),
            cli_dir,
        }
    }
}

/// Entry point del subcomando. `fix` habilita reparaciones seguras (con
/// confirmación interactiva salvo `assume_yes`). Devuelve exit code:
/// 0 si no hay FAIL, 1 si hay al menos un FAIL.
pub fn run(fix: bool, assume_yes: bool) -> i32 {
    let ctx = DoctorCtx::detect();
    let mut results: Vec<CheckResult> = Vec::new();
    checks::runtime(&ctx, &mut results);
    checks::planning(&mut results);
    checks::hardware(&mut results);
    checks::config(&mut results);
    checks::security(&ctx, &mut results);
    checks::providers(&ctx, &mut results);
    checks::tools(&mut results);
    checks::voice(&mut results);
    checks::sidecars(&mut results);
    checks::memory(&ctx, &mut results);
    checks::observability(&ctx, &mut results);
    checks::identity(&ctx, &mut results);
    checks::network(&mut results);
    // Gates de empaquetado: Doctor daba PASS sobre instalaciones que
    // nexum-verify-parity y nexum-registry-gate rechazaban.
    integrity::integrity(&mut results);

    let mut out = String::new();
    let (mut pass, mut warn, mut fail, mut skip, mut unknown) = (0, 0, 0, 0, 0);
    let mut by_cat = String::new();
    for r in &results {
        match r.status {
            Status::Pass => pass += 1,
            Status::Warn => warn += 1,
            Status::Fail => fail += 1,
            Status::Skip => skip += 1,
            Status::Unknown => unknown += 1,
        }
        let _ = writeln!(by_cat, "  [{}] {} — {}", r.status.glyph(), r.id, r.description);
        if !r.evidence.is_empty() {
            let _ = writeln!(by_cat, "         {}", r.evidence);
        }
        if matches!(r.status, Status::Warn | Status::Fail | Status::Unknown) {
            if let Some(rec) = &r.recommendation {
                let _ = writeln!(by_cat, "         → {rec}");
            }
        }
    }
    let _ = writeln!(out, "nexum doctor — diagnóstico ({} checks)\n", results.len());
    out.push_str(&by_cat);
    let _ = writeln!(
        out,
        "\nResumen: {pass} PASS · {warn} WARN · {fail} FAIL · {skip} SKIP · {unknown} UNKNOWN"
    );
    print!("{out}");

    if fix {
        apply_fixes(&results, assume_yes);
    } else if results.iter().any(|r| r.fixable.is_some()) {
        println!("\nHay reparaciones seguras disponibles: corré `nexum doctor --fix`.");
    }

    if fail > 0 {
        1
    } else {
        0
    }
}

fn apply_fixes(results: &[CheckResult], assume_yes: bool) {
    use std::io::Write as _;
    for r in results {
        if let Some(fix) = &r.fixable {
            println!("\n[{}] {}", r.id, fix.describe);
            let go = if assume_yes {
                true
            } else {
                print!("  ¿aplicar esta reparación? [y/N] ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                matches!(line.trim(), "y" | "Y" | "s" | "S")
            };
            if go {
                match (fix.apply)() {
                    Ok(()) => println!("  ✓ reparado"),
                    Err(e) => println!("  ✗ falló: {e}"),
                }
            } else {
                println!("  omitido");
            }
        }
    }
}

#[cfg(test)]
#[path = "doctor_test.rs"]
mod doctor_test;
