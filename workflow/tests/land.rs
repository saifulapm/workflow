//! `workflow land` through the real binary: a thread of a Claude Project
//! lands its milestone branch on origin's main. mem is a script
//! (`$WORKFLOW_MEM`); origin is a bare repository in the scratch folder.

#[path = "../src/scratch.rs"]
mod scratch;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use scratch::Scratch;

/// What every fake `mem sync --json` prints through (tests/fake_mem_sync.rs).
const SYNC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mem-sync.sh");
const BRANCH: &str = "cloud/gamma-m2-next";

/// An origin whose main holds `start`, and the thread's clone of it on the
/// milestone branch, one commit ahead with `work.txt`. The fake mem answers
/// for gamma from the scratch files and logs each call to `mem.log`.
struct World {
    work: Scratch,
}

impl World {
    fn new(tag: &str) -> World {
        World::build(tag, None)
    }

    /// An app of a monorepo, gamma in apps/gamma, whose trunk is origin's
    /// app/gamma, cut from main: the milestone's `work.txt` is in its
    /// folder, where the test command runs.
    fn app(tag: &str) -> World {
        World::build(tag, Some("apps/gamma"))
    }

    fn build(tag: &str, subdir: Option<&str>) -> World {
        let work = Scratch::new(tag);
        work.put(
            "gitconfig",
            "[user]\n\tname = t\n\temail = t@t\n[init]\n\tdefaultBranch = main\n",
        );
        let w = World { work };
        let origin = w.work.at("origin.git");
        w.git(
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
        let seed = w.work.at("seed");
        w.git(
            w.work.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                seed.to_str().unwrap(),
            ],
        );
        w.work.put("seed/shared.txt", "one\n");
        w.git(&seed, &["add", "."]);
        w.git(&seed, &["commit", "-q", "-m", "start"]);
        w.git(&seed, &["push", "-q", "origin", "HEAD:main"]);
        if let Some(sub) = subdir {
            w.git(&seed, &["push", "-q", "origin", "HEAD:app/gamma"]);
            w.work.put("subdir", sub);
        }
        let clone = w.clone_dir();
        w.git(
            w.work.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        w.git(&clone, &["checkout", "-q", "-b", BRANCH]);
        let folder = subdir.map_or("thread".to_string(), |sub| format!("thread/{sub}"));
        w.work.put(&format!("{folder}/work.txt"), "the milestone\n");
        w.git(&clone, &["add", "."]);
        w.git(&clone, &["commit", "-q", "-m", "Build the milestone"]);
        w.work.put(
            "roadmap.md",
            "# roadmap: gamma\n\n- [x] m1-first One\n- [ ] m2-next Two\n- [ ] m3-last Three\n",
        );
        w.work.put(
            "projects.json",
            r#"{"projects":[{"name":"gamma","remote":"github.com/me/gamma","checkouts":[],"roadmap_status":"approved","milestone":"m2-next"}]}"#,
        );
        w.verify("test -f work.txt");
        w.work.script(
            "mem",
            &format!(
                r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/mem.log"
case "$1 $2" in
"project current")
  sub=""
  [ -f "$dir/subdir" ] && sub=",\"subdir\":\"$(cat "$dir/subdir")\""
  ci=""
  [ -f "$dir/land_ci" ] && ci=",\"land_ci\":\"$(cat "$dir/land_ci")\""
  printf '{{"id":"01G","name":"gamma","root":"{root}"%s%s,"verify":%s}}\n' "$sub" "$ci" "$(cat "$dir/verify.json")" ;;
"projects --json") cat "$dir/projects.json" ;;
"roadmap --tick") sed -i "s/- \[ \] $3 /- [x] $3 /" "$dir/roadmap.md" ;;
"roadmap --project") cat "$dir/roadmap.md" ;;
"sync --json")
  n=0
  [ -f "$dir/outbox" ] && n=$(cat "$dir/outbox")
  sh {sync} "$n" ;;
"share export")
  if [ -f "$dir/outbox" ]; then
    echo '{{"writes":2}}' >.mem-outbox.json
    git add .mem-outbox.json
  fi ;;
esac
"#,
                root = clone.display(),
                sync = SYNC,
            ),
        );
        w.work.script("gh", &format!(r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/gh.log"
if [ -f "$dir/gh-refused" ]; then
  echo "gh: Resource not accessible by integration (HTTP 403)" >&2
  exit 1
fi
[ "$1" = api ] || exit 1
case "$2" in
repos/me/gamma/actions/workflows/ci.yml/runs\?*)
  sha=${{2#*head_sha=}}; sha=${{sha%%&*}}
  if [ -f "$dir/move-during-ci" ]; then
    rm "$dir/move-during-ci"
    (cd "$dir/seed" && git pull -q origin main && echo moved >moved.txt && git add moved.txt \
      && git commit -q -m "Move during CI" && git push -q origin HEAD:main) >&2
  fi
  # GitHub starts a run for a sha pushed to the ci/ ref, in the mode the
  # test chose, and keeps the result it ran with.
  tip=$(git -C "{origin}" rev-parse -q --verify refs/heads/ci/gamma-m2-next)
  if [ ! -f "$dir/runs/$sha" ] && [ "$tip" = "$sha" ]; then
    mkdir -p "$dir/runs" && cp "$dir/ci-mode" "$dir/runs/$sha"
  fi
  mode=none
  [ -f "$dir/runs/$sha" ] && mode=$(cat "$dir/runs/$sha")
  age=0
  [ -f "$dir/run-age" ] && age=$(cat "$dir/run-age")
  at=$(date -u -d "-$age seconds" +%Y-%m-%dT%H:%M:%SZ)
  # A pull request's green run of the same sha, on another branch, is newer.
  other='{{"id":90,"head_sha":"'$sha'","head_branch":"cloud/gamma-m2-next","event":"pull_request","status":"completed","conclusion":"success"}}'
  case "$mode" in
  none) echo '{{"total_count":0,"workflow_runs":[]}}' ;;
  green) echo '{{"total_count":2,"workflow_runs":['$other',{{"id":7,"head_sha":"'$sha'","head_branch":"ci/gamma-m2-next","event":"push","status":"completed","conclusion":"success"}}]}}' ;;
  red) echo '{{"total_count":2,"workflow_runs":['$other',{{"id":8,"head_sha":"'$sha'","head_branch":"ci/gamma-m2-next","event":"push","status":"completed","conclusion":"failure"}}]}}' ;;
  running) echo '{{"total_count":1,"workflow_runs":[{{"id":9,"head_sha":"'$sha'","head_branch":"ci/gamma-m2-next","event":"push","status":"in_progress","conclusion":null,"created_at":"'$at'"}}]}}' ;;
  esac ;;
repos/me/gamma/actions/runs/8/jobs*)
  echo '{{"total_count":3,"jobs":[{{"name":"php 8.4 sqlite","status":"completed","conclusion":"success"}},{{"name":"php 8.4 postgres:15","status":"completed","conclusion":"failure"}},{{"name":"audit","status":"completed","conclusion":"skipped"}}]}}' ;;
*) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac
"#, origin = origin.display()));
        w
    }

    /// The project waits on ci.yml, whose run on the ci/ ref ends `mode`:
    /// green, red, running, or none for no run at all.
    fn land_ci(&self, mode: &str) {
        self.work.put("land_ci", "ci.yml");
        self.work.put("ci-mode", mode);
    }

    fn gh_log(&self) -> String {
        if self.work.has("gh.log") {
            self.work.read("gh.log")
        } else {
            String::new()
        }
    }

    fn short_head(&self) -> String {
        self.git(&self.clone_dir(), &["rev-parse", "--short", "HEAD"])
    }

    fn clone_dir(&self) -> PathBuf {
        self.work.at("thread")
    }

    /// The project's test command, as mem's `verify` key holds it.
    fn verify(&self, command: &str) {
        self.work.put(
            "verify.json",
            &serde_json::Value::String(command.to_string()).to_string(),
        );
    }

    /// The fixture's own git, with no global hooks or config.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.work.at("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Another thread's work on origin's main: `file` holding `text`.
    fn trunk_moves(&self, file: &str, text: &str) {
        let seed = self.work.at("seed");
        self.git(&seed, &["pull", "-q", "origin", "main"]);
        self.work.put(&format!("seed/{file}"), text);
        self.git(&seed, &["add", "."]);
        self.git(&seed, &["commit", "-q", "-m", &format!("Change {file}")]);
        self.git(&seed, &["push", "-q", "origin", "HEAD:main"]);
    }

    fn origin_main(&self) -> String {
        self.origin_ref("main")
    }

    fn origin_ref(&self, name: &str) -> String {
        self.git(&self.work.at("origin.git"), &["rev-parse", name])
    }

    fn origin_log(&self, name: &str) -> String {
        self.git(&self.work.at("origin.git"), &["log", "--format=%s", name])
    }

    fn land(&self) -> Output {
        self.land_minute("20")
    }

    /// land with a minute of `ms` milliseconds.
    fn land_minute(&self, ms: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_workflow"))
            .arg("land")
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_GH", self.work.at("gh"))
            .env("WORKFLOW_CI_MINUTE_MS", ms)
            .env("GIT_CONFIG_GLOBAL", self.work.at("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("CLAUDE_CODE_REMOTE_SESSION_ID", "cse_01Thread")
            .env_remove("WORKFLOW_ALLOW_PUSH")
            .current_dir(self.clone_dir())
            .output()
            .unwrap()
    }

    fn ticked(&self) -> bool {
        self.work.has("mem.log")
            && self
                .work
                .read("mem.log")
                .lines()
                .any(|l| l == "roadmap --tick m2-next --project gamma")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn last_line(out: &Output) -> String {
    stdout(out).lines().last().unwrap_or("").to_string()
}

#[test]
fn a_milestone_lands_on_a_trunk_that_moved_and_names_the_next() {
    let w = World::new("land-moved");
    w.trunk_moves("other.txt", "another thread\n");
    let moved = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(last_line(&out), "next: Run the next milestone of gamma.");
    let clone = w.clone_dir();
    assert_eq!(w.origin_main(), w.git(&clone, &["rev-parse", "HEAD"]));
    assert_eq!(
        w.git(&clone, &["rev-parse", "HEAD~1"]),
        moved,
        "the milestone sits on the moved trunk"
    );
    assert!(w.ticked(), "{}", w.work.read("mem.log"));
}

#[test]
fn the_last_milestone_says_the_roadmap_is_done() {
    let w = World::new("land-last");
    w.work.put(
        "roadmap.md",
        "# roadmap: gamma\n\n- [x] m1-first One\n- [ ] m2-next Two\n",
    );
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(last_line(&out), "done: gamma's roadmap is done");
}

#[test]
fn a_paused_project_parks_after_its_landing() {
    let w = World::new("land-paused");
    let projects = w.work.read("projects.json").replacen(
        r#""name":"gamma","#,
        r#""name":"gamma","paused":"2026-11-09T05:00:00Z by saiful","#,
        1,
    );
    w.work.put("projects.json", &projects);
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(w.ticked());
    assert_eq!(last_line(&out), "parked: gamma is paused");
}

#[test]
fn a_rebase_conflict_pushes_nothing_and_parks_on_the_trunk() {
    let w = World::new("land-conflict");
    let clone = w.clone_dir();
    w.work.put("thread/shared.txt", "the milestone's\n");
    w.git(&clone, &["commit", "-q", "-am", "Change shared.txt here"]);
    let mine = w.git(&clone, &["rev-parse", "HEAD"]);
    w.trunk_moves("shared.txt", "the trunk's\n");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line = last_line(&out);
    assert!(line.starts_with("parked: trunk moved:"), "{line}");
    assert!(line.contains("shared.txt"), "{line}");
    assert_eq!(w.origin_main(), before);
    assert_eq!(
        w.git(&clone, &["rev-parse", "HEAD"]),
        mine,
        "the rebase was undone"
    );
    assert_eq!(w.git(&clone, &["branch", "--show-current"]), BRANCH);
    assert!(!w.ticked());
}

#[test]
fn a_red_test_run_after_the_rebase_pushes_nothing() {
    let w = World::new("land-red");
    w.verify("echo two tests failed; exit 1");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("two tests failed"),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("nothing was pushed to main"),
        "{}",
        stderr(&out)
    );
    assert_eq!(w.origin_main(), before);
    assert!(!w.ticked());
}

/// The trunk moves while the tests run: the push is refused, never forced,
/// and the landing starts over on the new trunk.
#[test]
fn a_trunk_that_moves_during_the_tests_is_rebased_onto_again_never_forced() {
    let w = World::new("land-race");
    let seed = w.work.at("seed");
    // The first run of the tests pushes another thread's commit to main.
    w.work.script(
        "move-once",
        &format!(
            r#"
[ -f "{flag}" ] && exit 0
touch "{flag}"
cd "{seed}" || exit 1
echo raced >raced.txt
git add raced.txt
git commit -q -m "Race the landing"
git push -q origin HEAD:main
"#,
            flag = w.work.at("moved").display(),
            seed = seed.display()
        ),
    );
    w.verify(&format!(
        "test -f work.txt && {}",
        w.work.at("move-once").display()
    ));
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let clone = w.clone_dir();
    let origin = w.work.at("origin.git");
    let raced = w.git(&origin, &["log", "--format=%s", "main"]);
    assert!(raced.contains("Race the landing"), "{raced}");
    assert!(raced.contains("Build the milestone"), "{raced}");
    assert_eq!(w.origin_main(), w.git(&clone, &["rev-parse", "HEAD"]));
}

#[test]
fn it_lands_only_a_clean_milestone_branch() {
    let w = World::new("land-dirty");
    w.work.put("thread/loose.txt", "not committed\n");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains("commit"), "{}", stderr(&out));
    assert_eq!(w.origin_main(), before);

    let w = World::new("land-on-main");
    let clone = w.clone_dir();
    w.git(&clone, &["checkout", "-q", "main"]);
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
    assert!(!w.ticked());
}

/// A commit that got past the hooks with a Claude line never reaches a
/// trunk: the branch parks naming it, and nothing is pushed.
#[test]
fn a_commit_with_a_claude_line_parks_the_landing() {
    for (tag, line) in [
        (
            "land-coauthor",
            "Co-Authored-By: Claude <noreply@anthropic.com>",
        ),
        (
            "land-session",
            "Claude-Session: https://claude.ai/code/session_01x",
        ),
    ] {
        let w = World::new(tag);
        let clone = w.clone_dir();
        w.work.put("thread/more.txt", "more\n");
        w.git(&clone, &["add", "."]);
        w.git(
            &clone,
            &["commit", "-q", "-m", &format!("Add more\n\n{line}")],
        );
        let sha = w.git(&clone, &["rev-parse", "--short", "HEAD"]);
        w.work.put("thread/last.txt", "last\n");
        w.git(&clone, &["add", "."]);
        w.git(&clone, &["commit", "-q", "-m", "Add the last"]);
        let before = w.origin_main();
        let out = w.land();
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        let field = line.split(':').next().unwrap();
        assert_eq!(
            last_line(&out),
            format!("parked: commit {sha} carries a {field} line")
        );
        assert_eq!(w.origin_main(), before);
        assert!(w.git(&clone, &["ls-remote", "origin", BRANCH]).is_empty());
        assert!(!w.ticked());
    }
}

/// A thread whose environment predates the kit's identity commits as
/// Claude with no trailer at all: such a commit never reaches a trunk
/// either, whether Claude wrote it or only committed it.
#[test]
fn a_commit_by_claudes_identity_parks_the_landing() {
    for (tag, who, role) in [
        ("land-claude-author", "AUTHOR", "authored"),
        ("land-claude-committer", "COMMITTER", "committed"),
    ] {
        let w = World::new(tag);
        let clone = w.clone_dir();
        w.work.put("thread/more.txt", "more\n");
        w.git(&clone, &["add", "."]);
        let out = Command::new("git")
            .args(["commit", "-q", "-m", "Add more"])
            .env("GIT_CONFIG_GLOBAL", w.work.at("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(format!("GIT_{who}_NAME"), "Claude")
            .env(format!("GIT_{who}_EMAIL"), "noreply@anthropic.com")
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let sha = w.git(&clone, &["rev-parse", "--short", "HEAD"]);
        let before = w.origin_main();
        let out = w.land();
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        assert_eq!(
            last_line(&out),
            format!("parked: commit {sha} is {role} by Claude <noreply@anthropic.com>")
        );
        assert_eq!(w.origin_main(), before);
        assert!(!w.ticked());
    }
}

/// Writes the store has not taken leave with the trunk: the next machine
/// that reads the project sends them.
#[test]
fn writes_the_store_has_not_taken_land_on_the_trunk_too() {
    let w = World::new("land-outbox");
    w.work.put("outbox", "2");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let origin = w.work.at("origin.git");
    let files = w.git(&origin, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains(".mem-outbox.json"), "{files}");
    assert_eq!(last_line(&out), "next: Run the next milestone of gamma.");
}

/// The outbox file at the top of the checkout, as `mem share export` writes
/// it for `machine`.
fn outbox_of(machine: &str) -> String {
    format!("{{\n  \"machine\": \"{machine}\",\n  \"writes\": []\n}}\n")
}

/// Every thread commits its unsent writes to the one outbox file, so two
/// projects of one repo both carry one. A clash there is no reason to park:
/// the trunk's copy stays, for the next mem to import, and the thread's own
/// writes wait in its outbox for the export after the landing.
#[test]
fn another_threads_outbox_on_main_never_stops_a_landing() {
    let w = World::new("land-outbox-clash");
    let clone = w.clone_dir();
    w.work.put("thread/.mem-outbox.json", &outbox_of("vm-b"));
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "share outbox"]);
    w.trunk_moves(".mem-outbox.json", &outbox_of("vm-a"));
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.origin_log("main").contains("Build the milestone"));
    let origin = w.work.at("origin.git");
    assert_eq!(
        w.git(&origin, &["show", "main:.mem-outbox.json"]),
        outbox_of("vm-a").trim_end()
    );
}

/// A sibling's import took the outbox file off main while this thread's
/// branch changed it: the landing goes on without it.
#[test]
fn an_outbox_main_took_out_never_stops_a_landing() {
    let w = World::new("land-outbox-gone");
    let clone = w.clone_dir();
    w.trunk_moves(".mem-outbox.json", &outbox_of("vm-a"));
    w.git(&clone, &["pull", "-q", "--rebase", "origin", "main"]);
    w.work.put("thread/.mem-outbox.json", &outbox_of("vm-b"));
    w.git(&clone, &["commit", "-q", "-am", "share outbox"]);
    let seed = w.work.at("seed");
    w.git(&seed, &["rm", "-q", ".mem-outbox.json"]);
    w.git(&seed, &["commit", "-q", "-m", "Import the outbox"]);
    w.git(&seed, &["push", "-q", "origin", "HEAD:main"]);
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.origin_log("main").contains("Build the milestone"));
}

/// The same clash when an app moves onto a main a sibling's thread left
/// its outbox on.
#[test]
fn a_siblings_outbox_never_stops_an_app_moving_onto_main() {
    let w = World::app("land-app-outbox-clash");
    let clone = w.clone_dir();
    w.work.put("thread/.mem-outbox.json", &outbox_of("vm-b"));
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "share outbox"]);
    w.trunk_moves(".mem-outbox.json", &outbox_of("vm-a"));
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(w.origin_log("main").contains("Build the milestone"));
    assert_eq!(w.origin_ref("app/gamma"), w.origin_main());
}

/// Origin refuses main for a reason of its own, a protected branch: the
/// milestone stays on app/gamma and the thread still gets its last line.
#[test]
fn a_main_origin_refuses_parks_the_move_with_a_line_to_end_on() {
    let w = World::app("land-app-refused");
    w.trunk_moves("apps/delta/other.txt", "another app\n");
    let hook = w.work.at("origin.git/hooks/pre-receive");
    std::fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do [ \"$ref\" = refs/heads/main ] && exit 1; done\nexit 0\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line = last_line(&out);
    assert!(line.starts_with("parked: main moved: gamma"), "{line}");
    assert!(w.origin_log("app/gamma").contains("Build the milestone"));
    assert!(!w.origin_log("main").contains("Build the milestone"));
}

/// The writes an app's landing carries go to both trunks that took it.
#[test]
fn an_apps_unsent_writes_land_on_its_trunk_and_on_main() {
    let w = World::app("land-app-carry");
    w.work.put("outbox", "2");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let origin = w.work.at("origin.git");
    for trunk in ["main", "app/gamma"] {
        let files = w.git(&origin, &["ls-tree", "--name-only", trunk]);
        assert!(files.contains(".mem-outbox.json"), "{trunk}: {files}");
    }
    assert_eq!(w.origin_ref("app/gamma"), w.origin_main());
}

/// A rebase rewrites the milestone branch, which every task already pushed:
/// after a red run there, the thread's next push of the branch, its fix or
/// its park, must still go through without a force.
#[test]
fn a_red_run_after_a_rebase_leaves_the_branch_pushable() {
    let w = World::new("land-red-moved");
    let clone = w.clone_dir();
    w.git(&clone, &["push", "-q", "origin", BRANCH]);
    w.trunk_moves("other.txt", "another thread\n");
    w.verify("exit 1");
    let out = w.land();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    w.work.put("thread/fix.txt", "the fix\n");
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "Fix the tests"]);
    w.git(&clone, &["push", "-q", "origin", BRANCH]);
    let origin = w.work.at("origin.git");
    assert_eq!(
        w.git(&origin, &["rev-parse", BRANCH]),
        w.git(&clone, &["rev-parse", "HEAD"])
    );
}

/// A push refused for any reason but a moved trunk fails at once, saying
/// why, rather than running the tests three times and blaming the trunk.
#[test]
fn a_push_refused_on_a_trunk_that_did_not_move_fails_at_once() {
    let w = World::new("land-refused");
    w.work.script(
        "origin.git/hooks/pre-receive",
        "#!/bin/sh\necho 'main is protected' >&2\nexit 1\n",
    );
    w.work.put("runs", "");
    w.verify(&format!("echo run >>{}", w.work.at("runs").display()));
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("main is protected"),
        "{}",
        stderr(&out)
    );
    assert!(!stdout(&out).contains("trunk moved"), "{}", stdout(&out));
    assert_eq!(w.work.read("runs").lines().count(), 1);
    assert_eq!(w.origin_main(), before);
    assert!(!w.ticked());
}

/// The rebased branch goes over origin's copy only when that copy holds
/// nothing the thread lacks: another account's commits there are kept.
#[test]
fn a_branch_with_commits_the_thread_lacks_is_not_overwritten() {
    let w = World::new("land-branch-ahead");
    let clone = w.clone_dir();
    w.git(&clone, &["push", "-q", "origin", BRANCH]);
    let other = w.work.at("other");
    let origin = w.work.at("origin.git");
    w.git(
        w.work.path(),
        &[
            "clone",
            "-q",
            "-b",
            BRANCH,
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    w.work.put("other/theirs.txt", "the other account's\n");
    w.git(&other, &["add", "."]);
    w.git(
        &other,
        &["commit", "-q", "-m", "Work from the other account"],
    );
    w.git(&other, &["push", "-q", "origin", BRANCH]);
    let theirs = w.git(&origin, &["rev-parse", BRANCH]);
    w.trunk_moves("other.txt", "another thread\n");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("commits this checkout lacks"),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("nothing was pushed to main"),
        "{}",
        stderr(&out)
    );
    assert_eq!(w.git(&origin, &["rev-parse", BRANCH]), theirs);
    assert_eq!(w.origin_main(), before);
}

/// An app lands on its own trunk, app/gamma, and then moves onto main:
/// both end on the one commit that holds the milestone.
#[test]
fn an_app_lands_on_its_own_trunk_then_moves_onto_main() {
    let w = World::app("land-app");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(last_line(&out), "next: Run the next milestone of gamma.");
    let head = w.git(&w.clone_dir(), &["rev-parse", "HEAD"]);
    assert_eq!(w.origin_ref("app/gamma"), head);
    assert_eq!(w.origin_main(), head);
    assert!(w.origin_log("main").contains("Build the milestone"));
    assert!(w.ticked(), "{}", w.work.read("mem.log"));
}

/// Another app moved main since app/gamma was cut: gamma's work moves onto
/// that main, and app/gamma follows it.
#[test]
fn an_app_moves_onto_a_main_another_app_moved() {
    let w = World::app("land-app-moved");
    w.trunk_moves("apps/delta/other.txt", "another app\n");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let log = w.origin_log("main");
    assert!(log.contains("Change apps/delta/other.txt"), "{log}");
    assert!(log.contains("Build the milestone"), "{log}");
    assert_eq!(w.origin_ref("app/gamma"), w.origin_main());
}

/// A sibling app landed first, on its own trunk and on main: gamma's landing
/// keeps all of it on main and leaves the sibling's trunk alone.
#[test]
fn an_app_landing_after_a_sibling_loses_nothing_of_it() {
    let w = World::app("land-app-sibling");
    w.trunk_moves("apps/delta/one.txt", "delta's first\n");
    w.trunk_moves("apps/delta/two.txt", "delta's second\n");
    let seed = w.work.at("seed");
    w.git(&seed, &["push", "-q", "origin", "HEAD:app/delta"]);
    let sibling = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let origin = w.work.at("origin.git");
    w.git(&origin, &["merge-base", "--is-ancestor", &sibling, "main"]);
    let files = w.git(&origin, &["ls-tree", "-r", "--name-only", "main"]);
    for file in [
        "apps/delta/one.txt",
        "apps/delta/two.txt",
        "apps/gamma/work.txt",
    ] {
        assert!(files.contains(file), "{file}: {files}");
    }
    assert_eq!(w.origin_ref("app/delta"), sibling);
}

#[test]
fn a_conflict_moving_onto_main_leaves_main_untouched_and_parks() {
    let w = World::app("land-app-conflict");
    let clone = w.clone_dir();
    w.work.put("thread/shared.txt", "gamma's\n");
    w.git(
        &clone,
        &["commit", "-q", "-am", "Change shared.txt in gamma"],
    );
    w.trunk_moves("shared.txt", "main's\n");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line = last_line(&out);
    assert!(line.starts_with("parked: main moved: gamma"), "{line}");
    assert_eq!(w.origin_main(), before);
    assert!(w.origin_log("app/gamma").contains("Build the milestone"));
}

/// Main took something that breaks gamma's tests only once gamma sits on
/// it: the milestone stays on app/gamma, and main is left as it was.
#[test]
fn red_tests_moving_onto_main_leave_main_untouched_and_park() {
    let w = World::app("land-app-red-move");
    w.verify("test -f work.txt && test ! -f ../../breaks-gamma.txt");
    w.trunk_moves("breaks-gamma.txt", "x\n");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line = last_line(&out);
    assert!(line.starts_with("parked: main moved: gamma"), "{line}");
    assert!(line.contains("the tests fail"), "{line}");
    assert_eq!(w.origin_main(), before);
    assert!(w.origin_log("app/gamma").contains("Build the milestone"));
    assert!(w.ticked(), "{}", w.work.read("mem.log"));
}

/// Main moves while the tests of gamma rebased onto main run: the push is
/// refused, never forced, and the move starts over on the new main.
#[test]
fn a_main_that_moves_during_the_tests_of_the_move_is_rebased_onto_again() {
    let w = World::app("land-app-race");
    w.trunk_moves("apps/delta/other.txt", "another app\n");
    let seed = w.work.at("seed");
    // The first run counts as the landing on app/gamma; the second, the
    // first of the move onto main, pushes another app's commit to main.
    w.work.script(
        "move-second",
        &format!(
            r#"
echo run >>"{runs}"
[ "$(wc -l <"{runs}")" -eq 2 ] || exit 0
cd "{seed}" || exit 1
git pull -q origin main
echo raced >raced.txt
git add raced.txt
git commit -q -m "Race the move"
git push -q origin HEAD:main
"#,
            runs = w.work.at("runs").display(),
            seed = seed.display()
        ),
    );
    w.verify(&format!(
        "test -f work.txt && {}",
        w.work.at("move-second").display()
    ));
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let log = w.origin_log("main");
    assert!(log.contains("Race the move"), "{log}");
    assert!(log.contains("Change apps/delta/other.txt"), "{log}");
    assert!(log.contains("Build the milestone"), "{log}");
    assert_eq!(w.origin_ref("app/gamma"), w.origin_main());
}

/// The test command runs in the app's own folder, as a local run does.
#[test]
fn the_test_command_of_an_app_runs_in_its_folder() {
    let w = World::app("land-app-folder");
    let clone = w.clone_dir();
    w.work.put("thread/apps/gamma/here.txt", "gamma's\n");
    w.git(&clone, &["add", "."]);
    w.git(&clone, &["commit", "-q", "-m", "Add here.txt"]);
    w.verify("test -f here.txt");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(last_line(&out), "next: Run the next milestone of gamma.");
}

#[test]
fn a_root_project_lands_on_main_and_makes_no_app_branch() {
    let w = World::new("land-root");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        w.origin_main(),
        w.git(&w.clone_dir(), &["rev-parse", "HEAD"])
    );
    let origin = w.work.at("origin.git");
    let apps = w.git(&origin, &["for-each-ref", "refs/heads/app"]);
    assert_eq!(apps, "");
}

#[test]
fn a_project_without_land_ci_never_asks_github() {
    let w = World::new("land-no-ci");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(w.gh_log(), "", "gh was called");
    assert!(!stdout(&out).contains("waiting on"), "{}", stdout(&out));
    let refs = w.git(
        &w.work.at("origin.git"),
        &["for-each-ref", "--format=%(refname)"],
    );
    assert!(!refs.contains("refs/heads/ci/"), "{refs}");
}

#[test]
fn a_green_ci_run_on_the_rebased_sha_lands_it() {
    let w = World::new("land-ci-green");
    w.land_ci("green");
    w.trunk_moves("other.txt", "another thread\n");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let head = w.git(&w.clone_dir(), &["rev-parse", "HEAD"]);
    assert!(
        stdout(&out).contains(&format!("land: waiting on ci.yml for {}\n", w.short_head())),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        w.origin_ref("ci/gamma-m2-next"),
        head,
        "CI ran on the rebased sha"
    );
    assert_eq!(w.origin_main(), head);
    assert!(
        w.gh_log().contains(&format!("head_sha={head}")),
        "{}",
        w.gh_log()
    );
    assert_eq!(last_line(&out), "next: Run the next milestone of gamma.");
    assert!(w.ticked());
}

#[test]
fn a_red_ci_run_parks_naming_its_failed_jobs_and_pushes_nothing_to_main() {
    let w = World::new("land-ci-red");
    w.land_ci("red");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        last_line(&out),
        format!("parked: CI red on {}: php 8.4 postgres:15", w.short_head()),
        "the pull request's green run on another branch never counts"
    );
    assert_eq!(w.origin_main(), before);
    assert!(!w.ticked());
}

/// One call waits at most 9 minutes, under a Bash call's 10: with no run
/// yet it says to run land again, and the 10 minutes count from the push,
/// across calls.
#[test]
fn no_ci_run_for_the_sha_parks_ten_minutes_after_the_push() {
    let w = World::new("land-ci-none");
    w.land_ci("none");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(
        last_line(&out),
        format!(
            "again: waiting on ci.yml for {}; run workflow land again",
            w.short_head()
        )
    );
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        last_line(&out),
        format!("parked: no ci.yml run for {}", w.short_head())
    );
    assert_eq!(w.origin_main(), before);
}

#[test]
fn a_ci_run_younger_than_ninety_minutes_is_waited_on_again() {
    let w = World::new("land-ci-young");
    w.land_ci("running");
    // A minute of 200 ms: the run started 15 s ago, 75 minutes, and one call
    // waits 9 minutes, so it is 84 minutes old when land says to run again.
    w.work.put("run-age", "15");
    let out = w.land_minute("200");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(last_line(&out).starts_with("again: "), "{}", stdout(&out));
}

#[test]
fn a_ci_run_still_going_ninety_minutes_after_it_started_parks() {
    let w = World::new("land-ci-running");
    w.land_ci("running");
    // 19 s at a minute of 200 ms: 95 minutes.
    w.work.put("run-age", "19");
    let before = w.origin_main();
    let out = w.land_minute("200");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(
        last_line(&out),
        format!("parked: ci.yml still running on {}", w.short_head())
    );
    assert_eq!(w.origin_main(), before);
}

#[test]
fn an_origin_that_refuses_the_ci_ref_parks_with_its_reason() {
    let w = World::new("land-ci-push-refused");
    w.land_ci("green");
    let hook = w.work.at("origin.git/hooks/pre-receive");
    w.work.script(
        "origin.git/hooks/pre-receive",
        "echo 'ci refs are not allowed here' >&2\nexit 1\n",
    );
    assert!(hook.exists());
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line = last_line(&out);
    assert!(
        line.starts_with("parked: origin refused ci/gamma-m2-next:"),
        "{line}"
    );
    assert!(line.contains("ci refs are not allowed here"), "{line}");
    assert_eq!(w.origin_main(), before);
}

#[test]
fn a_github_that_refuses_the_actions_api_parks_saying_so() {
    let w = World::new("land-ci-refused");
    w.land_ci("green");
    w.work.put("gh-refused", "");
    let before = w.origin_main();
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let line = last_line(&out);
    assert!(
        line.starts_with("parked: GitHub refused the Actions API for me/gamma:"),
        "{line}"
    );
    assert!(line.contains("HTTP 403"), "{line}");
    assert_eq!(w.origin_main(), before);
}

/// A second landing after a red run: the trunk moved, so the rebase makes a
/// new sha, which goes to the same ci/ ref over the old one with a lease,
/// and only its own run counts.
#[test]
fn a_new_sha_after_a_red_run_needs_its_own_green_run() {
    let w = World::new("land-ci-again");
    w.land_ci("red");
    let out = w.land();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let red = w.origin_ref("ci/gamma-m2-next");
    w.trunk_moves("other.txt", "another thread\n");
    w.work.put("ci-mode", "green");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let head = w.git(&w.clone_dir(), &["rev-parse", "HEAD"]);
    assert_ne!(head, red);
    assert_eq!(w.origin_ref("ci/gamma-m2-next"), head);
    assert_eq!(w.origin_main(), head);
}

/// A landing told to run again finds the ci/ ref on its sha: the tests
/// passed on it already, so they do not run again, and its run is read.
#[test]
fn a_landing_run_again_on_the_same_sha_reads_its_run_without_testing_again() {
    let w = World::new("land-ci-same");
    w.land_ci("running");
    let count = w.work.at("verified");
    w.verify(&format!(
        "test -f work.txt && echo run >>{}",
        count.display()
    ));
    let out = w.land();
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let sha = w.origin_ref("ci/gamma-m2-next");
    w.work.put(&format!("runs/{sha}"), "green");
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(w.origin_main(), sha);
    assert_eq!(w.work.read("verified").lines().count(), 1, "tested once");
}

/// The trunk moves while CI runs: main refuses the green sha, land rebases
/// again, and main takes only the new sha once its own run passed.
#[test]
fn a_trunk_that_moves_while_ci_runs_gets_a_new_run_before_main_moves() {
    let w = World::new("land-ci-race");
    w.land_ci("green");
    w.work.put("move-during-ci", "");
    let first = w.git(&w.clone_dir(), &["rev-parse", "HEAD"]);
    let out = w.land();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let head = w.git(&w.clone_dir(), &["rev-parse", "HEAD"]);
    assert_ne!(head, first);
    assert!(w.origin_log("main").contains("Move during CI"));
    assert_eq!(w.origin_main(), head);
    assert_eq!(w.origin_ref("ci/gamma-m2-next"), head);
    let log = w.gh_log();
    assert!(log.contains(&format!("head_sha={first}")), "{log}");
    assert!(log.contains(&format!("head_sha={head}")), "{log}");
}
