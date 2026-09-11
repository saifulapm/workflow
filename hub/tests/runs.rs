//! Ruling 5 — the overview's runs section, live off `workflow status --json`
//! in this machine's checkout of the project.

mod common;

use std::path::PathBuf;

use common::{
    Hub, TempDir, body_of, fixture_bin, fixture_mem, real_mem, recording_mem, seed_project,
    status_of,
};
use hub::memcli::MemCli;
use hub::model;

const PROJECT: &str = "proj-runs";

/// A projects doc with two rows: one with checkouts, one without — enough
/// to prove `checkout_of` takes the first entry and answers `None` for an
/// empty array or an unknown name.
const PROJECTS_DOC: &str = r#"{"projects":[
    {"id":"p1","name":"proj-alpha","remote":null,"aliases":[],"created":"2026-01-01",
     "items":0,"current":false,"checkouts":["/checkouts/alpha","/checkouts/alpha-old"]},
    {"id":"p2","name":"proj-beta","remote":null,"aliases":[],"created":"2026-01-01",
     "items":0,"current":false,"checkouts":[]}
]}"#;

#[test]
fn checkout_of_runs_reads_the_first_registered_checkout() {
    let dir = TempDir::new("runs-checkout-of");
    let bin = dir.join("bin");
    fixture_mem(&bin, &format!("echo '{PROJECTS_DOC}'"));
    let mem = MemCli::with_path(bin);

    assert_eq!(
        model::checkout_of(&mem, "proj-alpha").as_deref(),
        Some("/checkouts/alpha")
    );
    assert_eq!(
        model::checkout_of(&mem, "proj-beta"),
        None,
        "an empty checkouts array is no checkout"
    );
    assert_eq!(
        model::checkout_of(&mem, "proj-unknown"),
        None,
        "mem does not know this project"
    );
}

fn world(tag: &str) -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (mem_dir, _log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, PROJECT, "runs did a thing");
    (dir, home, mem_dir)
}

/// `workflow status --json`'s own shape (`workflow/src/status.rs`), one run
/// with two tasks.
const STATUS_DOC: &str = r#"{"project":"proj-runs","runs":[{"plan":"m1","live":true,
"base":"deadbeef","integration":"integration/m1","tasks":[
{"id":"t1","state":"merged","dispatches":2,"session":"s1","failed":"",
 "last_status":"ready: done","merged":"abc123","context":0},
{"id":"t2","state":"dispatched","dispatches":1,"session":"s2","failed":"",
 "last_status":"","merged":"","context":0}
]}]}"#;

#[test]
fn the_runs_section_lists_each_task_when_workflow_answers() {
    let (dir, home, mem_dir) = world("runs-page-lists");
    let workflow_dir = dir.join("workflow-bin");
    fixture_bin(&workflow_dir, "workflow", &format!("echo '{STATUS_DOC}'"));

    let hub = Hub::spawn(&home, &[&mem_dir, &workflow_dir], &["--port", "0"]);
    let response = hub.get(&format!("/p/{PROJECT}"));
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);

    assert!(body.contains("Runs on"), "runs heading: {body}");
    assert!(body.contains("m1"), "plan: {body}");
    assert!(
        body.contains("integration/m1"),
        "integration branch: {body}"
    );
    assert!(body.contains("t1"), "first task id: {body}");
    assert!(body.contains("merged"), "first task state: {body}");
    assert!(body.contains("t2"), "second task id: {body}");
    assert!(body.contains("dispatched"), "second task state: {body}");
    assert!(!body.contains("not installed"), "{body}");
}

#[test]
fn the_runs_section_says_workflow_is_not_installed_without_one() {
    let (_dir, home, mem_dir) = world("runs-page-missing");
    let hub = Hub::spawn(&home, &[&mem_dir], &["--port", "0"]);
    let response = hub.get(&format!("/p/{PROJECT}"));
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);

    assert!(body.contains("Runs on"), "runs heading: {body}");
    assert!(
        body.contains("workflow is not installed here"),
        "not-installed sentence: {body}"
    );
}
