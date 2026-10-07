//! The command surface, through the real binary: what `help` lists, and how an
//! unknown command differs from an unknown option.

use std::process::{Command, Output};

fn workflow(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(args)
        .output()
        .unwrap()
}

/// The command names `help` lists: the lines indented by two spaces.
fn listed(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn help_lists_the_five_commands_and_nothing_else() {
    for flag in ["help", "--help", "-h"] {
        let out = workflow(&[flag]);
        assert_eq!(out.status.code(), Some(0), "{flag}");
        assert_eq!(
            listed(&out),
            ["hygiene", "lint-msg", "hook", "install", "go"],
            "{flag}"
        );
    }
}

#[test]
fn no_command_prints_the_usage_and_exits_2() {
    let out = workflow(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(listed(&out).len(), 5);
}

#[test]
fn every_verb_the_engine_had_is_an_unknown_command() {
    for verb in [
        "run",
        "serve",
        "status",
        "reap",
        "park",
        "pause",
        "resume",
        "dogfood",
        "redispatch",
        "accept",
        "regate",
        "wait",
        "report",
        "verify",
        "plan-check",
        "review-needed",
        "docs",
        "skill",
        "doctor",
    ] {
        let out = workflow(&[verb]);
        assert_eq!(out.status.code(), Some(2), "{verb}");
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            said.contains(&format!("unknown command: {verb}")),
            "{verb}: {said}"
        );
    }
}

#[test]
fn a_mistyped_option_is_not_a_mistyped_command() {
    for verb in ["hygiene", "lint-msg", "install", "go"] {
        let out = workflow(&[verb, "--frob"]);
        let said = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{verb}");
        assert!(!said.contains("unknown command"), "{verb}: {said}");
        assert!(said.contains("--frob"), "{verb}: {said}");
    }
}
