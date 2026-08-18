use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_init_creates_agm_json() {
    let tmp = TempDir::new().unwrap();
    let output = Command::new("cargo")
        .args(["run", "-p", "agm", "--", "init", "-C"])
        .arg(tmp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "init failed: {:?}", output);
    let manifest = tmp.path().join("agm.json");
    assert!(manifest.exists(), "agm.json not created");
}

#[test]
fn test_install_without_manifest_fails() {
    let tmp = TempDir::new().unwrap();
    let output = Command::new("cargo")
        .args([
            "run", "-p", "agm", "--", "install", "--tool", "claude", "-C",
        ])
        .arg(tmp.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "install should fail without agm.json"
    );
}

#[test]
fn test_init_then_list() {
    let tmp = TempDir::new().unwrap();

    let output = Command::new("cargo")
        .args(["run", "-p", "agm", "--", "init", "-C"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    let output = Command::new("cargo")
        .args(["run", "-p", "agm", "--", "list", "-C"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn test_help_output() {
    let output = Command::new("cargo")
        .args(["run", "-p", "agm", "--", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("init"), "help should mention init");
    assert!(stdout.contains("install"), "help should mention install");
    assert!(
        stdout.contains("uninstall"),
        "help should mention uninstall"
    );
    assert!(stdout.contains("list"), "help should mention list");
    assert!(stdout.contains("update"), "help should mention update");
    assert!(stdout.contains("publish"), "help should mention publish");
    assert!(stdout.contains("gc"), "help should mention gc");
}

fn setup_git_repo_with_skills(tmp: &TempDir) -> String {
    let repo = tmp.path().join("owner/repo");
    std::fs::create_dir_all(repo.join("skills/grill-me")).unwrap();
    std::fs::create_dir_all(repo.join("skills/interview")).unwrap();
    std::fs::create_dir_all(repo.join("skills/skill-test")).unwrap();
    std::fs::write(repo.join("skills/grill-me/SKILL.md"), "# grill-me").unwrap();
    std::fs::write(repo.join("skills/interview/SKILL.md"), "# interview").unwrap();
    std::fs::write(repo.join("skills/skill-test/SKILL.md"), "# skill-test").unwrap();

    let init = Command::new("git")
        .args(["init"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(init.status.success());

    let config = Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(config.status.success());

    let config = Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(config.status.success());

    let add = Command::new("git")
        .args(["add", "."])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(add.status.success());

    let commit = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(commit.status.success());

    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&head.stdout).trim().to_string()
}

#[test]
fn test_install_git_with_pick_omit() {
    let tmp = TempDir::new().unwrap();
    let commit = setup_git_repo_with_skills(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir(&project).unwrap();

    let agm_json = format!(
        r#"{{
            "name": "project",
            "targets": ["claude"],
            "skills": {{
                "@git/owner/repo": {{
                    "version": "{}",
                    "pick": ["grill-*", "interview"],
                    "omit": ["**/*-test"]
                }}
            }}
        }}"#,
        commit
    );
    std::fs::write(project.join("agm.json"), agm_json).unwrap();

    let output = Command::new("cargo")
        .args([
            "run", "-p", "agm", "--", "install", "--tool", "claude", "--git",
        ])
        .arg(tmp.path().join("owner/repo").to_str().unwrap())
        .arg("-C")
        .arg(&project)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let skills_dir = project.join(".claude/skills");
    assert!(
        skills_dir.join("grill-me").exists() || skills_dir.join("grill-me").read_link().is_ok()
    );
    assert!(
        skills_dir.join("interview").exists() || skills_dir.join("interview").read_link().is_ok()
    );
    assert!(!skills_dir.join("skill-test").exists());
}

#[test]
fn test_uninstall_git_with_pick_omit() {
    let tmp = TempDir::new().unwrap();
    let commit = setup_git_repo_with_skills(&tmp);
    let project = tmp.path().join("project");
    std::fs::create_dir(&project).unwrap();

    let agm_json = format!(
        r#"{{
            "name": "project",
            "targets": ["claude"],
            "skills": {{
                "@git/owner/repo": {{
                    "version": "{}",
                    "pick": ["grill-*", "interview"],
                    "omit": ["**/*-test"]
                }}
            }}
        }}"#,
        commit
    );
    std::fs::write(project.join("agm.json"), agm_json).unwrap();

    let output = Command::new("cargo")
        .args([
            "run", "-p", "agm", "--", "install", "--tool", "claude", "--git",
        ])
        .arg(tmp.path().join("owner/repo").to_str().unwrap())
        .arg("-C")
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new("cargo")
        .args([
            "run",
            "-p",
            "agm",
            "--",
            "uninstall",
            "@git/owner/repo",
            "--tool",
            "claude",
        ])
        .arg("-C")
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "uninstall failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let skills_dir = project.join(".claude/skills");
    assert!(!skills_dir.join("grill-me").exists());
    assert!(!skills_dir.join("interview").exists());
    assert!(!skills_dir.join("skill-test").exists());
    assert!(!skills_dir.join("@git/owner/repo").exists());
}
