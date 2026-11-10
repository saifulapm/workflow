//! The hub starts a project and resumes a session with the user manager's
//! PATH. Its own test binary: it sets the process's PATH, which no other
//! test may see.

use std::os::unix::fs::PermissionsExt;

use hub::runs::{resume, start_project};

/// A started project and a resumed session both run with the user
/// manager's PATH, not the hub's own short one: the command they start
/// needs mise's shims.
#[test]
fn start_and_resume_run_with_the_user_managers_path() {
    let dir = std::env::temp_dir().join(format!("hub-runs-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = |name: &str, body: &str| {
        let file = dir.join(name);
        std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        file
    };
    script("systemctl", "echo PATH=/fake/shims:/usr/bin");
    let amx = script("amx", r#"echo '[{"id":"s1","state":"stopped","ended":1}]'"#);
    let run = script(
        "systemd-run",
        &format!(r#"echo "$*|$PATH" >> {}/ran"#, dir.display()),
    );
    let own = std::env::var("PATH").unwrap_or_default();
    // Safety: this binary's only test, so nothing else reads the
    // environment meanwhile.
    unsafe {
        std::env::set_var("PATH", format!("{}:{own}", dir.display()));
        std::env::set_var("WORKFLOW_AMX", &amx);
        std::env::set_var("WORKFLOW_SYSTEMD_RUN", &run);
    }
    assert!(start_project("gamma"));
    assert!(resume("gamma", "s1", "go on"));
    let ran = std::fs::read_to_string(dir.join("ran")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let lines: Vec<&str> = ran.lines().collect();
    assert_eq!(lines.len(), 2, "{ran}");
    assert_eq!(
        lines[0],
        "--user --scope --quiet workflow go gamma|/fake/shims:/usr/bin"
    );
    assert_eq!(
        lines[1],
        format!(
            "--user --scope --quiet {} resume s1 go on|/fake/shims:/usr/bin",
            amx.display()
        )
    );
}
