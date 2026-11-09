//! `workflow project-text <project>`: the text pasted once into a Claude
//! Project's instructions field, through the real binary with a script in
//! the place of mem.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};

use scratch::Scratch;

fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    work.put(
        "projects.json",
        r#"{"projects":[{"name":"cloud_walk","remote":"github.com/saifulapm/cloud-spike","checkouts":[],"roadmap_status":"approved","milestone":"w4-bytes"},{"name":"local_only","checkouts":[]}]}"#,
    );
    work.script(
        "mem",
        r#"
dir=$(dirname "$0")
case "$1" in
projects) cat "$dir/projects.json" ;;
project) exit 1 ;;
esac
"#,
    );
    work
}

fn text(work: &Scratch, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("project-text")
        .args(args)
        .env("WORKFLOW_MEM", work.at("mem"))
        .current_dir(work.path())
        .output()
        .unwrap()
}

#[test]
fn the_text_fits_the_field_and_names_the_start_line_and_both_parts() {
    let work = world("project-text");
    let out = text(&work, &["cloud_walk"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let said = String::from_utf8_lossy(&out.stdout);
    // claude.ai's Project instructions take 16,000 characters.
    assert!(said.chars().count() < 16_000, "{}", said.chars().count());
    assert!(
        said.contains("\"Run the next milestone of cloud_walk.\""),
        "{said}"
    );
    // The coordinator routes on the report's last line, and starts nothing else.
    for line in [
        "next: Run the next milestone of cloud_walk.",
        "done: cloud_walk's roadmap is done",
        "parked:",
        "Start nothing else",
    ] {
        assert!(said.contains(line), "{line}: {said}");
    }
    // The thread works in its clone, which a woken machine leaves.
    assert!(said.contains("cd /home/user/cloud-spike"), "{said}");
    assert!(
        said.contains("workflow go cloud_walk --cloud --print"),
        "{said}"
    );
    // A thread that runs nothing is on main, where the stop hook's commit
    // and push would land untested.
    assert!(said.contains("commit nothing and push nothing"), "{said}");
    assert!(
        !said.contains("{project}") && !said.contains("{repo}"),
        "{said}"
    );
}

#[test]
fn an_unknown_project_or_one_with_no_remote_is_refused() {
    let work = world("project-text-refused");
    let out = text(&work, &["nope"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("nope"),
        "{out:?}"
    );
    let out = text(&work, &["local_only"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no git remote"),
        "{out:?}"
    );
}
