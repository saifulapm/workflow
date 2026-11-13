//! `workflow project-new <project>`: the steps that put a project into a
//! Claude Project on both accounts, and the checks a machine can make,
//! through the real binary with scripts in the place of mem and ssh.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};

use scratch::Scratch;

/// This machine, as mem names it.
const HERE: &str = "mac-mini-m2";

/// A world whose mem lists `projects` (JSON rows, `{root}` standing for the
/// scratch folder) and logs every call it gets.
fn world(tag: &str, projects: &str) -> Scratch {
    let work = Scratch::new(tag);
    let rows = projects.replace("{root}", &work.path().display().to_string());
    work.put("projects.json", &format!(r#"{{"projects":[{rows}]}}"#));
    work.script(
        "mem",
        r#"
dir=$(dirname "$0")
printf '%s\n' "$*" >>"$dir/mem-calls"
case "$1" in
projects) cat "$dir/projects.json" ;;
*) exit 1 ;;
esac
"#,
    );
    work.put("config/qshell/machine", &format!("{HERE}\n"));
    work.mkdir("pass");
    work
}

fn run(work: &Scratch, project: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .args(["project-new", project])
        .env("WORKFLOW_MEM", work.at("mem"))
        .env("XDG_CONFIG_HOME", work.at("config"))
        .env("PASSWORD_STORE_DIR", work.at("pass"))
        .env("HUB_LOCAL_SOCKET", work.at("no-hub.sock"))
        .env("WORKFLOW_SSH", work.at("ssh"))
        .current_dir(work.path())
        .output()
        .unwrap()
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The lines under `heading`, up to the next blank line.
fn section(text: &str, heading: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .skip_while(|l| l.trim_end_matches(':') != heading);
    assert!(lines.next().is_some(), "no heading {heading:?} in:\n{text}");
    lines
        .take_while(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
        .collect()
}

/// The numbers the lines start with, as `1.` gives 1.
fn numbers(lines: &[String]) -> Vec<u32> {
    lines
        .iter()
        .filter_map(|l| l.split_once(". ").and_then(|(n, _)| n.parse().ok()))
        .collect()
}

fn trendsi(tag: &str) -> Scratch {
    let work = world(
        tag,
        r#"{"name":"lareys-trendsi","remote":"github.com/saifulapm/lareys-trendsi","checkouts":["{root}/lareys-trendsi"],"subdir":null,"device":null,"device_by":null,"cloud_secrets":null}"#,
    );
    work.mkdir("lareys-trendsi");
    work
}

#[test]
fn it_prints_three_steps_for_each_account_naming_the_repo_and_the_text() {
    let work = trendsi("steps");
    let out = run(&work, "lareys-trendsi");
    let text = said(&out);

    let once = section(&text, "Once, on any device");
    assert!(
        once.iter()
            .any(|l| l.contains("`mem project set device <machine>`")),
        "{text}"
    );
    for account in ["On the work account", "On the main account"] {
        let steps = section(&text, account);
        assert_eq!(numbers(&steps), [1, 2, 3], "{account}:\n{text}");
        let all = steps.join("\n");
        assert!(all.contains("saifulapm/lareys-trendsi"), "{all}");
        assert!(all.contains("environment `workflow`"), "{all}");
        assert!(
            all.contains("`workflow project-text lareys-trendsi`"),
            "{all}"
        );
    }
}

#[test]
fn a_submodule_repo_is_added_to_the_project_in_step_two() {
    let work = world(
        "submodule",
        r#"{"name":"shopify_apps","remote":"github.com/lareysbd/shopify_apps","checkouts":["{root}/shopify_apps"],"subdir":null},{"name":"palmshop","remote":"github.com/lareysbd/shopify_apps","checkouts":["{root}/shopify_apps/apps/palmshop"],"subdir":"apps/palmshop"}"#,
    );
    work.put(
        "shopify_apps/.gitmodules",
        "[submodule \"apps/palmshop/engine\"]\n\tpath = apps/palmshop/engine\n\turl = ../dactyl.git\n[submodule \"vendor/kit\"]\n\tpath = vendor/kit\n\turl = https://github.com/saifulapm/kit.git\n",
    );
    let out = run(&work, "shopify_apps");
    let text = said(&out);
    for account in ["On the work account", "On the main account"] {
        let steps = section(&text, account);
        let two = steps
            .iter()
            .find(|l| l.starts_with("2. "))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(two.contains("lareysbd/shopify_apps"), "{two}");
        assert!(two.contains("lareysbd/dactyl"), "{two}");
        assert!(two.contains("saifulapm/kit"), "{two}");
    }
}

#[test]
fn it_writes_nothing_and_only_reads_mem() {
    let work = trendsi("writes");
    run(&work, "lareys-trendsi");
    let calls = work.read("mem-calls");
    assert!(!calls.trim().is_empty());
    for call in calls.lines() {
        assert_eq!(call, "projects --json", "mem was called with {call:?}");
    }
}

#[test]
fn an_unknown_project_is_refused() {
    let work = trendsi("unknown");
    let out = run(&work, "nosuch");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("nosuch"));
}
