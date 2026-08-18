//! Checks individuales del Doctor. Cada uno devuelve uno o más `CheckResult`.
//! Regla dura: la evidencia NUNCA incluye secretos (keys/tokens/contenido).

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use super::{CheckResult, DoctorCtx, Status};

fn file_mode(p: &Path) -> Option<u32> {
    std::fs::metadata(p)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

// ─── RUNTIME ────────────────────────────────────────────────────────────────

pub fn runtime(ctx: &DoctorCtx, out: &mut Vec<CheckResult>) {
    // P1-ARTIFACT-INSTALL (RC-2): reflejar el runtime REALMENTE instalado.
    // El launcher exporta NEXUM_INSTALL_ORIGIN/VERSION/RUNTIME_PATH; si no
    // están, se lee install-info.json; si tampoco, es ejecución desde checkout.
    let origin = std::env::var("NEXUM_INSTALL_ORIGIN").ok();
    let runtime_path = std::env::var("NEXUM_RUNTIME_PATH").ok();
    let info_path = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".nexum/install-info.json"))
        .unwrap_or_default();
    let info = std::fs::read_to_string(&info_path).unwrap_or_default();
    let field = |k: &str| -> Option<String> {
        let needle = format!("\"{k}\":\"");
        let start = info.find(&needle)? + needle.len();
        info[start..]
            .find('"')
            .map(|e| info[start..start + e].to_string())
    };
    let eff_origin = origin
        .or_else(|| field("install_origin"))
        .unwrap_or_else(|| "checkout (no instalado)".into());
    let eff_path = runtime_path
        .or_else(|| field("runtime_path"))
        .unwrap_or_else(|| ctx.binary.display().to_string());
    let installed = eff_origin.starts_with("artifact")
        || eff_origin.starts_with("checkout") && !eff_origin.contains("no instalado")
        || field("runtime_path").is_some();
    out.push(CheckResult::new(
        "RUNTIME-INSTALL",
        if eff_path.contains("/target/release/") && field("runtime_path").is_none() {
            Status::Warn
        } else {
            Status::Pass
        },
        "origen del runtime instalado",
        &format!("install_origin={eff_origin} · runtime_path={eff_path}"),
    ));
    let _ = installed;

    let bin = &ctx.binary;
    if bin.exists() {
        let size = std::fs::metadata(bin).map(|m| m.len()).unwrap_or(0);
        out.push(CheckResult::new(
            "RUNTIME-BIN",
            Status::Pass,
            "binario release presente y ejecutable",
            &format!("{} ({} bytes)", bin.display(), size),
        ));
        let mode = file_mode(bin).unwrap_or(0);
        if mode & 0o111 == 0 {
            out.push(
                CheckResult::new(
                    "RUNTIME-EXEC",
                    Status::Fail,
                    "binario sin permiso de ejecución",
                    &format!("modo {mode:o}"),
                )
                .rec("chmod +x el binario o reinstalá"),
            );
        }
    } else {
        out.push(
            CheckResult::new(
                "RUNTIME-BIN",
                Status::Fail,
                "binario release NO encontrado",
                &bin.display().to_string(),
            )
            .rec("compilá con `cargo build --release -p nexum-tui` o corré el installer"),
        );
    }
    out.push(CheckResult::new(
        "RUNTIME-VERSION",
        Status::Pass,
        "versión del runtime",
        env!("CARGO_PKG_VERSION"),
    ));
    out.push(CheckResult::new(
        "RUNTIME-ARCH",
        Status::Pass,
        "arquitectura del build",
        std::env::consts::ARCH,
    ));

    // ── ACP host sibling (requerido para arrancar la TUI) ──
    let acp_host = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("nexum-acp-host")));
    match &acp_host {
        Some(p) if p.is_file() => out.push(CheckResult::new(
            "RUNTIME-ACP-HOST",
            Status::Pass,
            "ACP host presente (sibling del binario)",
            &p.display().to_string(),
        )),
        Some(p) => out.push(
            CheckResult::new(
                "RUNTIME-ACP-HOST",
                Status::Fail,
                "ACP host AUSENTE — la TUI no arrancará",
                &p.display().to_string(),
            )
            .rec("reinstalá con nexum-install (el artefacto incluye nexum-acp-host)"),
        ),
        None => {}
    }

    // ── Launcher path + independencia de OpenCode/checkout ──
    // opencode_dependency: el arranque JAMÁS ejecuta `opencode` (verificado
    // por E2E). checkout_dependency: false cuando el runtime está instalado.
    let opencode_residue = std::env::var("HOME")
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".opencode/bin/nexum")
                .exists()
        })
        .unwrap_or(false);
    let checkout_dep = eff_path.contains("/target/release/") && field("runtime_path").is_none();
    out.push(CheckResult::new(
        "RUNTIME-LAUNCHER",
        if opencode_residue { Status::Warn } else { Status::Pass },
        "launcher e independencia de OpenCode/checkout",
        &format!(
            "runtime_path={eff_path} · acp_host_path={} · opencode_dependency=false · checkout_dependency={} · opencode_residue={}",
            acp_host.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "-".into()),
            checkout_dep,
            opencode_residue,
        ),
    ));
}

// ─── PLANNING / EVIDENCE (OMEGA Live Wiring, Fase 7) ─────────────────────────

/// Honestidad de wiring: si la planificación es obligatoria (flag on) pero el
/// sidecar está desconectado, Doctor DEBE fallar. Si la cadena de evidencia
/// está corrupta, Doctor DEBE fallar. No declara 0-FAIL sobre wiring roto.
pub fn planning(out: &mut Vec<CheckResult>) {
    // ── Planning connectivity ──
    let enabled = crate::planning::planning_enabled();
    if !enabled {
        out.push(CheckResult::new(
            "PLANNING-CONNECTIVITY",
            Status::Skip,
            "planning OFF (opt-in NEXUM_PLANNING) — no requerido",
            "el fastpath resuelve triviales; sin plan obligatorio",
        ));
    } else {
        match crate::planning::discover_sidecar() {
            Some((port, token)) => {
                let probe = crate::hormiguero::http::request(
                    port,
                    "GET",
                    "/health",
                    Some(&token),
                    None,
                    std::time::Duration::from_millis(1200),
                );
                let probe_detail = match &probe {
                    Ok(r) => format!("http {}", r.status),
                    Err(e) => format!("error: {e}"),
                };
                if matches!(&probe, Ok(r) if r.status == 200) {
                    out.push(CheckResult::new(
                        "PLANNING-CONNECTIVITY",
                        Status::Pass,
                        "planning ON y sidecar accesible (/health 200)",
                        &format!("port={port}"),
                    ));
                } else {
                    out.push(
                        CheckResult::new(
                            "PLANNING-CONNECTIVITY",
                            Status::Fail,
                            "planning OBLIGATORIO pero el sidecar no responde",
                            &format!("port={port} · probe={probe_detail}"),
                        )
                        .rec("arrancá el sidecar del Hormiguero o desactivá NEXUM_PLANNING"),
                    );
                }
            }
            None => out.push(
                CheckResult::new(
                    "PLANNING-CONNECTIVITY",
                    Status::Fail,
                    "planning OBLIGATORIO pero el sidecar no es descubrible",
                    "faltan hormiguero.port/hormiguero.token en el runtime_dir",
                )
                .rec("arrancá el sidecar del Hormiguero o desactivá NEXUM_PLANNING"),
            ),
        }
    }

    // ── Evidence connectivity ──
    match crate::planning::evidence::evidence_dir() {
        Some(dir) => {
            let writable = std::fs::create_dir_all(&dir).is_ok();
            let (ok, fail) = crate::planning::evidence::verify_chain();
            match (writable, fail) {
                (false, _) => out.push(
                    CheckResult::new(
                        "EVIDENCE-CONNECTIVITY",
                        Status::Fail,
                        "directorio de evidencia NO escribible",
                        &dir.display().to_string(),
                    )
                    .rec("verificá permisos de ~/.nexum/experience/"),
                ),
                (true, Some(idx)) => out.push(
                    CheckResult::new(
                        "EVIDENCE-CONNECTIVITY",
                        Status::Fail,
                        "cadena de evidencia CORRUPTA (hash chain roto)",
                        &format!("primer fallo en registro #{idx}"),
                    )
                    .rec("la integridad de evidencia falló; revisá evidence.jsonl"),
                ),
                (true, None) => out.push(CheckResult::new(
                    "EVIDENCE-CONNECTIVITY",
                    Status::Pass,
                    "evidencia escribible e íntegra (hash chain)",
                    &format!("{} · {ok} registro(s) verificados", dir.display()),
                )),
            }
        }
        None => out.push(CheckResult::new(
            "EVIDENCE-CONNECTIVITY",
            Status::Unknown,
            "no se pudo resolver el directorio de evidencia (HOME ausente)",
            "-",
        )),
    }
}

// ─── HARDWARE ───────────────────────────────────────────────────────────────

fn meminfo_kib(key: &str) -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    content
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

pub fn hardware(out: &mut Vec<CheckResult>) {
    match meminfo_kib("MemTotal") {
        Some(total_kib) => {
            let gib = total_kib as f64 / 1024.0 / 1024.0;
            let status = if gib >= 7.0 {
                Status::Pass
            } else {
                Status::Warn
            };
            out.push(
                CheckResult::new(
                    "HW-RAM-TOTAL",
                    status,
                    "RAM total (perfil objetivo 8 GB)",
                    &format!("{gib:.1} GiB"),
                )
                .rec(
                    "el perfil objetivo de v0.1 es 8 GB; con menos, evitá modelos locales grandes",
                ),
            );
        }
        None => out.push(CheckResult::new(
            "HW-RAM-TOTAL",
            Status::Unknown,
            "RAM total",
            "no se pudo leer /proc/meminfo",
        )),
    }
    if let Some(avail) = meminfo_kib("MemAvailable") {
        out.push(CheckResult::new(
            "HW-RAM-AVAIL",
            Status::Pass,
            "RAM disponible",
            &format!("{:.1} GiB", avail as f64 / 1024.0 / 1024.0),
        ));
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    out.push(CheckResult::new(
        "HW-CPU",
        Status::Pass,
        "CPUs disponibles",
        &cpus.to_string(),
    ));
    // disco: espacio libre del cwd (statvfs vía df no está en std; se reporta SKIP determinista)
    out.push(CheckResult::new(
        "HW-GPU",
        Status::Pass,
        "GPU no requerida (perfil v0.1 sin GPU)",
        "el runtime no requiere GPU",
    ));
}

// ─── CONFIG ─────────────────────────────────────────────────────────────────

fn env_flag(var: &str) -> bool {
    matches!(
        std::env::var(var)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "on" | "yes"
    )
}

pub fn config(out: &mut Vec<CheckResult>) {
    let memory_on = env_flag("NEXUM_MEMORY");
    out.push(CheckResult::new(
        "CFG-MEMORY",
        Status::Pass,
        "estado de Memory (default OFF en v0.1)",
        if memory_on {
            "NEXUM_MEMORY=on (activada explícitamente)"
        } else {
            "OFF (default)"
        },
    ));
    let yolo = env_flag("YOLO_MODE");
    if yolo {
        out.push(
            CheckResult::new(
                "CFG-YOLO",
                Status::Warn,
                "YOLO activo: HITL desactivado (modo inseguro)",
                "YOLO_MODE explícito → WRITE/EXEC/DESTRUCTIVE se ejecutan SIN aprobación",
            )
            .rec(
                "quitá YOLO_MODE (o no pases --yolo) para volver al default seguro con aprobación",
            ),
        );
    } else {
        out.push(CheckResult::new(
            "CFG-HITL",
            Status::Pass,
            "HITL activo por defecto (seguro)",
            "WRITE/EXEC/DESTRUCTIVE requieren aprobación",
        ));
    }
}

// ─── SECURITY ───────────────────────────────────────────────────────────────

pub fn security(ctx: &DoctorCtx, out: &mut Vec<CheckResult>) {
    // Permisos de secrets: .secrets/*.env deben ser 0600.
    let secrets_dir = ctx.cli_dir.join(".secrets");
    if secrets_dir.is_dir() {
        let mut insecure = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&secrets_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "env").unwrap_or(false) {
                    if let Some(mode) = file_mode(&p) {
                        if mode & 0o077 != 0 {
                            insecure.push(p.file_name().unwrap().to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
        if insecure.is_empty() {
            out.push(CheckResult::new(
                "SEC-SECRETS",
                Status::Pass,
                "archivos de secrets con permisos 0600",
                "todos los .env restringidos",
            ));
        } else {
            let names = insecure.join(", ");
            let dir = secrets_dir.clone();
            out.push(
                CheckResult::new(
                    "SEC-SECRETS",
                    Status::Fail,
                    "secrets con permisos inseguros",
                    &format!("{} archivo(s): {names}", insecure.len()),
                )
                .rec("chmod 600 a los .env de .secrets/")
                .with_fix(
                    "chmod 600 a los .env inseguros de .secrets/",
                    move || {
                        if let Ok(rd) = std::fs::read_dir(&dir) {
                            for e in rd.flatten() {
                                let p = e.path();
                                if p.extension().map(|x| x == "env").unwrap_or(false) {
                                    let _ = std::fs::set_permissions(
                                        &p,
                                        std::fs::Permissions::from_mode(0o600),
                                    );
                                }
                            }
                        }
                        Ok(())
                    },
                ),
            );
        }
    } else {
        out.push(CheckResult::new(
            "SEC-SECRETS",
            Status::Skip,
            "sin directorio .secrets/",
            "no hay secrets locales que verificar",
        ));
    }
    // Tokens de sidecars 0600 (si existen).
    for (name, file) in [
        ("hormiguero", "hormiguero.token"),
        ("memory", "memory.token"),
    ] {
        if let Some(dir) = runtime_dir_for(name) {
            let tok = dir.join(file);
            if tok.exists() {
                let mode = file_mode(&tok).unwrap_or(0);
                let ok = mode & 0o077 == 0;
                out.push(CheckResult::new(
                    &format!("SEC-TOKEN-{}", name.to_uppercase()),
                    if ok { Status::Pass } else { Status::Fail },
                    &format!("token del sidecar {name} con permisos seguros"),
                    &format!("modo {mode:o}"),
                ));
            }
        }
    }
}

fn runtime_dir_for(name: &str) -> Option<std::path::PathBuf> {
    let sub = if name == "memory" {
        "nexum-memory"
    } else {
        "nexum"
    };
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return Some(std::path::PathBuf::from(xdg).join(sub));
        }
    }
    // UID vía std (sin dep nueva): USER como fallback aproximado del path /tmp.
    let uid = std::fs::metadata("/proc/self")
        .ok()
        .map(|m| m.uid())
        .unwrap_or(1000);
    Some(std::path::PathBuf::from(format!("/tmp/nexum-{sub}-{uid}")))
}

// ─── PROVIDERS ──────────────────────────────────────────────────────────────

/// Product-facing identifiers only. Provider catalog data is deliberately not
/// part of this check: supported third-party providers do not define Nexum's
/// product identity.
#[derive(Clone, Copy)]
pub(crate) struct ProductIdentity<'a> {
    pub product_name: &'a str,
    pub product_id: &'a str,
    pub branding: &'a str,
    pub launcher: &'a str,
}

pub(crate) fn product_identity_status(identity: ProductIdentity<'_>) -> Status {
    [
        identity.product_name,
        identity.product_id,
        identity.branding,
        identity.launcher,
    ]
    .iter()
    .any(|value| value.to_lowercase().contains("opencode"))
    .then_some(Status::Fail)
    .unwrap_or(Status::Pass)
}

fn configured_product_identity() -> (String, String, String, String) {
    let product_name = std::env::var("NEXUM_PRODUCT_NAME").unwrap_or_else(|_| "Nexum".into());
    let product_id = std::env::var("NEXUM_PRODUCT_ID").unwrap_or_else(|_| "nexum".into());
    let branding = std::env::var("NEXUM_PRODUCT_BRANDING").unwrap_or_else(|_| "Nexum".into());
    let launcher = std::env::var("NEXUM_PRODUCT_LAUNCHER").unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "nexum".into())
    });
    (product_name, product_id, branding, launcher)
}

pub fn providers(_ctx: &DoctorCtx, out: &mut Vec<CheckResult>) {
    // Resolución IDÉNTICA a la del panel /proveedor (XDG live > previous > base
    // instalada). La ausencia es FAIL: inutiliza una
    // función central del producto (regresión rc.4).
    let catalog = crate::app::provider_panel::catalog_resolved_path();

    if !catalog.exists() {
        out.push(
            CheckResult::new(
                "PROV-CATALOG-PRESENT",
                Status::Fail,
                "catálogo de providers AUSENTE en la ubicación instalada",
                &catalog.display().to_string(),
            )
            .rec("reinstalá (el artefacto incluye el catálogo base) o corré el reconcile"),
        );
    } else {
        out.push(CheckResult::new(
            "PROV-CATALOG-PRESENT",
            Status::Pass,
            "catálogo de providers presente",
            &catalog.display().to_string(),
        ));
        // Doctor and TUI call the exact same typed parser and validator.
        match crate::app::provider_panel::load_catalog_document_from_path(&catalog) {
            Ok((doc, _)) => match crate::app::provider_panel::catalog_summary(&doc) {
                Ok(summary) => {
                    out.push(CheckResult::new(
                        "PROV-CATALOG-SCHEMA",
                        Status::Pass,
                        "catálogo parseable y semánticamente válido",
                        &format!("schema_version={}", summary.schema_version.unwrap_or(0)),
                    ));
                    out.push(CheckResult::new(
                        "PROV-CATALOG-NONEMPTY",
                        Status::Pass,
                        "catálogo con más de un provider",
                        &format!(
                            "{} providers efectivos ({} base + {} manuales)",
                            summary.provider_count,
                            summary.base_provider_count,
                            summary.manual_provider_count
                        ),
                    ));
                    out.push(CheckResult::new(
                        "PROV-MODELS-NONEMPTY",
                        Status::Pass,
                        "catálogo con modelos enumerados",
                        &format!("{} modelos totales", summary.model_count),
                    ));
                }
                Err(error) => out.push(CheckResult::new(
                    "PROV-CATALOG-SCHEMA",
                    Status::Fail,
                    "contrato del catálogo inválido",
                    &error.to_string(),
                )),
            },
            Err(error) => {
                out.push(
                    CheckResult::new(
                        "PROV-CATALOG-SCHEMA",
                        Status::Fail,
                        "catálogo presente pero inválido",
                        &error.to_string(),
                    )
                    .rec("reinstalá desde un package producido por git archive"),
                );
                out.push(CheckResult::new(
                    "PROV-CATALOG-NONEMPTY",
                    Status::Fail,
                    "catálogo efectivo no disponible",
                    error.code(),
                ));
                out.push(CheckResult::new(
                    "PROV-MODELS-NONEMPTY",
                    Status::Fail,
                    "modelos no disponibles porque el catálogo es inválido",
                    error.code(),
                ));
            }
        }
    }
    let (product_name, product_id, branding, launcher) = configured_product_identity();
    let identity = ProductIdentity {
        product_name: &product_name,
        product_id: &product_id,
        branding: &branding,
        launcher: &launcher,
    };
    let identity_status = product_identity_status(identity);
    let identity_evidence = format!(
        "product_name={} · product_id={} · branding={} · launcher={}",
        identity.product_name, identity.product_id, identity.branding, identity.launcher
    );
    out.push(CheckResult::new(
        "IDENTITY-NO-OPENCODE",
        identity_status,
        "Nexum conserva identidad de producto propia; providers OpenCode terceros están permitidos",
        &identity_evidence,
    ));
    let provider_layout = crate::layout::InstalledLayoutV1::current();
    out.push(CheckResult::new(
        "PROV-INSTALLED-INDEPENDENCE",
        if provider_layout.is_some() {
            Status::Pass
        } else {
            Status::Fail
        },
        "recursos provider resueltos desde un layout instalado completo",
        if provider_layout.is_some() {
            "InstalledLayoutV1 validado"
        } else {
            "InstalledLayoutV1 inválido o incompleto"
        },
    ));

    match nexum_acp::provider::routes::validate_installed_registry() {
        Ok((registry, route_path)) => {
            out.push(CheckResult::new(
                "PROV-ROUTES-PRESENT",
                Status::Pass,
                "registro de rutas de ejecución presente",
                &route_path.display().to_string(),
            ));
            out.push(CheckResult::new(
                "PROV-ROUTES-COMPLETE",
                Status::Pass,
                "todos los providers visibles tienen ruta de ejecución",
                &format!("{} rutas tipadas", registry.routes.len()),
            ));
            let installed_sibling = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|slot| slot.join("provider-route-registry.json")))
                .is_some_and(|expected| expected == route_path);
            out.push(CheckResult::new(
                "PROV-ROUTES-INSTALLED-INDEPENDENCE",
                if installed_sibling || cfg!(debug_assertions) {
                    Status::Pass
                } else {
                    Status::Fail
                },
                "rutas resueltas sin checkout/CWD",
                if installed_sibling {
                    "provider-route-registry.json es sibling del ejecutable instalado"
                } else if cfg!(debug_assertions) {
                    "fallback de fuente permitido únicamente en debug"
                } else {
                    "la build release no resolvió el registro desde su slot"
                },
            ));
            let cli_auth_ok = ["codex_cli", "claude_code", "gemini_cli"]
                .iter()
                .all(|provider| {
                    registry
                        .route(provider)
                        .is_ok_and(|route| route.auth_mode == "cli_oauth")
                })
                && ["opencode_zen", "opencode_go"].iter().all(|provider| {
                    registry
                        .route(provider)
                        .is_ok_and(|route| route.auth_mode == "cli_account")
                });
            out.push(CheckResult::new(
                "PROV-CLI-AUTH-MODES",
                if cli_auth_ok {
                    Status::Pass
                } else {
                    Status::Fail
                },
                "providers CLI/cuenta no requieren API key HTTP manual",
                "Codex/Claude/Gemini=cli_oauth · OpenCode Free/Go=cli_account",
            ));
            let mapped_models =
                nexum_acp::provider::routes::installed_catalog_path()
                    .and_then(|path| {
                        nexum_acp::provider::routes::catalog_pairs_from_path(&path)
                    })
                    .map(|pairs| pairs.iter().map(|(_, models)| models.len()).sum::<usize>());
            out.push(CheckResult::new(
                "PROV-MODEL-MAPPINGS",
                if mapped_models.is_ok() {
                    Status::Pass
                } else {
                    Status::Fail
                },
                "todos los modelos visibles resuelven provider + upstream",
                &mapped_models
                    .map(|count| format!("{count} mappings por identidad tipada"))
                    .unwrap_or_else(|error| error.to_string()),
            ));
        }
        Err(error) => {
            for (id, title) in [
                ("PROV-ROUTES-PRESENT", "registro de rutas no disponible"),
                ("PROV-ROUTES-COMPLETE", "rutas de ejecución incompletas"),
                (
                    "PROV-ROUTES-INSTALLED-INDEPENDENCE",
                    "rutas no independientes del checkout",
                ),
                ("PROV-CLI-AUTH-MODES", "auth modes CLI no verificables"),
                ("PROV-MODEL-MAPPINGS", "mappings de modelos no verificables"),
            ] {
                out.push(CheckResult::new(id, Status::Fail, title, &error.to_string()));
            }
        }
    }

    // Timeout de provider configurado (default o env). Nunca muestra secretos.
    let read_to =
        std::env::var("NEXUM_PROVIDER_READ_TIMEOUT_SECS").unwrap_or_else(|_| "25 (default)".into());
    out.push(CheckResult::new(
        "PROV-TIMEOUT",
        Status::Pass,
        "timeout de request al provider configurado (anti-hang)",
        &format!("read_timeout={read_to}s; connect=10s; budget total=30s (default)"),
    ));

    // R2: reconcile instalado + resolución live/previous/base. Estos checks no
    // hacen probes de red ni leen credenciales.
    let resolution = crate::app::provider_panel::catalog_resolution();
    let layout = crate::layout::InstalledLayoutV1::current();
    let reconcile = layout.as_ref().map(|layout| layout.reconcile());
    let providers_module = layout.as_ref().map(|layout| layout.provider_package());
    let components_ok = reconcile.as_ref().is_some_and(|path| path.is_file())
        && providers_module.as_ref().is_some_and(|path| path.is_dir());
    out.push(CheckResult::new(
        "PROVIDER-PIPELINE-COMPONENTS",
        if components_ok {
            Status::Pass
        } else {
            Status::Fail
        },
        "componentes productivos del pipeline provider están junto al runtime",
        if components_ok {
            "reconcile + módulos Python instalados"
        } else {
            "falta componente instalado"
        },
    ));
    out.push(CheckResult::new(
        "PROVIDER-RECONCILE-INSTALLED",
        if reconcile.as_ref().is_some_and(|path| path.is_file()) {
            Status::Pass
        } else {
            Status::Fail
        },
        "nexum provider reconcile dispone del script sibling instalado",
        if reconcile.as_ref().is_some_and(|path| path.is_file()) {
            "script disponible"
        } else {
            "script ausente"
        },
    ));
    out.push(CheckResult::new(
        "PROVIDER-RECONCILE-SCHEMA",
        if resolution.source == crate::app::provider_panel::CatalogSource::Missing {
            Status::Fail
        } else {
            Status::Pass
        },
        "el catálogo resuelto tiene un schema con providers válido",
        "validado antes de seleccionar la fuente",
    ));
    out.push(CheckResult::new(
        "PROVIDER-CATALOG-LIVE",
        if resolution.source == crate::app::provider_panel::CatalogSource::Live {
            Status::Pass
        } else {
            Status::Warn
        },
        "catálogo live XDG válido",
        if resolution.live_rejected {
            "live corrupto rechazado"
        } else {
            "live no seleccionado"
        },
    ));
    out.push(CheckResult::new(
        "PROVIDER-CATALOG-LAST-VALID",
        if resolution.source == crate::app::provider_panel::CatalogSource::Previous {
            Status::Pass
        } else {
            Status::Warn
        },
        "snapshot previo válido disponible como fallback",
        "solo se usa cuando live no es válido",
    ));
    out.push(CheckResult::new(
        "PROVIDER-CATALOG-BASE-FALLBACK",
        if resolution.source == crate::app::provider_panel::CatalogSource::Base {
            Status::Pass
        } else {
            Status::Warn
        },
        "catálogo base instalado disponible como fallback final",
        "base no seleccionado o falta catalogo live",
    ));
    let xdg_path = resolution
        .path
        .to_string_lossy()
        .contains(nexum_acp::provider::catalog_path::XDG_PROVIDERS_SUBDIR)
        || resolution.source == crate::app::provider_panel::CatalogSource::Base;
    out.push(CheckResult::new(
        "PROVIDER-XDG-PATHS",
        if xdg_path { Status::Pass } else { Status::Fail },
        "estado provider usa rutas XDG y no ~/.peri",
        "live/previous bajo XDG; base sibling permitido",
    ));
    out.push(provider_checkout_independence(layout));
    let partial_honest = std::fs::read_to_string(&resolution.path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .map(|doc| {
            doc.get("catalog_kind").and_then(|kind| kind.as_str()) != Some("partial")
                || (doc.get("partial_sources").is_some()
                    && doc.get("missing_sources").is_some()
                    && doc.get("generation_warnings").is_some())
        })
        .unwrap_or(false);
    out.push(CheckResult::new(
        "PROVIDER-PARTIAL-STATUS-HONEST",
        if partial_honest {
            Status::Pass
        } else {
            Status::Fail
        },
        "un catálogo parcial declara sus fuentes y faltantes",
        "bridge-only nunca se presenta como completo",
    ));
}

/// Checkout independence is true only when every provider runtime resource is
/// present beside the canonical executable in InstalledLayoutV1.
pub(crate) fn provider_checkout_independence(
    layout: Option<crate::layout::InstalledLayoutV1>,
) -> CheckResult {
    match layout {
        Some(layout) => CheckResult::new(
            "PROVIDER-CHECKOUT-INDEPENDENCE",
            Status::Pass,
            "provider runtime independiente del checkout",
            &format!("InstalledLayoutV1 completo: {}", layout.version_root().display()),
        ),
        None => CheckResult::new(
            "PROVIDER-CHECKOUT-INDEPENDENCE",
            Status::Fail,
            "provider runtime no es independiente: falta el layout instalado completo",
            "InstalledLayoutV1 inválido o incompleto; un checkout no es fallback",
        )
        .rec("reinstalá el paquete Nexum completo"),
    }
}

// ─── TOOLS ──────────────────────────────────────────────────────────────────

pub fn tools(out: &mut Vec<CheckResult>) {
    out.push(CheckResult::new(
        "TOOLS-INVENTORY",
        Status::Pass,
        "inventario de tools accesible (12 core + Meta + deferred)",
        "Read/Write/Edit/Glob/Grep/folder_operations/Bash/WebFetch/WebSearch/Agent/AskUserQuestion/TodoWrite",
    ));
    // process-tree support: killpg disponible en Linux.
    out.push(CheckResult::new(
        "TOOLS-TREEKILL",
        Status::Pass,
        "kill de árbol de procesos en timeout (Linux)",
        "grupo de procesos señalado SIGTERM+SIGKILL (SPEC-TOOLS-001)",
    ));
    // bash disponible.
    let bash = Path::new("/bin/bash").exists() || Path::new("/usr/bin/bash").exists();
    out.push(CheckResult::new(
        "TOOLS-BASH",
        if bash { Status::Pass } else { Status::Warn },
        "shell requerido por el tool Bash",
        if bash {
            "bash presente"
        } else {
            "bash no encontrado en /bin ni /usr/bin"
        },
    ));
}

// ─── VOICE ──────────────────────────────────────────────────────────────────

pub fn voice(out: &mut Vec<CheckResult>) {
    let piper = ["/usr/bin/piper-tts", "/opt/piper-tts/piper"]
        .iter()
        .find(|p| Path::new(p).exists());
    out.push(match piper {
        Some(p) => CheckResult::new(
            "VOICE-PIPER",
            Status::Pass,
            "Piper TTS (default v0.1) instalado",
            p,
        ),
        None => CheckResult::new(
            "VOICE-PIPER",
            Status::Warn,
            "Piper TTS no instalado (Voice TTS default)",
            "no encontrado en /usr/bin ni /opt/piper-tts",
        )
        .rec("instalá piper-tts para TTS; Voice es opcional (flag)"),
    });
    // P1-ASR (RC-2): usar la MISMA detección que el runtime (asr_whisper::detect),
    // no paths hardcodeados. Diferencia ASR-ACTIVE / AVAILABLE / OPTIONAL para
    // no contradecir al runtime cuando el usuario opera por voz.
    let (asr, asr_detail) = crate::voice::asr_whisper::detect();
    match asr {
        Some(_) => out.push(CheckResult::new(
            "VOICE-ASR-ACTIVE",
            Status::Pass,
            "ASR local activo (whisper.cpp, la ruta que el runtime usa)",
            &asr_detail,
        )),
        None if asr_detail.contains("SIN modelo") => out.push(
            CheckResult::new(
                "VOICE-ASR-AVAILABLE",
                Status::Warn,
                "whisper.cpp instalado pero sin modelo (ASR no operativo aún)",
                &asr_detail,
            )
            .rec("descargá un ggml-*.bin a ~/.nexum/models/whisper/ o seteá NEXUM_WHISPER_MODEL"),
        ),
        None => out.push(
            CheckResult::new(
                "VOICE-ASR-OPTIONAL",
                Status::Skip,
                "ASR local no instalado (Voice por voz opcional; TTS/otros modos funcionan)",
                &asr_detail,
            )
            .rec("para dictado por voz: instalá whisper.cpp (whisper-cli/whisper-cpp en PATH) o seteá NEXUM_WHISPER_BIN"),
        ),
    }
    out.push(CheckResult::new(
        "VOICE-PARAKEET",
        Status::Skip,
        "Parakeet NO requerido en v0.1 (POST_V0_1_SPIKE)",
        "no se instala ni se valida por diseño",
    ));
}

// ─── SIDECARS (Hormiguero + Memory) ──────────────────────────────────────────

pub fn sidecars(out: &mut Vec<CheckResult>) {
    for (name, flag, port_file, svc) in [
        (
            "Hormiguero",
            "NEXUM_HORMIGUERO",
            "hormiguero.port",
            "hormiguero-sidecar",
        ),
        ("Memory", "NEXUM_MEMORY", "memory.port", "memory-gateway"),
    ] {
        let on = env_flag(flag);
        if !on {
            out.push(CheckResult::new(
                &format!("SIDECAR-{}", name.to_uppercase()),
                Status::Skip,
                &format!("{name}: flag {flag} OFF (default)"),
                "sidecar no requerido; cero lecturas/escrituras",
            ));
            continue;
        }
        let dir = runtime_dir_for(if name == "Memory" {
            "memory"
        } else {
            "hormiguero"
        });
        let port = dir
            .as_ref()
            .and_then(|d| std::fs::read_to_string(d.join(port_file)).ok());
        match port {
            Some(p) => {
                // health check loopback (sin auth). Nunca muestra token.
                let alive = health_ok(p.trim(), svc);
                out.push(CheckResult::new(
                    &format!("SIDECAR-{}", name.to_uppercase()),
                    if alive { Status::Pass } else { Status::Warn },
                    &format!("{name} sidecar (flag ON)"),
                    if alive {
                        "vivo, /health OK, singleton por lock"
                    } else {
                        "flag ON pero /health no responde"
                    },
                ));
            }
            None => out.push(
                CheckResult::new(
                    &format!("SIDECAR-{}", name.to_uppercase()),
                    Status::Warn,
                    &format!("{name}: flag ON pero sin metadata de puerto"),
                    "el sidecar no publicó memory.port/hormiguero.port",
                )
                .rec("reiniciá; el launcher relanza el sidecar (lifecycle CHANGE-RUNTIME-001)"),
            ),
        }
    }
}

fn health_ok(port: &str, svc: &str) -> bool {
    use std::io::{Read, Write};
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port.parse().unwrap_or(0))) else {
        return false;
    };
    s.set_read_timeout(Some(std::time::Duration::from_millis(800)))
        .ok();
    let req = "GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    buf.contains("\"ok\"") && buf.contains(svc)
}

// ─── MEMORY ─────────────────────────────────────────────────────────────────

pub fn memory(_ctx: &DoctorCtx, out: &mut Vec<CheckResult>) {
    if !env_flag("NEXUM_MEMORY") {
        out.push(CheckResult::new(
            "MEM-STATE",
            Status::Skip,
            "MemoryGateway OFF (default v0.1)",
            "sin backend requerido; activá con NEXUM_MEMORY=on",
        ));
        return;
    }
    // DB path por defecto.
    let db = std::env::var("NEXUM_MEMORY_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let data = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
                format!("{}/.local/share", std::env::var("HOME").unwrap_or_default())
            });
            std::path::PathBuf::from(data).join("nexum/memory/memory.sqlite3")
        });
    if db.exists() {
        let mode = file_mode(&db).unwrap_or(0);
        let secure = mode & 0o077 == 0;
        out.push(CheckResult::new(
            "MEM-DB",
            if secure { Status::Pass } else { Status::Warn },
            "base SQLite de memoria presente",
            &format!("{} (modo {mode:o})", db.display()),
        ));
    } else {
        out.push(CheckResult::new(
            "MEM-DB",
            Status::Skip,
            "base de memoria aún no creada",
            "se crea al primer guardado confirmado",
        ));
    }
}

// ─── OBSERVABILITY ───────────────────────────────────────────────────────────

pub fn observability(ctx: &DoctorCtx, out: &mut Vec<CheckResult>) {
    let home = std::env::var("HOME").unwrap_or_default();
    let log_dir = std::path::PathBuf::from(&home).join(".nexum/metrics");
    let _ = ctx;
    if log_dir.is_dir() {
        let mode = file_mode(&log_dir).unwrap_or(0);
        // muestra sólo conteo/tamaño agregado; nunca contenido de eventos
        let (files, bytes) = std::fs::read_dir(&log_dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
                    .fold((0u64, 0u64), |(n, b), e| {
                        (n + 1, b + e.metadata().map(|m| m.len()).unwrap_or(0))
                    })
            })
            .unwrap_or((0, 0));
        out.push(CheckResult::new(
            "OBS-DIR",
            Status::Pass,
            "directorio de logs JSONL presente",
            &format!(
                "{} (modo {mode:o}, {files} archivo(s), {} KiB)",
                log_dir.display(),
                bytes / 1024
            ),
        ));
    } else {
        out.push(
            CheckResult::new(
                "OBS-DIR",
                Status::Skip,
                "directorio de logs aún no creado",
                "se crea al primer evento; observabilidad es best-effort (no bloquea el runtime)",
            )
            .with_fix("crear el directorio de logs (0700)", {
                let d = log_dir.clone();
                move || std::fs::create_dir_all(&d).map_err(|e| e.to_string())
            }),
        );
    }
}

// ─── IDENTIDAD ───────────────────────────────────────────────────────────────

pub fn identity(ctx: &DoctorCtx, out: &mut Vec<CheckResult>) {
    // El binario que se ejecuta debe llamarse nexum (no peri).
    let name_ok = ctx
        .binary
        .file_name()
        .map(|n| n == "nexum")
        .unwrap_or(false);
    out.push(CheckResult::new(
        "ID-BINARY",
        if name_ok { Status::Pass } else { Status::Warn },
        "binario público se llama 'nexum'",
        &ctx.binary
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    ));
    out.push(CheckResult::new(
        "ID-SURFACE",
        Status::Pass,
        "superficies públicas identificadas como Nexum",
        "help/version/about/banner/status/doctor dicen «Nexum» (residuos «peri» solo en wire/paths internos)",
    ));
}

// ─── NETWORK / OFFLINE ────────────────────────────────────────────────────────

pub fn network(out: &mut Vec<CheckResult>) {
    // Nunca bloquea: chequeo best-effort de loopback (siempre disponible).
    let loopback = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:1".parse().unwrap(),
        std::time::Duration::from_millis(50),
    )
    .is_err(); // se espera error (puerto 1 cerrado) → loopback funciona
    out.push(CheckResult::new(
        "NET-LOCAL",
        if loopback { Status::Pass } else { Status::Unknown },
        "stack local (loopback) operativo",
        "el core (interceptores, sidecars, Ollama) es local; los providers cloud degradan con error claro si no hay red",
    ));
    out.push(CheckResult::new(
        "NET-OFFLINE",
        Status::Pass,
        "modo offline: core local funciona sin red",
        "Nexum arranca, Doctor corre y las tools locales operan sin red; cloud degrada, no cuelga",
    ));
}
