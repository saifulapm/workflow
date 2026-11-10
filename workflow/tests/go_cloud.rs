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

/// What every fake `mem sync --json` prints through (tests/fake_mem_sync.rs).
const SYNC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mem-sync.sh");
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
            &r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/mem.log"
case "$1" in
projects) cat "$dir/projects.json" ;;
project)
  # The checkout the caller stands in, by its folder's name, as a VM's
  # mem knows it: a root project with no subdir.
  if [ "$2" = current ]; then
    [ -f "$dir/current-${PWD##*/}.json" ] || exit 1
    cat "$dir/current-${PWD##*/}.json"
    exit 0
  fi
  # A cloud runner lands in the projects as the real mem's replica holds
  # it: the session's own, or with a file `winner`, the record of a run the
  # store took first.
  if [ "$2 $3 $4" = "set runner cloud" ]; then
    id=$5
    account=
    [ "${10:-}" = --account ] && account=${11}
    [ -f "$dir/winner" ] && id=$(cat "$dir/winner") && account=$(cat "$dir/winner-account" 2>/dev/null)
    python3 - "$dir/projects.json" "$id" "$7" "$9" "$account" <<'PY_EOF'
import json, sys, datetime
path, id, slug, project, account = sys.argv[1:6]
doc = json.load(open(path))
since = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
for row in doc["projects"]:
    if row["name"] == project:
        row["runner"] = {"machine": "cloud", "id": id, "since": since, "milestone": slug, "account": account or None}
json.dump(doc, open(path, "w"))
PY_EOF
  fi
  exit 0 ;;
sync)
  n=0
  [ -f "$dir/outbox" ] && n=$(cat "$dir/outbox")
  # With `outbox-later`, the first sync, a thread's check before it
  # decides, finds nothing waiting, and the later ones find that many.
  if [ -f "$dir/outbox-later" ]; then
    [ -f "$dir/synced-once" ] && n=$(cat "$dir/outbox-later")
    touch "$dir/synced-once"
  fi
  stuck=
  [ -f "$dir/not-performed" ] && stuck=stuck
  sh @SYNC@ "$n" $stuck ;;
roadmap) cat "$dir/roadmap-$3.md" ;;
handoff) [ -f "$dir/handoff-$3.md" ] && cat "$dir/handoff-$3.md" || exit 1 ;;
esac
"#
            .replace("@SYNC@", SYNC),
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
            .env("XDG_STATE_HOME", self.work.at("state"))
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
        self.origin_with_cloud_branch_for("m1-first")
    }

    /// The same with the cloud run of `slug`.
    fn origin_with_cloud_branch_for(&self, slug: &str) -> String {
        let branch = format!("cloud/gamma-{slug}");
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
        git(&cloud, &["checkout", "-q", "-b", &branch]);
        commit(&cloud, "Land t1 in the cloud");
        git(&cloud, &["push", "-q", "origin", &branch]);
        head(&cloud)
    }

    /// The same, with the cloud run landed: its thread pushed the branch's
    /// work to origin's main, as `workflow land` does.
    fn origin_with_landed_main(&self) -> String {
        let tip = self.origin_with_cloud_branch();
        let cloud = self.work.at("cloud-clone");
        git(&cloud, &["push", "-q", "origin", "HEAD:main"]);
        tip
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
        .args(["-c", "core.hooksPath=/dev/null"])
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
            .env("XDG_STATE_HOME", self.work.at("state"))
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

/// The fixture's own git, with no hooks: from inside an agent session the
/// global ones would refuse the push that seeds origin and the task-shaped
/// commit messages.
fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
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

/// The session a thread of a Claude Project runs as, as claude.ai names it
/// in `CLAUDE_CODE_REMOTE_SESSION_ID`, and as its links name it.
const THREAD: &str = "cse_01Thread";
const THREAD_SESSION: &str = "session_01Thread";

impl World {
    /// `workflow go <args>` in a thread of a Claude Project.
    fn thread(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_CLAUDE", self.work.at("claude"))
            .env("WORKFLOW_AMX", self.work.at("no-amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env("XDG_STATE_HOME", self.work.at("state"))
            .env("CLAUDE_CODE_REMOTE_SESSION_ID", THREAD)
            .env_remove("WORKFLOW_MACHINE")
            .env_remove("WORKFLOW_ACCOUNT")
            .env_remove("AMX_ID")
            .current_dir(self.work.path())
            .output()
            .unwrap()
    }

    /// `workflow go <args>` in a thread standing in `dir`.
    /// A thread of the Project on claude.ai account `account`, whose
    /// environment says which in `WORKFLOW_ACCOUNT`.
    fn thread_as(&self, account: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_CLAUDE", self.work.at("claude"))
            .env("WORKFLOW_AMX", self.work.at("no-amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env("XDG_STATE_HOME", self.work.at("state"))
            .env("CLAUDE_CODE_REMOTE_SESSION_ID", THREAD)
            .env("WORKFLOW_ACCOUNT", account)
            .env_remove("WORKFLOW_MACHINE")
            .env_remove("AMX_ID")
            .current_dir(self.work.path())
            .output()
            .unwrap()
    }

    fn thread_in(&self, dir: &std::path::Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_CLAUDE", self.work.at("claude"))
            .env("WORKFLOW_AMX", self.work.at("no-amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env("XDG_STATE_HOME", self.work.at("state"))
            .env("CLAUDE_CODE_REMOTE_SESSION_ID", THREAD)
            .env_remove("WORKFLOW_MACHINE")
            .env_remove("WORKFLOW_ACCOUNT")
            .env_remove("AMX_ID")
            .current_dir(dir)
            .output()
            .unwrap()
    }

    /// A monorepo `root` cloned as a thread clones it, which mem knows
    /// by its remote alone, with child projects at `children` (name, subdir)
    /// that record no checkout, as on a cloud VM. Returns the clone.
    fn monorepo(&self, root: &str, children: &[(&str, &str)]) -> PathBuf {
        let clone = self.work.mkdir(root);
        git(&clone, &["init", "-q", "-b", "main"]);
        self.work.put(
            &format!("current-{root}.json"),
            &format!(
                r#"{{"id":"01R{root}","name":"{root}","root":"{}"}}"#,
                clone.display()
            ),
        );
        let mut rows = vec![format!(
            r#"{{"name":"{root}","remote":"github.com/me/{root}","checkouts":[],"subdir":null,"roadmap_status":"approved","milestone":"m2-next"}}"#
        )];
        for (name, subdir) in children {
            rows.push(format!(
                r#"{{"name":"{name}","remote":"github.com/me/{root}","checkouts":[],"subdir":"{subdir}","roadmap_status":"approved","milestone":"m2-next"}}"#
            ));
            self.work.put(
                &format!("roadmap-{name}.md"),
                &format!("# roadmap: {name}\n\n- [x] m1-first One\n- [ ] m2-next Two\n"),
            );
        }
        self.work.put(
            &format!("roadmap-{root}.md"),
            &format!("# roadmap: {root}\n\n- [x] m1-first One\n- [ ] m2-next Two\n"),
        );
        let projects = self.work.read("projects.json").replacen(
            r#"{"projects":["#,
            &format!(r#"{{"projects":[{},"#, rows.join(",")),
            1,
        );
        self.work.put("projects.json", &projects);
        clone
    }

    fn claimed_for(&self, project: &str) -> bool {
        self.mem_calls().iter().any(|c| {
            c == &format!(
                "project set runner cloud {THREAD_SESSION} --milestone m2-next --project {project}"
            )
        })
    }

    fn claimed(&self) -> bool {
        self.mem_calls().iter().any(|c| {
            c == &format!(
                "project set runner cloud {THREAD_SESSION} --milestone m2-next --project gamma"
            )
        })
    }
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
    // It lands through workflow land, never by a push of its own, and ends
    // on the line the coordinator reads.
    for said in [
        "`workflow land`",
        "never force a push",
        "never on main",
        "parked: local-only <task>",
        "parked: trunk moved:",
        "parked: main moved:",
        "parked: CI red on",
        "run_in_background",
        "parked: 6-hour stop",
        "never start the milestone over",
    ] {
        assert!(prompt.contains(said), "{said}: {prompt}");
    }
    let dir = w.checkout("gamma");
    assert!(
        prompt.contains(&format!("cd {}", dir.display())),
        "a woken thread goes back to its checkout: {prompt}"
    );
    assert!(!prompt.contains('{'), "a placeholder left: {prompt}");
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
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["check-ref-format", "--branch", "cloud/shopify_apps-m2-next"])
        .output()
        .unwrap();
    assert!(check.status.success(), "{check:?}");
}

/// The Claude Project starts every cloud run: a machine starts none, and
/// says what to type there instead.
#[test]
fn a_cloud_start_from_a_machine_refuses_naming_the_start_line() {
    let w = World::new("cloud-start-refused");
    let out = w.go(&["gamma", "--cloud"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains(r#""Run the next milestone of gamma." in gamma's Claude Project"#),
        "{}",
        stderr(&out)
    );
    assert!(!w.claude_ran());
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}

/// A thread's start claims the run before it prints the prompt: the
/// runner record, then a sync the store has taken all of, then the record
/// read back.
#[test]
fn a_thread_claims_the_run_then_prints_the_prompt() {
    let w = World::new("cloud-thread-start");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("cloud/gamma-m2-next"),
        "{}",
        stdout(&out)
    );
    assert!(w.claimed(), "{:?}", w.mem_calls());
    let calls = w.mem_calls();
    let set = calls
        .iter()
        .position(|c| c.starts_with("project set runner cloud"));
    let sync = calls.iter().rposition(|c| c.starts_with("sync"));
    let reread = calls.iter().rposition(|c| c.starts_with("projects"));
    assert!(set.is_some() && sync > set && reread > sync, "{calls:?}");
    assert!(!w.claude_ran());

    // A woken or resumed thread finds its own run and carries on.
    let again = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(again.status.code(), Some(0), "{}", stdout(&again));
    assert_eq!(stdout(&again), stdout(&out));
}

/// Two threads that claim at once both write the runner; the store keeps
/// the first and the second's replica takes it, so the second parks.
#[test]
fn a_thread_the_store_took_another_claim_over_parks_naming_it() {
    let w = World::new("cloud-thread-race");
    w.work.put("winner", "session_01First");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: gamma already runs m2-next in https://claude.ai/code/session_01First"
    );
}

/// Both claude.ai accounts hold the same Projects: a thread records the
/// account its environment names, so the other account's start can say
/// whose run holds the project.
#[test]
fn a_thread_records_its_account_and_a_start_from_the_other_parks_naming_it() {
    let w = World::new("cloud-thread-account");
    let out = w.thread_as("work", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        w.mem_calls().iter().any(|c| c
            == &format!(
                "project set runner cloud {THREAD_SESSION} --milestone m2-next --project gamma --account work"
            )),
        "{:?}",
        w.mem_calls()
    );

    let w = World::new("cloud-thread-other-account");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Work","since":"{}","milestone":"m2-next","account":"work"}}"#,
            now_minus(1)
        ),
    );
    let out = w.thread_as("main", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: gamma already runs m2-next from the work account: https://claude.ai/code/session_01Work"
    );
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));

    // Once that run landed and wrote its handoff, the main account starts
    // the next milestone.
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(0)));
    let projects = w
        .work
        .read("projects.json")
        .replace(r#""milestone":"m2-next"}"#, r#""milestone":"m3-last"}"#);
    w.work.put("projects.json", &projects);
    w.work.put(
        "roadmap-gamma.md",
        "# roadmap: gamma\n\n- [x] m1-first One\n- [x] m2-next Two\n- [ ] m3-last Three\n",
    );
    w.work.put(
        "handoff-gamma.md",
        "Next: m3-last. m2-next landed at abc1234.\n",
    );
    let out = w.thread_as("main", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        w.mem_calls()
            .iter()
            .any(|c| c.ends_with("--milestone m3-last --project gamma --account main")),
        "{:?}",
        w.mem_calls()
    );
}

/// An environment whose WORKFLOW_ACCOUNT is neither account parks the
/// thread naming the variable, before it writes any record.
#[test]
fn a_thread_with_an_unknown_account_parks_naming_the_variable() {
    let w = World::new("cloud-thread-bad-account");
    let out = w.thread_as("personal", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: this environment's WORKFLOW_ACCOUNT is \"personal\"; set it to work or main in the Project's environment"
    );
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));

    // The variable is this Project's to fix whatever else holds the
    // project, so it is said first.
    cloud_runner(&w, "gamma", "session_01Work", 1);
    let out = w.thread_as("personal", &["gamma", "--cloud", "--print"]);
    assert!(
        stdout(&out).contains("WORKFLOW_ACCOUNT is \"personal\""),
        "{}",
        stdout(&out)
    );
}

/// Both accounts' threads start the same milestone in the same minute: the
/// store keeps one claim, and the other's thread parks naming its account.
#[test]
fn the_thread_that_loses_a_race_with_the_other_account_names_it() {
    let w = World::new("cloud-thread-account-race");
    w.work.put("winner", "session_01Work");
    w.work.put("winner-account", "work");
    let out = w.thread_as("main", &["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: gamma already runs m2-next from the work account: https://claude.ai/code/session_01Work"
    );
}

/// A claim the store has not taken could still lose to another: the
/// thread parks rather than run twice.
#[test]
fn a_thread_whose_claim_never_reached_the_store_parks() {
    let w = World::new("cloud-thread-offline");
    w.work.put("outbox-later", "3");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(
        said.starts_with("parked: the store has not taken"),
        "{said}"
    );
    assert!(said.contains("3 writes still wait"), "{said}");
    assert!(!said.contains("cloud/gamma-m2-next"), "{said}");
}

/// A cloud runner record for `project`, started `hours` ago.
fn cloud_runner(w: &World, project: &str, id: &str, hours: i64) {
    w.runner(
        project,
        &format!(
            r#"{{"machine":"cloud","id":"{id}","since":"{}","milestone":"m2-next"}}"#,
            now_minus(hours)
        ),
    );
}

/// Cloud runs of other projects hold no start back: there is no cap across
/// projects (#0W59FSK2, replacing #RA9XKD8H's two).
#[test]
fn live_or_parked_cloud_runs_elsewhere_hold_no_start_back() {
    let w = World::new("cloud-two-live");
    cloud_runner(&w, "delta", "session_01Other", 1);
    cloud_runner(&w, "shopify_apps", "session_01Parked", 2);
    w.set(
        "shopify_apps",
        &format!(r#""handoff_at":"{}""#, now_minus(1)),
    );
    w.work.put(
        "handoff-shopify_apps.md",
        "parked: t2 waits, question #ABCD1234\n",
    );
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed());
}

#[test]
fn a_project_whose_own_run_is_live_starts_no_second() {
    let w = World::new("cloud-own-live");
    cloud_runner(&w, "gamma", "session_01Mine", 1);
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: gamma already runs m2-next in https://claude.ai/code/session_01Mine"
    );
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));

    // On a machine the print says so on stderr.
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("gamma already has a cloud run: m2-next, session_01Mine"),
        "{}",
        stderr(&out)
    );
}

/// A thread waits for an answer inside its turn and never parks for one,
/// so a thread's park, its 6-hour stop or a moved trunk, means it ended:
/// the next thread takes the milestone up from its handoff. On a machine a
/// parked cloud run still holds its project, as a cloud plan's does.
#[test]
fn a_thread_takes_up_a_milestone_whose_thread_parked() {
    let w = World::new("cloud-parked");
    cloud_runner(&w, "gamma", "session_01Mine", 2);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work
        .put("handoff-gamma.md", "parked: 6-hour stop, t3 half done\n");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed(), "{:?}", w.mem_calls());

    let w = World::new("cloud-parked-machine");
    cloud_runner(&w, "gamma", "session_01Mine", 2);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work.put("handoff-gamma.md", "parked: t2 waits\n");
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("gamma already has a cloud run: m2-next, session_01Mine"),
        "{}",
        stderr(&out)
    );
}

/// A thread's own record in its replica is no claim until the store has
/// it: a claim that did not reach the store, run again, parks again.
#[test]
fn a_thread_whose_own_record_never_reached_the_store_parks_again() {
    let w = World::new("cloud-thread-own-unsent");
    cloud_runner(&w, "gamma", THREAD_SESSION, 0);
    w.work.put("outbox-later", "2");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stdout(&out).starts_with("parked: the store has not taken"),
        "{}",
        stdout(&out)
    );
}

/// A dry run claims nothing, in a thread too.
#[test]
fn a_dry_run_in_a_thread_claims_nothing() {
    let w = World::new("cloud-thread-dry-run");
    let out = w.thread(&["gamma", "--cloud", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("cloud/gamma-m2-next"),
        "{}",
        stdout(&out)
    );
    assert!(
        !w.mem_calls()
            .iter()
            .any(|c| c.contains("set runner") || c.starts_with("sync")),
        "{:?}",
        w.mem_calls()
    );
}

/// A run parked local-only waits for a device to take its branch: a
/// thread starting the milestone again would build it twice.
#[test]
fn a_run_parked_local_only_holds_a_thread_back() {
    let w = World::new("cloud-thread-local-only");
    cloud_runner(&w, "gamma", "session_01Parked", 2);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work.put(
        "handoff-gamma.md",
        "parked: local-only t2: needs the desktop\n",
    );
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("parked: gamma's m2-next is parked local-only in "),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_run_that_ended_landed_or_lapsed_holds_the_project_back_no_longer() {
    // Ended: a handoff after its start that is no park.
    let w = World::new("cloud-ended");
    cloud_runner(&w, "gamma", "session_01Before", 2);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work.put("handoff-gamma.md", "Next: m2-next again\n");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed());

    // Lapsed: no handoff and no tick for 7 hours.
    let w = World::new("cloud-lapsed");
    cloud_runner(&w, "gamma", "session_01Before", 8);
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed());

    // Parked, but 8 hours ago: the 6-hour stop and an hour more.
    let w = World::new("cloud-parked-lapsed");
    cloud_runner(&w, "gamma", "session_01Before", 8);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(2)));
    w.work
        .put("handoff-gamma.md", "parked: 6-hour stop, t3 half done\n");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));

    // Landed: its milestone is ticked.
    let w = World::new("cloud-landed");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Before","since":"{}","milestone":"m1-first"}}"#,
            now_minus(1)
        ),
    );
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed());
}

/// Every run is cut from main: a landed run pushed its work there.
#[test]
fn a_next_run_after_a_landed_cloud_run_is_cut_from_main() {
    let w = World::new("cloud-base");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Before","since":"{}","milestone":"m1-first"}}"#,
            now_minus(3)
        ),
    );
    let prompt = stdout(&w.go(&["gamma", "--cloud", "--print"]));
    assert!(!prompt.contains("cloud/gamma-m1-first"), "{prompt}");
    assert!(prompt.contains("origin/main"), "{prompt}");
}

#[test]
fn a_paused_project_parks_a_thread_even_with_a_parked_handoff() {
    let w = World::new("cloud-paused");
    w.set("gamma", r#""paused":"2026-10-15T09:00:00Z by saiful""#);
    w.work
        .put("handoff-gamma.md", "parked: waiting on #ABCD1234\n");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout(&out).trim(), "parked: gamma is paused");
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(stdout(&out).trim(), "gamma: paused");
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}

/// The line a thread that finds nothing left to run ends its report on.
#[test]
fn a_thread_on_a_done_roadmap_says_done() {
    let w = World::new("cloud-thread-done");
    w.work.put(
        "roadmap-gamma.md",
        "# roadmap: gamma\n\n- [x] m1-first One\n",
    );
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "done: gamma's roadmap is done");
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}

/// A landed cloud run pushed its work to origin's main, so the next local
/// go fast-forwards the trunk from there.
#[test]
fn the_next_local_go_fast_forwards_the_trunk_to_origin_main_after_a_landed_cloud_run() {
    let w = World::new("cloud-ff");
    let tip = w.origin_with_landed_main();
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
        "the trunk's tip is origin main's"
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

/// An app's cloud run lands on its own trunk first: the next local go in the
/// app's worktree takes origin's app/<app>, even when main never got it.
#[test]
fn the_next_local_go_of_an_app_fast_forwards_its_own_trunk() {
    let w = World::new("cloud-ff-app");
    w.origin_with_cloud_branch();
    let dir = w.checkout("gamma");
    git(&dir, &["checkout", "-q", "-b", "app/gamma"]);
    git(&dir, &["push", "-q", "origin", "app/gamma"]);
    w.set("gamma", &format!(r#""worktree":"{}""#, dir.display()));
    let cloud = w.work.at("cloud-clone");
    git(&cloud, &["push", "-q", "origin", "HEAD:app/gamma"]);
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
        head(&dir),
        head(&cloud),
        "the trunk's tip is origin app/gamma's"
    );
}

/// A cloud run that met a local-only task parks for a device with no
/// question: the next local go takes its branch and runs the rest.
#[test]
fn a_run_parked_local_only_is_taken_over_by_the_next_local_go() {
    let w = World::new("cloud-local-only");
    let tip = w.origin_with_cloud_branch_for("m2-next");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"cloud","id":"session_01Parked","since":"{}","milestone":"m2-next"}}"#,
            now_minus(2)
        ),
    );
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work.put(
        "handoff-gamma.md",
        "parked: local-only t2: needs the desktop to walk the popup\n",
    );
    let out = w.go_local(&["gamma"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        head(&w.checkout("gamma")),
        tip,
        "the trunk takes the cloud branch"
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
    w.origin_with_landed_main();
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

/// An app of a monorepo works on its own trunk, app/<app>: once origin has
/// that branch, its cloud milestone is cut from there, and the worktree a
/// machine records for it refuses nothing.
#[test]
fn an_app_with_a_trunk_of_its_own_is_cut_from_it() {
    let w = World::new("cloud-app-trunk");
    w.origin_with_cloud_branch();
    let dir = w.checkout("gamma");
    git(&dir, &["push", "-q", "origin", "main:app/gamma"]);
    git(&dir, &["fetch", "-q", "origin"]);
    w.set("gamma", &format!(r#""worktree":"{}""#, dir.display()));
    for out in [
        w.go(&["gamma", "--cloud", "--print"]),
        w.thread(&["gamma", "--cloud", "--print"]),
    ] {
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let prompt = stdout(&out);
        assert!(prompt.contains("from origin/app/gamma"), "{prompt}");
        assert!(!prompt.contains("origin/main."), "{prompt}");
    }
}

/// A thread stands in its repo's clone, which mem files under the root
/// project, and names the project of its start line: a root and its child,
/// as ebdfy and ebdfy-ui are, both start from one Project, each in its own
/// folder.
#[test]
fn a_root_and_its_child_both_start_from_the_repos_clone() {
    let w = World::new("cloud-root-and-child");
    let clone = w.monorepo("ebdfy", &[("ebdfy-ui", "packages/ui")]);
    let out = w.thread_in(&clone, &["ebdfy-ui", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let prompt = stdout(&out);
    assert!(prompt.contains("cloud/ebdfy-ui-m2-next"), "{prompt}");
    assert!(
        prompt.contains(&format!("`cd {}/packages/ui`", clone.display())),
        "{prompt}"
    );
    assert!(w.claimed_for("ebdfy-ui"), "{:?}", w.mem_calls());

    let out = w.thread_in(&clone, &["ebdfy", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains(&format!("`cd {}`", clone.display())),
        "{}",
        stdout(&out)
    );
    assert!(w.claimed_for("ebdfy"), "{:?}", w.mem_calls());
}

/// Two apps of one repo run at once; one app still never runs twice.
#[test]
fn two_apps_of_one_repo_run_at_once_but_each_only_once() {
    let w = World::new("cloud-two-apps");
    let clone = w.monorepo(
        "cloud_walk",
        &[("walk-a", "apps/walk-a"), ("walk-b", "apps/walk-b")],
    );
    w.runner(
        "walk-a",
        &format!(
            r#"{{"machine":"cloud","id":"session_01First","since":"{}","milestone":"m2-next"}}"#,
            now_minus(1)
        ),
    );
    let out = w.thread_in(&clone, &["walk-b", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains(&format!("`cd {}/apps/walk-b`", clone.display())),
        "{}",
        stdout(&out)
    );
    assert!(w.claimed_for("walk-b"), "{:?}", w.mem_calls());

    let out = w.thread_in(&clone, &["walk-a", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: walk-a already runs m2-next in https://claude.ai/code/session_01First"
    );
    assert!(!w.claimed_for("walk-a"), "{:?}", w.mem_calls());
}

/// A clone mem once recorded elsewhere on this machine is not the clone the
/// thread stands in: the thread's own folder wins over a listed one.
#[test]
fn the_clone_a_thread_stands_in_beats_a_checkout_mem_listed() {
    let w = World::new("cloud-own-clone");
    let clone = w.monorepo("cloud_walk", &[("walk-a", "apps/walk-a")]);
    let stale = w.work.mkdir("old-clone/apps/walk-a");
    let projects = w.work.read("projects.json").replacen(
        r#""checkouts":[],"subdir":"apps/walk-a""#,
        &format!(
            r#""checkouts":["{}"],"subdir":"apps/walk-a""#,
            stale.display()
        ),
        1,
    );
    w.work.put("projects.json", &projects);
    let out = w.thread_in(&clone, &["walk-a", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains(&format!("`cd {}/apps/walk-a`", clone.display())),
        "{}",
        stdout(&out)
    );
}

/// A new app's folder may exist only on its own branch, not on the main the
/// thread stands on: its trunk is still app/<app>.
#[test]
fn a_new_apps_trunk_is_found_before_its_folder_exists() {
    let w = World::new("cloud-new-app");
    let clone = w.monorepo("cloud_walk", &[("walk-a", "apps/walk-a")]);
    let origin = w.work.at("mono-origin.git");
    git(
        w.work.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        &clone,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    commit(&clone, "start");
    git(&clone, &["push", "-q", "origin", "main", "main:app/walk-a"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert!(!clone.join("apps/walk-a").exists());
    let out = w.thread_in(&clone, &["walk-a", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("from origin/app/walk-a"),
        "{}",
        stdout(&out)
    );
}

/// On a machine, an app worktree on an app/<app> origin lacks holds work a
/// cloud run would never see.
#[test]
fn an_app_branch_origin_lacks_keeps_the_cloud_out() {
    let w = World::new("cloud-app-unpushed");
    w.origin_with_cloud_branch();
    let dir = w.checkout("gamma");
    git(&dir, &["checkout", "-q", "-b", "app/gamma"]);
    w.set("gamma", &format!(r#""worktree":"{}""#, dir.display()));
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("push app/gamma first"),
        "{}",
        stderr(&out)
    );
}

/// Another repo's clone is no folder for an app: its subdir means nothing
/// there.
#[test]
fn an_app_of_another_repo_finds_no_folder_in_this_clone() {
    let w = World::new("cloud-other-repo");
    w.monorepo("cloud_walk", &[("walk-a", "apps/walk-a")]);
    let other = w.monorepo("lathe", &[]);
    let out = w.thread_in(&other, &["walk-a", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: walk-a has no checkout on this machine"
    );
}

#[test]
fn a_project_with_no_remote_stays_off_the_cloud() {
    let w = World::new("cloud-no-remote");
    let projects =
        w.work
            .read("projects.json")
            .replacen(r#","remote":"github.com/me/gamma""#, "", 1);
    w.work.put("projects.json", &projects);
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert_eq!(stdout(&out).trim(), "parked: gamma has no git remote");
}

/// On a machine a device's runner keeps the cloud out as a local go would.
#[test]
fn a_device_orchestrator_on_another_machine_keeps_the_cloud_out() {
    let w = World::new("cloud-device-runner");
    w.runner(
        "gamma",
        r#"{"machine":"nuc","id":"gamma-m2-next","since":"2026-10-15T07:00:00Z"}"#,
    );
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("nuc"), "{}", stderr(&out));
}

/// A thread cannot ask a device's amx, so it weighs a device's run by its
/// record: under 7 hours with no handoff since, or a park, it is live.
#[test]
fn a_thread_weighs_a_device_run_by_its_record() {
    let device = |w: &World, hours: i64| {
        w.runner(
            "gamma",
            &format!(
                r#"{{"machine":"nuc","id":"gamma-m2-next","since":"{}"}}"#,
                now_minus(hours)
            ),
        );
    };
    let w = World::new("cloud-thread-device-live");
    device(&w, 2);
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "parked: gamma already runs on nuc (gamma-m2-next)"
    );

    let w = World::new("cloud-thread-device-parked");
    device(&w, 3);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work
        .put("handoff-gamma.md", "parked: t3 waits on #ABCD1234\n");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));

    let w = World::new("cloud-thread-device-landed");
    device(&w, 3);
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work
        .put("handoff-gamma.md", "Next: m2-next. m1-first landed.\n");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed());

    let w = World::new("cloud-thread-device-old");
    device(&w, 8);
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.claimed());
}

/// A device orchestrator that parked on a question has stopped its own
/// session, so amx shows nothing running, but the hub resumes that session
/// when Saiful answers: a cloud start would run the milestone twice.
#[test]
fn a_milestone_parked_on_this_machine_starts_no_cloud_run() {
    let w = World::new("cloud-device-parked");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"mini","id":"gamma-m2-next","since":"{}"}}"#,
            now_minus(3)
        ),
    );
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(1)));
    w.work.put(
        "handoff-gamma.md",
        "parked: m2-next t3 waits on question #ABCD1234\n",
    );
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("gamma's m2-next is parked on mini (gamma-m2-next)"),
        "{}",
        stderr(&out)
    );

    // A park from before this runner started is an older run's.
    let w = World::new("cloud-device-old-park");
    w.runner(
        "gamma",
        &format!(
            r#"{{"machine":"mini","id":"gamma-m2-next","since":"{}"}}"#,
            now_minus(1)
        ),
    );
    w.set("gamma", &format!(r#""handoff_at":"{}""#, now_minus(3)));
    w.work.put("handoff-gamma.md", "parked: an older run\n");
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert!(!stderr(&out).contains("is parked on"), "{}", stderr(&out));
}

#[test]
fn a_cloud_run_takes_the_next_milestone_only() {
    let w = World::new("cloud-later-milestone");
    let out = w.thread(&["gamma", "--cloud", "--print", "--milestone", "m3-last"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("parked: "), "{}", stdout(&out));
    assert!(stdout(&out).contains("m2-next"), "{}", stdout(&out));
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}

#[test]
fn a_cloud_run_that_landed_the_last_milestone_still_reaches_the_trunk() {
    let w = World::new("cloud-ff-last");
    let tip = w.origin_with_landed_main();
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

/// On a machine the print still checks that origin holds what the trunk
/// does, since the thread clones origin.
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

    git(&checkout, &["push", "-q", "origin", "main"]);
    let out = w.go(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

impl World {
    /// A plain `workflow go` with both claude and amx faked, so a run sent
    /// the wrong way shows as the other one's log, never a real session.
    fn go_plain(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_CLAUDE", self.work.at("claude"))
            .env("WORKFLOW_AMX", self.work.at("amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env("XDG_STATE_HOME", self.work.at("state"))
            .env("WORKFLOW_MACHINE", "mini")
            .env_remove("AMX_ID")
            .current_dir(self.work.path())
            .output()
            .unwrap()
    }
}

/// An old `cloud` key carries nothing any more: a plain go starts the
/// orchestrator here and pushes nothing.
#[test]
fn a_plain_go_of_a_project_with_an_old_cloud_key_stays_local() {
    let w = World::new("cloud-old-key");
    w.set("gamma", r#""cloud":"mini""#);
    w.origin_with_cloud_branch_for("m1-first");
    let checkout = w.checkout("gamma");
    commit(&checkout, "landed on the device");
    let out = w.go_plain(&["gamma"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(w.amx_started(), "{}", stderr(&out));
    assert!(!w.claude_ran(), "it went to the cloud");
    let origin = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(["rev-parse", "origin/main"])
        .current_dir(&checkout)
        .output()
        .unwrap();
    assert_ne!(
        String::from_utf8_lossy(&origin.stdout).trim(),
        head(&checkout)
    );

    let w = World::new("cloud-old-key-dry");
    w.set("gamma", r#""cloud":"mini""#);
    let out = w.go_plain(&["gamma", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("amx"), "{}", stdout(&out));
    assert!(
        !stdout(&out).contains("cloud/gamma-m2-next"),
        "{}",
        stdout(&out)
    );
}

/// A cloud dry run prints the prompt and records and starts nothing
/// (#6CWMTDXX: it once pushed main and started a real session).
#[test]
fn a_cloud_dry_run_records_and_starts_nothing() {
    let w = World::new("cloud-dry-run");
    let out = w.go_plain(&["gamma", "--cloud", "--dry-run"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("cloud/gamma-m2-next"),
        "{}",
        stdout(&out)
    );
    assert!(!w.claude_ran());
    assert!(!w.amx_started());
    assert!(
        !w.mem_calls().iter().any(|c| c.contains("set runner")),
        "{:?}",
        w.mem_calls()
    );
}

/// A thread's replica can be the copy its environment baked, when the
/// first mem's store sync failed: a stale roadmap there read as done once
/// (cloud_walk w5). A thread decides nothing until a sync has performed.
#[test]
fn a_thread_whose_replica_never_reached_the_store_decides_nothing() {
    let w = World::new("cloud-thread-stale");
    w.work.put(
        "roadmap-gamma.md",
        "# roadmap: gamma\n\n- [x] m1-first One\n",
    );
    w.work.put("not-performed", "");
    let out = w.thread(&["gamma", "--cloud", "--print"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    let said = stdout(&out);
    assert!(
        said.starts_with("parked: mem in this thread has not reached the store"),
        "{said}"
    );
    assert!(!said.contains("done:"), "{said}");
    assert!(!w.mem_calls().iter().any(|c| c.contains("set runner")));
}
