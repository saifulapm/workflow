//! `workflow mask` through the real binary: stdin in, stdout out.

use std::io::Write;
use std::process::Stdio;

fn mask(input: &str) -> (Option<i32>, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("mask")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn mask_hides_the_secret_shapes() {
    let (code, out) = mask("a shpss_abc123 b cfut_X-y_9 tskey-auth-k1 shpat_Z\n");
    assert_eq!(code, Some(0));
    assert_eq!(out, "a shpss_*** b cfut_*** tskey-*** shpat_***\n");
}

#[test]
fn mask_leaves_other_text_alone() {
    let text = "value: \"shpss_\" ok\n  - textbox [ref=e3]: tskey-\n\u{e9}t\u{e9} no newline";
    assert_eq!(mask(text), (Some(0), text.to_string()));
}
