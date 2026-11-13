//! The fake playwright-cli must print what the real one printed. Each case
//! drives the fake into the state that produces a transcript recorded from
//! the real CLI (tests/fixtures/playwright-cli) and compares stdout, stderr
//! and the exit code with it, each on its own.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::TempDir;

fn fixture(case: &str, ext: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/playwright-cli")
        .join(format!("{case}.{ext}"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Runs the fake over a scratch browser holding `tabs` (the strip file) and
/// `sessions` (the attached session names), and checks its three outputs
/// against the transcript recorded as `case`.
fn replays(case: &str, tabs: &str, sessions: &str, args: &[&str]) {
    let dir = TempDir::new("fake-cli");
    let state = dir.join("browser");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("tabs"), tabs).unwrap();
    std::fs::write(state.join("sessions"), sessions).unwrap();
    std::fs::write(state.join("next"), "2\n").unwrap();
    let out = Command::new("sh")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fake_tabs_cli.sh"))
        .args(args)
        .env("S", &state)
        .output()
        .unwrap();
    let code = String::from_utf8(fixture(case, "code")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&fixture(case, "out")),
        "{case}: stdout"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&fixture(case, "err")),
        "{case}: stderr"
    );
    assert_eq!(
        out.status.code(),
        Some(code.trim().parse().unwrap()),
        "{case}: exit"
    );
}

#[test]
fn run_code_result() {
    replays(
        "result",
        "",
        "m2b\n",
        &["-s=m2b", "run-code", "async page => { return -2 }"],
    );
}

#[test]
fn closing_the_last_tab_ends_the_session() {
    replays(
        "sessclosed",
        "2\tm2b\t1\tabout:blank\t0\n",
        "m2b\n",
        &["-s=m2b", "run-code", "async page => { page.close() }"],
    );
}

#[test]
fn tab_new_with_an_unresolvable_host() {
    replays(
        "tabnew_bad",
        "",
        "m2b\n",
        &["-s=m2b", "tab-new", "https://nope.invalid/"],
    );
}

#[test]
fn detach_of_a_session_that_is_not_attached() {
    replays("detach_none", "", "", &["-s=m2bnone", "detach"]);
}

#[test]
fn list_with_no_session() {
    replays("list", "", "", &["list"]);
}

#[test]
fn list_with_one_session() {
    replays("list1", "", "m2blist\n", &["list"]);
}
