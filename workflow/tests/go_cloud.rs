//! `workflow go <project> --cloud` through the real binary, with scripts in
//! the place of mem (`$WORKFLOW_MEM`) and claude (`$WORKFLOW_CLAUDE`).

#[path = "../src/scratch.rs"]
mod scratch;

use std::path::PathBuf;
use std::process::{Command, Output};

use scratch::Scratch;

/// `gamma`, `shopify_apps` and `delta`, each with a checkout and an approved
/// roadmap. The fake mem logs every call to `mem.log` and answers from the
/// scratch files; the fake claude logs its argv and its folder to
/// `claude.log` and prints what `claude --cloud` prints.
struct World {
    work: Scratch,
}

const SESSION: &str = "session_01TestCloudRun";

impl World {
    fn new(tag: &str) -> World {
        let work = Scratch::new(tag);
        let mut rows = Vec::new();
        for name in ["gamma", "shopify_apps", "delta"] {
            let checkout = work.mkdir(&format!("{name}-checkout"));
            git(&checkout, &["init", "-q", "-b", "main"]);
            rows.push(format!(
                r#"{{"name":"{name}","remote":"github.com/me/{name}","checkouts":["{}"],"roadmap_status":"approved","milestone":"m2-next"}}"#,
                checkout.display()
            ));
            work.put(
                &format!("roadmap-{name}.md"),
                &format!("# roadmap: {name}\n\n- [x] m1-first One\n- [ ] m2-next Two\n- [ ] m3-last Three\n"),
            );
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
roadmap) cat "$dir/roadmap-$3.md" ;;
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
printf '\033[>4mCreated cloud session: Run m2-next\r\n'
printf 'View: https://claude.ai/code/{SESSION}?from=cli&m=0\r\n'
printf 'Branch main is 3 commits ahead of origin/main (as last fetched)\r\n'
"#
            ),
        );
        work.script(
            "amx",
            r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/amx.log"
case "$1" in
ls) echo '[]' ;;
new) echo "started $3" ;;
esac
"#,
        );
        World { work }
    }

    /// A local `workflow go`, with amx faked.
    fn go_local(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_AMX", self.work.at("amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env("WORKFLOW_MACHINE", "mini")
            .env_remove("AMX_ID")
            .current_dir(self.work.path())
            .output()
            .unwrap()
    }

    fn amx_started(&self) -> bool {
        self.work.has("amx.log")
            && self
                .work
                .read("amx.log")
                .lines()
                .any(|l| l.starts_with("new "))
    }

    /// An origin for gamma's checkout holding main, and a cloud run's branch
    /// `cloud/gamma-m1-first` one commit ahead of it. Returns that tip.
    fn origin_with_cloud_branch(&self) -> String {
        let origin = self.work.at("origin.git");
        git(
            self.work.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        let checkout = self.checkout("gamma");
        git(
            &checkout,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        commit(&checkout, "start");
        git(&checkout, &["push", "-q", "origin", "main"]);
        let cloud = self.work.at("cloud-clone");
        git(
            self.work.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                cloud.to_str().unwrap(),
            ],
        );
        git(&cloud, &["checkout", "-q", "-b", "cloud/gamma-m1-first"]);
        commit(&cloud, "Land t1 in the cloud");
        git(&cloud, &["push", "-q", "origin", "cloud/gamma-m1-first"]);
        head(&cloud)
    }
}

fn commit(dir: &std::path::Path, message: &str) {
    git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ],
    );
}

fn head(dir: &std::path::Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl World {
    fn go(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_CLAUDE", self.work.at("claude"))
            .env("WORKFLOW_AMX", self.work.at("no-amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env("WORKFLOW_MACHINE", "mini")
            .env_remove("AMX_ID")
            .current_dir(self.work.path())
            .output()
            .unwrap()
    }

    /// Give `project` a runner record, spliced into its row.
    fn runner(&self, project: &str, runner: &str) {
        let projects = self.work.read("projects.json").replacen(
            &format!(r#""name":"{project}","#),
            &format!(r#""name":"{project}","runner":{runner},"#),
            1,
        );
        self.work.put("projects.json", &projects);
    }

    fn set(&self, project: &str, key_value: &str) {
        let projects = self.work.read("projects.json").replacen(
            &format!(r#""name":"{project}","#),
            &format!(r#""name":"{project}",{key_value},"#),
            1,
        );
        self.work.put("projects.json", &projects);
    }

    fn mem_calls(&self) -> Vec<String> {
        if !self.work.has("mem.log") {
            return Vec::new();
        }
        self.work
            .read("mem.log")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn claude_ran(&self) -> bool {
        self.work.has("claude.log")
    }

    fn checkout(&self, project: &str) -> PathBuf {
        self.work.at(&format!("{project}-checkout"))
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
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

#[test]
fn print_shows_the_prompt_naming_the_cloud_branch_and_starts_nothing() {
    let w = World::new("cloud-print");
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let prompt = stdout(&out);
    assert!(prompt.contains("cloud/gamma-m2-next"), "{prompt}");
    assert!(prompt.contains("origin/main"), "cut from main: {prompt}");
    assert!(prompt.contains("orchestrate skill"), "{prompt}");
    assert!(prompt.contains("mem project set runner cloud"), "{prompt}");
    assert!(prompt.contains("mem roadmap --tick m2-next"), "{prompt}");
    assert!(prompt.contains("parked:"), "{prompt}");
    assert!(!prompt.contains("/goal"), "no slash command: {prompt}");
    assert!(!w.claude_ran());
    assert!(
        !w.mem_calls().iter().any(|c| c.contains("set runner")),
        "{:?}",
        w.mem_calls()
    );
}

#[test]
fn an_underscore_project_keeps_its_name_in_the_branch() {
    let w = World::new("cloud-underscore");
    let out = w.go(&["shopify_apps", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("cloud/shopify_apps-m2-next"),
        "{}",
        stdout(&out)
    );
    let check = Command::new("git")
        .args(["check-ref-format", "--branch", "cloud/shopify_apps-m2-next"])
        .output()
        .unwrap();
    assert!(check.status.success(), "{check:?}");
}

#[test]
fn a_start_runs_claude_cloud_in_the_checkout_records_the_runner_and_prints_the_url() {
    let w = World::new("cloud-start");
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        format!("https://claude.ai/code/{SESSION}")
    );
    assert!(
        stderr(&out).contains("3 commits ahead"),
        "claude's notice passes on: {}",
        stderr(&out)
    );

    let log = w.work.read("claude.log");
    let mut lines = log.lines();
    assert_eq!(lines.next(), Some(w.checkout("gamma").to_str().unwrap()));
    assert_eq!(lines.next(), Some("--cloud"));
    let printed = stdout(&w.go(&["gamma", "--cloud", "--print"]));
    assert_eq!(
        lines.collect::<Vec<_>>().join("\n"),
        printed.trim_end(),
        "the prompt is the one --print shows"
    );
    assert!(
        w.mem_calls().iter().any(|c| c
            == &format!("project set runner cloud {SESSION} --milestone m2-next --project gamma")),
        "{:?}",
        w.mem_calls()
    );
}

#[test]
fn a_live_cloud_run_anywhere_refuses_a_second_naming_its_project() {
    let w = World::new("cloud-one-live");
    w.runner(
        "delta",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Other","since":"{}","milestone":"m2-next"}}"#,
            now_minus(1)
        ),
    );
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("delta"), "{}", stderr(&out));
    assert!(!w.claude_ran());
}

#[test]
fn a_parked_cloud_run_still_holds_the_one_cloud_slot() {
    let w = World::new("cloud-parked");
    w.runner(
        "delta",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Other","since":"{}","milestone":"m2-next"}}"#,
            now_minus(2)
        ),
    );
    w.set("delta", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work.put(
        "handoff-delta.md",
        "parked: t2 is local-only, question #ABCD1234\n",
    );
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("delta"), "{}", stderr(&out));
}

#[test]
fn a_run_that_landed_wrote_a_handoff_or_lapsed_frees_the_slot() {
    // Landed: its milestone is ticked, so the row's next one differs.
    let w = World::new("cloud-landed");
    w.runner(
        "delta",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Other","since":"{}","milestone":"m1-first"}}"#,
            now_minus(1)
        ),
    );
    assert_eq!(w.go(&["gamma", "--cloud"]).status.code(), Some(0));

    // Lapsed: no handoff and no tick for 7 hours.
    let w = World::new("cloud-lapsed");
    w.runner(
        "delta",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Other","since":"{}","milestone":"m2-next"}}"#,
            now_minus(8)
        ),
    );
    assert_eq!(w.go(&["gamma", "--cloud"]).status.code(), Some(0));
}

#[test]
fn a_next_run_after_a_landed_cloud_run_is_cut_from_its_branch() {
    let w = World::new("cloud-base");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Before","since":"{}","milestone":"m1-first"}}"#,
            now_minus(3)
        ),
    );
    let prompt = stdout(&w.go(&["gamma", "--cloud", "--print"]));
    assert!(prompt.contains("origin/cloud/gamma-m1-first"), "{prompt}");
    assert!(prompt.contains("cloud/gamma-m2-next"), "{prompt}");
}

#[test]
fn a_paused_project_refuses_even_with_a_parked_handoff() {
    let w = World::new("cloud-paused");
    w.set("gamma", r#""paused":"2026-10-15T09:00:00Z by saiful""#);
    w.work
        .put("handoff-gamma.md", "parked: waiting on #ABCD1234\n");
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout(&out).trim(), "gamma: paused");
    assert!(!w.claude_ran());
}

#[test]
fn claude_printing_no_session_fails_and_records_nothing() {
    let w = World::new("cloud-no-session");
    w.work
        .script("claude", "echo 'Error: not logged in' >&2; exit 1");
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(stderr(&out).contains("not logged in"), "{}", stderr(&out));
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}

#[test]
fn the_next_local_go_fast_forwards_the_trunk_to_a_landed_cloud_runs_branch() {
    let w = World::new("cloud-ff");
    let tip = w.origin_with_cloud_branch();
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Before","since":"{}","milestone":"m1-first"}}"#,
            now_minus(3)
        ),
    );
    let out = w.go_local(&["gamma"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        head(&w.checkout("gamma")),
        tip,
        "the trunk's tip is the cloud branch's"
    );
    assert!(
        w.mem_calls()
            .iter()
            .any(|c| c == "project unset runner --project gamma"),
        "{:?}",
        w.mem_calls()
    );
    assert!(w.amx_started());
}

#[test]
fn a_trunk_that_moved_while_the_cloud_ran_refuses_and_starts_nothing() {
    let w = World::new("cloud-diverged");
    w.origin_with_cloud_branch();
    let checkout = w.checkout("gamma");
    commit(&checkout, "A local commit the cloud never saw");
    let before = head(&checkout);
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Before","since":"{}","milestone":"m1-first"}}"#,
            now_minus(3)
        ),
    );
    let out = w.go_local(&["gamma"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("cannot fast-forward"),
        "{}",
        stderr(&out)
    );
    assert_eq!(head(&checkout), before);
    assert!(!w.amx_started());
    assert!(!w.mem_calls().iter().any(|c| c.contains("unset runner")));
}

#[test]
fn a_live_or_parked_cloud_run_keeps_the_mini_out_of_its_project() {
    let w = World::new("cloud-local-live");
    w.origin_with_cloud_branch();
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Now","since":"{}","milestone":"m2-next"}}"#,
            now_minus(1)
        ),
    );
    let out = w.go_local(&["gamma"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("session_01Now"), "{}", stderr(&out));
    assert!(!w.amx_started());
}

#[test]
fn an_untrusted_folder_is_refused_at_once_with_how_to_trust_it() {
    let w = World::new("cloud-untrusted");
    // claude's trust prompt, drawn word by word as the TUI does, then a wait
    // for a key that never comes.
    w.work.script(
        "claude",
        "printf 'Quick safety check:\\033[1CIs\\033[1Cthis\\033[1Ca\\033[1Cproject\\r\\n\\033[1CYes,\\033[1CI\\033[1Ctrust\\033[1Cthis\\033[1Cfolder\\r\\n'; sleep 30",
    );
    let started = std::time::Instant::now();
    let out = w.go(&["gamma", "--cloud"]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let said = stderr(&out);
    assert!(said.contains("trust"), "{said}");
    assert!(
        said.contains(w.checkout("gamma").to_str().unwrap()),
        "{said}"
    );
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}

#[test]
fn a_parked_run_frees_the_slot_after_seven_hours() {
    let w = World::new("cloud-parked-lapsed");
    w.runner(
        "delta",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Other","since":"{}","milestone":"m2-next"}}"#,
            now_minus(8)
        ),
    );
    w.set("delta", &format!(r#""handoff_at":"{}""#, now_minus(2)));
    w.work
        .put("handoff-delta.md", "parked: 6-hour stop, t3 half done\n");
    assert_eq!(w.go(&["gamma", "--cloud"]).status.code(), Some(0));
}

#[test]
fn a_project_on_a_branch_of_its_own_or_with_no_remote_stays_off_the_cloud() {
    let w = World::new("cloud-worktree");
    let dir = w.checkout("gamma");
    w.set("gamma", &format!(r#""worktree":"{}""#, dir.display()));
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("branch of its own"),
        "{}",
        stderr(&out)
    );

    let w = World::new("cloud-no-remote");
    let projects =
        w.work
            .read("projects.json")
            .replacen(r#","remote":"github.com/me/gamma""#, "", 1);
    w.work.put("projects.json", &projects);
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("no git remote"), "{}", stderr(&out));
}

#[test]
fn a_device_orchestrator_on_another_machine_keeps_the_cloud_out() {
    let w = World::new("cloud-device-runner");
    w.runner(
        "gamma",
        r#"{"machine":"nuc","id":"gamma-m2-next","since":"2026-10-15T07:00:00Z"}"#,
    );
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("nuc"), "{}", stderr(&out));
    assert!(!w.claude_ran());
}

#[test]
fn a_cloud_run_takes_the_next_milestone_only() {
    let w = World::new("cloud-later-milestone");
    let out = w.go(&["gamma", "--cloud", "--milestone", "m3-last"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("m2-next"), "{}", stderr(&out));
    assert!(!w.claude_ran());
}

#[test]
fn a_cloud_run_that_landed_the_last_milestone_still_reaches_the_trunk() {
    let w = World::new("cloud-ff-last");
    let tip = w.origin_with_cloud_branch();
    w.work.put(
        "roadmap-gamma.md",
        "# roadmap: gamma\n\n- [x] m1-first One\n",
    );
    let projects = w.work.read("projects.json").replacen(
        r#""name":"gamma","remote":"github.com/me/gamma","#,
        &format!(
            r#""name":"gamma","remote":"github.com/me/gamma","runner":{{"machine":"cloud","id":"session_01Last","since":"{}","milestone":"m1-first"}},"#,
            now_minus(2)
        ),
        1,
    ).replacen(r#""milestone":"m2-next"}"#, r#""milestone":null}"#, 1);
    w.work.put("projects.json", &projects);
    let out = w.go_local(&["gamma"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "gamma: the roadmap is done");
    assert_eq!(head(&w.checkout("gamma")), tip);
    assert!(
        w.mem_calls()
            .iter()
            .any(|c| c == "project unset runner --project gamma"),
        "{:?}",
        w.mem_calls()
    );
    assert!(!w.amx_started());
}

#[test]
fn a_landed_run_whose_branch_is_missing_refuses_and_keeps_its_record() {
    let w = World::new("cloud-ff-missing");
    w.origin_with_cloud_branch();
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Gone","since":"{}","milestone":"m0-gone"}}"#,
            now_minus(3)
        ),
    );
    let out = w.go_local(&["gamma"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("cloud/gamma-m0-gone"),
        "{}",
        stderr(&out)
    );
    assert!(!w.mem_calls().iter().any(|c| c.contains("unset runner")));
    assert!(!w.amx_started());
}

#[test]
fn the_runner_record_reaches_the_hub_before_the_session_writes_its_own() {
    let w = World::new("cloud-sync-after-record");
    assert_eq!(w.go(&["gamma", "--cloud"]).status.code(), Some(0));
    let calls = w.mem_calls();
    let set = calls
        .iter()
        .position(|c| c.starts_with("project set runner cloud"));
    let sync = calls.iter().rposition(|c| c.starts_with("sync"));
    assert!(set.is_some() && sync > set, "{calls:?}");
}

/// A cloud run clones origin, so work the trunk holds and origin/main lacks,
/// a fast-forwarded cloud branch or a milestone the mini landed, would be
/// missing from it. Pushing main is Saiful's, so the run waits for that.
#[test]
fn a_trunk_ahead_of_origin_main_refuses_until_main_is_pushed() {
    let w = World::new("cloud-unpushed");
    w.origin_with_cloud_branch();
    let checkout = w.checkout("gamma");
    commit(&checkout, "Land t1 on the mini");
    commit(&checkout, "Land t2 on the mini");
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("gamma's main is 2 commits ahead of origin/main; push main first"),
        "{}",
        stderr(&out)
    );
    assert!(!w.claude_ran());

    git(&checkout, &["push", "-q", "origin", "main"]);
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}
