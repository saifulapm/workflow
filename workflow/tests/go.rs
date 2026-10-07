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

/// A project `alpha` with a checkout, a fake mem and a fake amx that records
/// every call in `amx.log`.
struct World {
    work: Scratch,
    checkout: PathBuf,
}

impl World {
    fn new(tag: &str, status: &str) -> World {
        let work = Scratch::new(tag);
        let checkout = work.mkdir("alpha-checkout");
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
case "$1" in
projects) cat "$dir/projects.json" ;;
project) [ -f "$dir/current.json" ] && cat "$dir/current.json" || exit 1 ;;
roadmap) cat "$dir/roadmap-$3.md" ;;
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_workflow"));
        command
            .arg("go")
            .args(args)
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_AMX", self.work.at("amx"))
            .env_remove("AMX_ID")
            .current_dir(self.work.path());
        if let Some(id) = amx_id {
            command.env("AMX_ID", id);
        }
        command.output().unwrap()
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
fn a_live_orchestrator_refuses_and_the_caller_itself_does_not_count() {
    let world = World::new("go-live", "approved");
    world.work.put(
        "ls.json",
        r#"[{"id":"alpha-skeleton","state":"working"},
            {"id":"alpha-mobile-engage","state":"working"},
            {"id":"beta-winners","state":"working"},
            {"id":"alpha-old","state":"done"}]"#,
    );

    let out = world.go(&["alpha"]);
    assert_eq!(out.status.code(), Some(2), "{}", complained(&out));
    assert!(
        complained(&out).contains("alpha already has an orchestrator running: alpha-skeleton"),
        "{}",
        complained(&out)
    );
    assert!(world.new_calls().is_empty());

    // The orchestrator of the milestone before starts the next: it is the
    // agent that is running, and it is the one asking.
    let out = world.go_as(&["alpha"], Some("alpha-skeleton"));
    assert_eq!(out.status.code(), Some(0), "{}", complained(&out));
    assert_eq!(said(&out), "alpha-winners\n");
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
