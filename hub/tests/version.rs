//! `hub --version` names the commit the binary was built from, so a hub
//! serving an old build can be told from one serving the checkout.

use std::process::Command;

#[test]
fn the_version_names_the_commit_it_was_built_from() {
    let head = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let head = String::from_utf8(head.stdout).unwrap();
    let head = head.trim();
    assert_eq!(head.len(), 7, "{head:?}");

    let out = Command::new(env!("CARGO_BIN_EXE_hub"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("hub {} {head}\n", env!("CARGO_PKG_VERSION"))
    );
}
