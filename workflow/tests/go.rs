//! `workflow go` through the real binary, with a script in the place of mem
//! (`$WORKFLOW_MEM`) and another in the place of amx (`$WORKFLOW_AMX`).

#[path = "../src/scratch.rs"]
mod scratch;

use std::path::PathBuf;
use std::process::{Command, Output};

use scratch::Scratch;

const ROADMAP: &str = "\
# roadmap: alpha

- [x] skeleton The platform opens
- [ ] winners Find and import products
- [ ] orders An order reaches the supplier
";

/// A project `alpha` with a checkout, a fake mem and a fake amx that record
/// every call in `mem.log` and `amx.log`. The fake mem keeps a pause as a
/// `paused-<project>` file and splices it into its `projects` row.
struct World {
    work: Scratch,
    checkout: PathBuf,
}

impl World {
    fn new(tag: &str, status: &str) -> World {
        let work = Scratch::new(tag);
        let checkout = work.mkdir("alpha-checkout");
        git(&checkout, &["init", "-q", "-b", "main"]);
        work.put(
            "projects.json",
            &format!(
                r#"{{"projects":[
                  {{"name":"alpha","checkouts":["{dir}"],"roadmap_status":{status}}},
                  {{"name":"alpha-mobile","checkouts":[],"roadmap_status":"approved"}}
                ]}}"#,
                dir = checkout.display(),
                status = if status == "null" {
                    "null".to_string()
                } else {
                    format!("\"{status}\"")
                },
            ),
        );
        work.put("roadmap-alpha.md", ROADMAP);
        work.put("ls.json", "[]");
        work.put("taken", "");
        work.script(
            "mem",
            r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/mem.log"
case "$1" in
projects)
	out=$(cat "$dir/projects.json")
	for f in "$dir"/paused-*; do
		[ -f "$f" ] || continue
		p=${f##*/paused-}
		out=$(printf '%s' "$out" | sed "s|\"name\":\"$p\",|&\"paused\":\"$(cat "$f")\",|")
	done
	printf '%s\n' "$out"
	;;
project)
	case "$2" in
	set) printf '2026-10-15T09:00:00Z by %s' "$4" >"$dir/paused-$6" ;;
	unset) rm -f "$dir/paused-$5" ;;
	*) [ -f "$dir/current.json" ] && cat "$dir/current.json" || exit 1 ;;
	esac
	;;
roadmap) cat "$dir/roadmap-$3.md" ;;
handoff) [ -f "$dir/handoff-$3.md" ] && cat "$dir/handoff-$3.md" || exit 1 ;;
esac
"#,
        );
        work.script(
            "amx",
            r#"
dir=$(dirname "$0")
echo "$*" >>"$dir/amx.log"
case "$1" in
ls) cat "$dir/ls.json" ;;
new)
	if grep -qx -- "$3" "$dir/taken"; then
		echo "amx: name \"$3\" is already taken" >&2
		exit 1
	fi
	[ -f "$dir/new-fails" ] && { cat "$dir/new-fails" >&2; exit 1; }
	echo "started $3"
	;;
esac
"#,
        );
        World { work, checkout }
    }

    fn go(&self, args: &[&str]) -> Output {
        self.go_as(args, None)
    }

    fn go_as(&self, args: &[&str], amx_id: Option<&str>) -> Output {
        self.run(&[&["go"], args].concat(), amx_id)
    }

    /// Any workflow command, against the fakes. `qshell/machine` under the
    /// scratch config dir names the machine.
    fn run(&self, args: &[&str], amx_id: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workflow"));
        command
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_AMX", self.work.at("amx"))
            .env("XDG_CONFIG_HOME", self.work.at("config"))
            .env_remove("AMX_ID")
            .current_dir(self.work.path());
        if let Some(id) = amx_id {
            command.env("AMX_ID", id);
        }
        command.output().unwrap()
    }

    /// A second project, `gamma`, with an approved roadmap and a checkout of
    /// its own.
    fn add_gamma(&self) {
        let checkout = self.work.mkdir("gamma-checkout");
        git(&checkout, &["init", "-q", "-b", "main"]);
        let projects = self.work.read("projects.json").replacen(
            "\n                ]}",
            &format!(
                r#",
                  {{"name":"gamma","checkouts":["{}"],"roadmap_status":"approved"}}
                ]}}"#,
                checkout.display()
            ),
            1,
        );
        self.work.put("projects.json", &projects);
        self.work
            .put("roadmap-gamma.md", "# roadmap: gamma\n\n- [ ] first One\n");
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

    /// Every amx call so far, one line each.
    fn amx_calls(&self) -> Vec<String> {
        if !self.work.has("amx.log") {
            return Vec::new();
        }
        self.work
            .read("amx.log")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn new_calls(&self) -> Vec<String> {
        self.amx_calls()
            .into_iter()
            .filter(|call| call.starts_with("new "))
            .collect()
    }

    /// The `amx new` line for `name`.
    fn new_line(&self, name: &str) -> String {
        format!(
            "new --name {name} --model opus --effort high --permission bypassPermissions \
             --no-worktree --dir {} {}",
            self.checkout.display(),
            task_of("alpha", "winners"),
        )
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn task_of(project: &str, slug: &str) -> String {
    workflow::go::task(project, slug)
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn complained(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn it_starts_the_orchestrator_and_prints_its_name() {
    let world = World::new("go-start", "approved");

    let out = world.go(&["alpha"]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "alpha-winners\n");
    assert_eq!(world.new_calls(), [world.new_line("alpha-winners")]);
}

#[test]
fn a_dry_run_prints_the_command_line_and_starts_nothing() {
    let world = World::new("go-dry", "approved");

    let out = world.go(&["alpha", "--dry-run"]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    let want = format!(
        "{} new --name alpha-winners --model opus --effort high --permission bypassPermissions \
         --no-worktree --dir {} '{}'\n",
        world.work.at("amx").display(),
        world.checkout.display(),
        // The goal has an apostrophe in it, which a shell needs spelled out.
        task_of("alpha", "winners").replace('\'', "'\\''"),
    );
    assert_eq!(said(&out), want);
    assert!(world.new_calls().is_empty(), "{:?}", world.amx_calls());
}

#[test]
fn a_dry_run_still_refuses_what_a_real_run_would() {
    let world = World::new("go-dry-refuse", "draft");
    let out = world.go(&["alpha", "--dry-run"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(said(&out).is_empty());

    let world = World::new("go-dry-live", "approved");
    world
        .work
        .put("ls.json", r#"[{"id":"alpha-winners","state":"working"}]"#);
    let out = world.go(&["alpha", "--dry-run"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_model_the_effort_and_the_milestone_can_be_named() {
    let world = World::new("go-flags", "approved");

    let out = world.go(&[
        "alpha",
        "--milestone",
        "orders",
        "--model",
        "sonnet",
        "--effort",
        "max",
    ]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "alpha-orders\n");
    let call = &world.new_calls()[0];
    assert!(
        call.starts_with("new --name alpha-orders --model sonnet --effort max --permission "),
        "{call}"
    );
    assert!(call.contains(&task_of("alpha", "orders")), "{call}");
}

#[test]
fn a_roadmap_that_is_not_approved_is_refused_unless_a_milestone_is_named() {
    for status in ["null", "draft", "running"] {
        let world = World::new("go-status", status);

        let out = world.go(&["alpha"]);
        assert_eq!(out.status.code(), Some(2), "{status}");
        assert!(
            complained(&out).contains("alpha: the roadmap is not approved"),
            "{status}: {}",
            complained(&out)
        );
        assert!(world.new_calls().is_empty());

        let out = world.go(&["alpha", "--milestone", "winners"]);
        assert_eq!(out.status.code(), Some(0), "{status}: {}", complained(&out));
        assert_eq!(world.new_calls().len(), 1);
    }
}

#[test]
fn a_live_orchestrator_of_the_milestone_refuses_and_an_earlier_one_does_not() {
    let world = World::new("go-live", "approved");
    world.work.put(
        "ls.json",
        r#"[{"id":"alpha-winners-2","state":"working","ended":0},
            {"id":"alpha-mobile-winners","state":"working"},
            {"id":"beta-winners","state":"working"}]"#,
    );
    let out = world.go(&["alpha"]);
    assert_eq!(out.status.code(), Some(2), "{}", complained(&out));
    assert!(
        complained(&out).contains("alpha already has an orchestrator running: alpha-winners-2"),
        "{}",
        complained(&out)
    );
    assert!(world.new_calls().is_empty());

    // The orchestrator of the milestone before idles at its prompt once it
    // has started this one, and a dead successor has lost its pane: neither
    // is in the way of starting it again.
    world.work.put(
        "ls.json",
        r#"[{"id":"alpha-skeleton","state":"idle","ended":0},
            {"id":"alpha-winners","state":"stopped","ended":0}]"#,
    );
    let out = world.go(&["alpha"]);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(world.new_calls().len(), 1);
}

#[test]
fn another_projects_orchestrator_in_the_checkout_refuses() {
    // alpha-mobile lives in the same monorepo checkout as alpha, and its
    // orchestrator works there in its own subdir.
    let world = World::new("go-folder", "approved");
    world.work.mkdir("alpha-checkout/.git");
    let mobile = world.work.mkdir("alpha-checkout/apps/mobile");
    let ls = |dir: &std::path::Path| {
        format!(
            r#"[{{"id":"alpha-mobile-engage","state":"working","ended":0,
                 "dir":"{}","task":"{}"}}]"#,
            dir.display(),
            task_of("alpha-mobile", "engage"),
        )
    };
    world.work.put("ls.json", &ls(&mobile));

    let out = world.go(&["alpha", "--dry-run"]);
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert!(
        complained(&out).contains(&format!(
            "alpha-mobile-engage, the orchestrator of alpha-mobile, already works in {}",
            world.checkout.display()
        )),
        "{}",
        complained(&out)
    );

    // In a worktree of its own it is out of the way.
    let own = world.work.mkdir("alpha-mobile-worktree/apps/mobile");
    world.work.put("alpha-mobile-worktree/.git", "gitdir: x\n");
    world.work.put("ls.json", &ls(&own));
    let out = world.go(&["alpha"]);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(world.new_calls(), [world.new_line("alpha-winners")]);
}

#[test]
fn a_checkout_off_main_refuses_unless_it_is_the_recorded_worktree() {
    let world = World::new("go-trunk", "approved");
    git(
        &world.checkout,
        &["symbolic-ref", "HEAD", "refs/heads/feature"],
    );

    let out = world.go(&["alpha", "--dry-run"]);
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert!(
        complained(&out).contains(&format!(
            "{} is on feature, not main",
            world.checkout.display()
        )),
        "{}",
        complained(&out)
    );

    // Recorded as alpha's worktree, its branch is the trunk.
    let projects = world.work.read("projects.json").replacen(
        r#""roadmap_status":"approved"}"#,
        &format!(
            r#""roadmap_status":"approved","worktree":"{}"}}"#,
            world.checkout.display()
        ),
        1,
    );
    world.work.put("projects.json", &projects);
    let out = world.go(&["alpha"]);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(world.new_calls(), [world.new_line("alpha-winners")]);
}

#[test]
fn a_taken_name_gets_a_suffix() {
    let world = World::new("go-taken", "approved");
    world.work.put("taken", "alpha-winners\nalpha-winners-2\n");

    let out = world.go(&["alpha"]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "alpha-winners-3\n");
    assert_eq!(world.new_calls().len(), 3);
}

#[test]
fn a_finished_roadmap_says_so_and_starts_nothing() {
    let world = World::new("go-done", "approved");
    world.work.put(
        "roadmap-alpha.md",
        "# roadmap: alpha\n\n- [x] skeleton One\n- [x] winners Two\n",
    );

    let out = world.go(&["alpha"]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "alpha: the roadmap is done\n");
    assert!(world.new_calls().is_empty());
}

#[test]
fn amx_refusing_fails_with_its_words() {
    let world = World::new("go-amx-fails", "approved");
    world
        .work
        .put("new-fails", "amx: no model `opus` on this machine\n");

    let out = world.go(&["alpha"]);

    assert_eq!(out.status.code(), Some(1));
    assert!(
        complained(&out).contains("amx: no model `opus` on this machine"),
        "{}",
        complained(&out)
    );
    assert!(said(&out).is_empty());
}

#[test]
fn with_no_project_named_the_current_directory_decides() {
    let world = World::new("go-here", "approved");
    let here = world.work.mkdir("somewhere-else");
    git(&here, &["init", "-q", "-b", "main"]);
    world.work.put(
        "current.json",
        &format!(
            r#"{{"id":"01A","name":"alpha","root":"{}"}}"#,
            here.display()
        ),
    );

    let out = world.go(&["--dry-run"]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert!(
        said(&out).contains(&format!("--dir {} ", here.display())),
        "{}",
        said(&out)
    );

    // Where mem knows no project, it asks for one.
    std::fs::remove_file(world.work.at("current.json")).unwrap();
    let out = world.go(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        complained(&out).contains("workflow go <project>"),
        "{}",
        complained(&out)
    );
}

#[test]
fn a_paused_project_is_refused_and_starts_nothing() {
    let world = World::new("go-paused", "approved");
    world.add_gamma();
    world.work.put("config/qshell/machine", "laptop\n");

    let out = world.run(&["pause", "gamma"], None);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "gamma: paused\n");
    assert!(
        world
            .mem_calls()
            .contains(&"project set paused laptop --project gamma".to_string()),
        "{:?}",
        world.mem_calls()
    );
    assert_eq!(
        world.work.read("paused-gamma"),
        "2026-10-15T09:00:00Z by laptop"
    );

    for args in [&["gamma"][..], &["gamma", "--milestone", "first"][..]] {
        let out = world.go(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", complained(&out));
        assert_eq!(said(&out), "gamma: paused\n", "{args:?}");
    }
    assert!(world.new_calls().is_empty(), "{:?}", world.amx_calls());

    // A handoff that is not parked is no way past the pause either.
    world
        .work
        .put("handoff-gamma.md", "working on the first task\n");
    assert_eq!(world.go(&["gamma"]).status.code(), Some(2));
    assert!(world.new_calls().is_empty(), "{:?}", world.amx_calls());

    // A name mem does not know is refused the way go refuses it.
    let out = world.run(&["pause", "nosuch", "--by", "saiful"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        complained(&out).contains("no project named nosuch"),
        "{}",
        complained(&out)
    );
}

#[test]
fn a_parked_handoff_lets_a_paused_project_carry_on() {
    let world = World::new("go-parked", "approved");
    world.add_gamma();
    let out = world.run(&["pause", "gamma", "--by", "saiful"], None);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(
        world.work.read("paused-gamma"),
        "2026-10-15T09:00:00Z by saiful"
    );
    world.work.put(
        "handoff-gamma.md",
        "parked: waiting on #X\nthe rest of it\n",
    );

    let out = world.go(&["gamma"]);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "gamma-first\n");
    assert_eq!(world.new_calls().len(), 1, "{:?}", world.amx_calls());
}

#[test]
fn unpause_clears_the_pause_and_starts_nothing() {
    let world = World::new("go-unpause", "approved");
    world.add_gamma();
    let out = world.run(&["pause", "gamma", "--by", "saiful"], None);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    let amx_before = world.amx_calls().len();

    let out = world.run(&["unpause", "gamma"], None);

    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "gamma: unpaused\n");
    assert!(
        world
            .mem_calls()
            .contains(&"project unset paused --project gamma".to_string()),
        "{:?}",
        world.mem_calls()
    );
    assert!(!world.work.has("paused-gamma"));
    assert_eq!(
        world.amx_calls().len(),
        amx_before,
        "{:?}",
        world.amx_calls()
    );

    world
        .work
        .put("handoff-gamma.md", "working on the first task\n");
    let out = world.go(&["gamma"]);
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "gamma-first\n");
    assert_eq!(world.new_calls().len(), 1, "{:?}", world.amx_calls());
}
