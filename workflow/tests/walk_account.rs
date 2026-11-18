//! `workflow walk-account` through the real binary, against a stand-in
//! playwright-cli on PATH that logs each call and answers `eval` with the
//! page text the test gives it.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Output;

use scratch::Scratch;

/// A PATH whose playwright-cli logs its arguments and the token it got to
/// `calls`, fails the verb named in `fail`, and prints `page` for eval.
fn world(tag: &str, page: &str, fail: &str) -> Scratch {
    let w = Scratch::new(tag);
    w.put("page", page);
    w.script(
        "bin/playwright-cli",
        &format!(
            "#!/bin/sh\necho \"$* token=$PLAYWRIGHT_MCP_EXTENSION_TOKEN\" >> {calls}\n\
             [ \"$2\" = '{fail}' ] && {{ echo \"$2 broke\" >&2; exit 1; }}\n\
             [ \"$2\" = eval ] && cat {page}\nexit 0\n",
            calls = w.at("calls").display(),
            page = w.at("page").display(),
        ),
    );
    w.script("bin/playwright-extension-token", "#!/bin/sh\necho tok-1\n");
    w
}

fn walk_account(w: &Scratch) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("walk-account")
        .env("PATH", format!("{}:/usr/bin:/bin", w.at("bin").display()))
        .env_remove("PLAYWRIGHT_MCP_EXTENSION_TOKEN")
        .output()
        .unwrap()
}

fn calls(w: &Scratch) -> String {
    std::fs::read_to_string(w.at("calls")).unwrap_or_default()
}

#[test]
fn the_main_account_names_its_ultra_plan() {
    let w = world(
        "walk-account-main",
        "New session\nSaiful Islam\nUltra\n",
        "none",
    );
    let out = walk_account(&w);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "claude.ai account in Chromium: main\n"
    );
    assert_eq!(
        calls(&w),
        "-s=walk-account attach --extension=chromium token=tok-1\n\
         -s=walk-account tab-new https://claude.ai/code token=tok-1\n\
         -s=walk-account eval () => document.body.innerText token=tok-1\n\
         -s=walk-account tab-close token=tok-1\n\
         -s=walk-account detach token=tok-1\n"
    );
}

#[test]
fn a_page_with_no_ultra_plan_is_not_main() {
    let w = world(
        "walk-account-work",
        "New session\nSaiful Islam\nMax\n",
        "none",
    );
    let out = walk_account(&w);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("claude.ai account in Chromium: not main"),
        "{out:?}"
    );
}

#[test]
fn a_tab_that_did_not_open_is_not_closed() {
    let w = world("walk-account-notab", "Ultra", "tab-new");
    let out = walk_account(&w);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).is_empty());
    let calls = calls(&w);
    assert!(!calls.contains("tab-close"), "{calls}");
    assert!(
        calls.ends_with("-s=walk-account detach token=tok-1\n"),
        "{calls}"
    );
}

#[test]
fn a_failed_attach_stops_before_any_tab() {
    let w = world("walk-account-noattach", "Ultra", "attach");
    let out = walk_account(&w);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("attach broke"));
    assert_eq!(calls(&w).lines().count(), 1);
}
