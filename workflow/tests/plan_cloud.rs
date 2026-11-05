//! `workflow plan <project> --cloud` through the real binary, with scripts in
//! the place of mem (`$WORKFLOW_MEM`) and claude (`$WORKFLOW_CLAUDE`).

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::{Command, Output};

use scratch::Scratch;

const SESSION: &str = "session_01TestCloudPlan";

/// `gamma` with a checkout, a remote and no roadmap yet, and `delta` beside
/// it. The fake mem logs every call to `mem.log` and answers from the
/// scratch files; the fake claude logs its folder and argv to `claude.log`
/// and prints what `claude --cloud` prints.
fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let mut rows = Vec::new();
    for name in ["gamma", "delta"] {
        let checkout = work.mkdir(&format!("{name}-checkout"));
        git(&checkout, &["init", "-q", "-b", "main"]);
        rows.push(format!(
            r#"{{"name":"{name}","remote":"github.com/me/{name}","checkouts":["{}"]}}"#,
            checkout.display()
        ));
    }
    work.put(
        "projects.json",
        &format!(r#"{{"projects":[{}]}}"#, rows.join(",")),
    );
    work.script(
        "mem",
        r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/mem.log"
case "$1" in
projects) cat "$dir/projects.json" ;;
project) [ "$2" = current ] && exit 1; exit 0 ;;
handoff) [ -f "$dir/handoff-$3.md" ] && cat "$dir/handoff-$3.md" || exit 1 ;;
esac
"#,
    );
    work.script(
        "claude",
        &format!(
            r#"
dir=$(dirname "$0")
printf '%s\n' "$PWD" "$@" >"$dir/claude.log"
[ -t 1 ] || {{ echo "Error: --cloud requires an interactive terminal." >&2; exit 1; }}
printf 'Created cloud session: Plan gamma\r\n'
printf 'View: https://claude.ai/code/{SESSION}?from=cli&m=0\r\n'
"#
        ),
    );
    work
}

/// Splice `key_value` into `project`'s row.
fn set(work: &Scratch, project: &str, key_value: &str) {
    let projects = work.read("projects.json").replacen(
        &format!(r#""name":"{project}","#),
        &format!(r#""name":"{project}",{key_value},"#),
        1,
    );
    work.put("projects.json", &projects);
}

fn run(work: &Scratch, cmd: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg(cmd)
        .args(args)
        .env("WORKFLOW_MEM", work.at("mem"))
        .env("WORKFLOW_CLAUDE", work.at("claude"))
        .env("WORKFLOW_AMX", work.at("no-amx"))
        .env("XDG_CONFIG_HOME", work.at("config"))
        .env("XDG_STATE_HOME", work.at("state"))
        .env("WORKFLOW_MACHINE", "mini")
        .env_remove("AMX_ID")
        .current_dir(work.path())
        .output()
        .unwrap()
}

fn now_minus(hours: i64) -> String {
    let out = Command::new("date")
        .args([
            "-u",
            "-d",
            &format!("{hours} hours ago"),
            "+%Y-%m-%dT%H:%M:%SZ",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[test]
fn print_shows_the_plan_prompt_on_its_branch_and_starts_nothing() {
    let work = world("plan-print");
    let out = run(&work, "plan", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let prompt = text(&out.stdout);
    assert!(
        prompt.contains("Plan gamma in this cloud session"),
        "{prompt}"
    );
    assert!(prompt.contains("the plan skill"), "{prompt}");
    assert!(
        prompt.contains("refreshes your skills"),
        "skills read after the refresh: {prompt}"
    );
    assert!(prompt.contains("cloud/gamma-plan"), "{prompt}");
    assert!(prompt.contains("--milestone plan"), "{prompt}");
    assert!(prompt.contains("mem questions --wait"), "{prompt}");
    assert!(!work.has("claude.log"));
}

#[test]
fn it_starts_a_cloud_session_in_the_checkout_and_records_the_plan_runner() {
    let work = world("plan-start");
    let out = run(&work, "plan", &["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        text(&out.stdout).contains(&format!("https://claude.ai/code/{SESSION}")),
        "{out:?}"
    );
    let log = work.read("claude.log");
    let mut lines = log.lines();
    assert_eq!(
        lines.next(),
        Some(work.at("gamma-checkout").to_str().unwrap())
    );
    assert_eq!(lines.next(), Some("--cloud"));
    assert!(log.contains("Plan gamma in this cloud session"), "{log}");
    let mem = work.read("mem.log");
    assert!(
        mem.lines()
            .any(|l| l
                == format!("project set runner cloud {SESSION} --milestone plan --project gamma")),
        "{mem}"
    );
}

#[test]
fn a_project_with_no_remote_is_refused() {
    let work = world("plan-no-remote");
    let projects = work
        .read("projects.json")
        .replace(r#""remote":"github.com/me/gamma","#, "");
    work.put("projects.json", &projects);
    let out = run(&work, "plan", &["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        text(&out.stderr).contains("gamma has no git remote"),
        "{out:?}"
    );
    assert!(!work.has("claude.log"));
}

#[test]
fn live_cloud_runs_elsewhere_hold_no_plan_back() {
    let work = world("plan-busy");
    // A third project, so two others run in the cloud.
    let projects = work.read("projects.json");
    let rows = projects.trim_end().strip_suffix("]}").unwrap();
    work.put(
        "projects.json",
        &format!(
            r#"{rows},{{"name":"epsilon","remote":"github.com/me/epsilon","checkouts":[]}}]}}"#
        ),
    );
    for (project, id) in [("delta", "session_01Other"), ("epsilon", "session_01More")] {
        set(
            &work,
            project,
            &format!(
                r#""milestone":"m2-next","runner":{{"machine":"cloud","id":"{id}","since":"{}","milestone":"m2-next"}}"#,
                now_minus(1)
            ),
        );
    }
    let out = run(&work, "plan", &["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(work.has("claude.log"));
}

#[test]
fn a_running_cloud_plan_elsewhere_holds_no_plan_back() {
    let work = world("plan-second-slot");
    set(
        &work,
        "delta",
        &format!(
            r#""runner":{{"machine":"cloud","id":"session_01Plan","since":"{}","milestone":"plan"}}"#,
            now_minus(1)
        ),
    );
    let out = run(&work, "plan", &["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(work.has("claude.log"));
}

#[test]
fn a_project_whose_own_cloud_plan_runs_plans_no_second() {
    let work = world("plan-own-live");
    set(
        &work,
        "gamma",
        &format!(
            r#""runner":{{"machine":"cloud","id":"session_01Plan","since":"{}","milestone":"plan"}}"#,
            now_minus(1)
        ),
    );
    let out = run(&work, "plan", &["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        text(&out.stderr).contains("gamma already has a cloud run: plan, session_01Plan"),
        "{out:?}"
    );
    assert!(!work.has("claude.log"));
}

#[test]
fn the_first_cloud_milestone_after_a_cloud_plan_is_cut_from_the_plan_branch() {
    let work = world("plan-then-go");
    set(
        &work,
        "gamma",
        &format!(
            r#""roadmap_status":"approved","milestone":"m1-first","handoff_at":"{}","runner":{{"machine":"cloud","id":"session_01Plan","since":"{}","milestone":"plan"}}"#,
            now_minus(1),
            now_minus(2)
        ),
    );
    work.put("handoff-gamma.md", "Next: workflow go gamma --cloud\n");
    work.put(
        "roadmap-gamma.md",
        "# roadmap: gamma\n\n- [ ] m1-first One\n",
    );
    let mem = work.read("mem");
    work.script(
        "mem",
        &mem.replace(
            "projects) cat",
            "roadmap) cat \"$dir/roadmap-$3.md\" ;;\nprojects) cat",
        ),
    );
    let out = run(&work, "go", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        text(&out.stdout).contains("origin/cloud/gamma-plan"),
        "{out:?}"
    );
}
