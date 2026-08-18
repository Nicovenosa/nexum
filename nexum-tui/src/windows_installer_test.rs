use std::process::Command;

const INSTALLERS: &[(&str, &str)] = &[
    ("scripts/install.ps1", include_str!("../../scripts/install.ps1")),
    ("agm/install.ps1", include_str!("../../agm/install.ps1")),
];

const REAL_INSTALLER: (&str, &str) =
    ("scripts/nexum-install.ps1", include_str!("../../scripts/nexum-install.ps1"));
const REAL_UNINSTALLER: (&str, &str) =
    ("scripts/nexum-uninstall.ps1", include_str!("../../scripts/nexum-uninstall.ps1"));

#[test]
fn retired_windows_installers_are_local_exit_69_stubs() {
    for (path, script) in INSTALLERS {
        let normalized = script.to_ascii_lowercase();
        assert!(normalized.contains("exit 69"), "{path} must exit 69");
        for forbidden in [
            "konghayao",
            "peri",
            "invoke-webrequest",
            "invoke-restmethod",
            "http://",
            "https://",
            "expand-archive",
            "start-process",
        ] {
            assert!(
                !normalized.contains(forbidden),
                "{path} contains forbidden installer behavior: {forbidden}"
            );
        }
    }
}

#[test]
fn retired_windows_installers_exit_69_when_pwsh_is_available() {
    if Command::new("pwsh").arg("-Version").output().is_err() {
        return;
    }
    for (path, _) in INSTALLERS {
        let status = Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-File", path])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(69), "{path} must reject execution");
    }
}

#[test]
fn real_installer_implements_installed_layout_v1() {
    let (path, script) = REAL_INSTALLER;
    let normalized = script.to_ascii_lowercase();
    for required in [
        "installedlayoutv1",
        "lib\\nexum",
        "current",
        "manifest.json",
        "slot",
        "nexum-acp-host",
        "provider-catalog-output.json",
        "provider-route-registry.json",
        "nexum.cmd",
        "addtopath",
        "junction",
        "-artifact",
    ] {
        assert!(normalized.contains(required), "{path} missing installer behavior: {required}");
    }
    for forbidden in ["konghayao", "peri"] {
        assert!(!normalized.contains(forbidden), "{path} contains inherited brand: {forbidden}");
    }
}

#[test]
fn real_uninstaller_removes_layout_and_path() {
    let (path, script) = REAL_UNINSTALLER;
    let normalized = script.to_ascii_lowercase();
    for required in [
        "lib\\nexum",
        "remove-item",
        "$command.cmd",
        "reparsepoint",
        "hkc",
        "keeppath",
    ] {
        assert!(normalized.contains(required), "{path} missing uninstaller behavior: {required}");
    }
}

#[test]
#[allow(clippy::disallowed_methods)] // test-only: CI scratch file, sandbox not needed
fn real_installer_parses_when_pwsh_is_available() {
    if Command::new("pwsh").arg("-Version").output().is_err() {
        return;
    }
    for (path, _) in [REAL_INSTALLER, REAL_UNINSTALLER] {
        let errors_file = std::env::temp_dir().join(format!(
            "nexum-ps-parse-errors-{}.txt",
            std::process::id()
        ));
        let script = format!(
            "$errors=@(); [void][System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$null, [ref]$errors); if($errors.Count -gt 0){{ $errors | Out-String | Set-Content '{}'; exit 1 }}",
            path,
            errors_file.display()
        );
        let status = Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "{path} failed PowerShell parse (see {})",
            errors_file.display()
        );
    }
}
