//! `workflow agent` through the real binary, with scripts in the place of
//! curl (`$WORKFLOW_CURL`), mem, amx and systemd-run
//! (`$WORKFLOW_SYSTEMD_RUN`). Every fake appends `<name> <argv>` to one
//! `calls.log`, so the order of calls across them can be read back.

#[path = "../src/scratch.rs"]
mod scratch;

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use scratch::Scratch;
use serde_json::json;

const HUB: &str = "https://dulali.taila27604.ts.net";

struct World {
    work: Scratch,
}

impl World {
    fn new(tag: &str) -> World {
        let work = Scratch::new(tag);
        work.put(
            "projects.json",
            r#"{"projects":[
              {"name":"gamma","checkouts":["/src/gamma"]},
              {"name":"beta","checkouts":[]},
              {"name":"workflow","checkouts":["/src/workflow","/wt/workflow"],"worktree":"/wt/workflow"}
            ]}"#,
        );
        work.put("ls.json", "[]");
        // curl: one argv element per line in curl.argv, the body it was
        // handed in curl.stdin. A pairing call (-D -) prints pair-reply; a poll
        // prints reply once, then an empty command list, and the status code
        // last, as `-w '\n%{http_code}'` would.
        work.script(
            "curl",
            r#"
dir=$(dirname "$0")
echo "curl $*" | tr '\n' ' ' >>"$dir/calls.log"
echo >>"$dir/calls.log"
for a in "$@"; do printf '%s\n' "$a"; done >"$dir/curl.argv"
cat >"$dir/curl.stdin"
case " $* " in
*" -D "*) cat "$dir/pair-reply" ;;
*)
	if [ -f "$dir/reply" ]; then
		cat "$dir/reply"
		mv "$dir/reply" "$dir/served"
	else
		printf '{"commands":[]}'
	fi
	printf '\n%s' "$(cat "$dir/status" 2>/dev/null || echo 200)"
	;;
esac
"#,
        );
        // mem: `sync` fails while sync-fails says always, or once, and finds
        // a round already running, as mem says with exit 0, while
        // sync-busy says once. A sync-slow file makes every round take 3 s.
        work.script(
            "mem",
            r#"
dir=$(dirname "$0")
echo "mem $*" >>"$dir/calls.log"
case "$1" in
projects) cat "$dir/projects.json" ;;
sync)
	if [ -e "$dir/sync-slow" ]; then sleep 3; fi
	case "$(cat "$dir/sync-fails" 2>/dev/null)" in
	always) exit 1 ;;
	once) rm "$dir/sync-fails"; exit 1 ;;
	esac
	if [ -e "$dir/sync-busy" ]; then
		rm "$dir/sync-busy"
		echo '{"sync":"not performed","detail":"another round is in progress; not performed"}'
	else
		echo '{"sync":"performed","detail":""}'
	fi
	;;
esac
"#,
        );
        // amx: `resume` writes its last argv element, byte for byte.
        work.script(
            "amx",
            r#"
dir=$(dirname "$0")
echo "amx $*" >>"$dir/calls.log"
case "$1" in
ls) cat "$dir/ls.json" ;;
resume|send)
	for a in "$@"; do last=$a; done
	printf '%s' "$last" >"$dir/last-arg"
	;;
esac
"#,
        );
        // systemd-run: logs, and runs the fake amx it is handed; `workflow go`
        // is only logged.
        work.script(
            "systemd-run",
            r#"
dir=$(dirname "$0")
echo "systemd-run $*" >>"$dir/calls.log"
shift 3
case "$1" in
*/amx) exec "$@" ;;
esac
"#,
        );
        // notify-send: one argv element per line in notify.argv, and a line
        // in notify.count per run.
        work.script(
            "notify-send",
            r#"
dir=$(dirname "$0")
echo run >>"$dir/notify.count"
for a in "$@"; do printf '%s\n' "$a"; done >"$dir/notify.argv"
"#,
        );
        // claude: one argv element per line in claude.argv, and the reply
        // in claude-reply, `ok` by default.
        work.script(
            "claude",
            r#"
dir=$(dirname "$0")
echo "claude $#" >>"$dir/calls.log"
for a in "$@"; do printf '%s\n' "$a"; done >"$dir/claude.argv"
if [ -f "$dir/claude-reply" ]; then
	cat "$dir/claude-reply"
else
	echo '{"ok":true,"session_id":"session_01X","url":"https://claude.ai/code/session_01X"}'
fi
"#,
        );
        World { work }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workflow"));
        command
            .arg("agent")
            .args(args)
            .env("HOME", self.work.at("home"))
            .env("XDG_STATE_HOME", self.work.at("state"))
            .env("XDG_RUNTIME_DIR", self.work.at("run"))
            .env("WORKFLOW_CURL", self.work.at("curl"))
            .env("WORKFLOW_MEM", self.work.at("mem"))
            .env("WORKFLOW_AMX", self.work.at("amx"))
            .env("WORKFLOW_SYSTEMD_RUN", self.work.at("systemd-run"))
            .env("WORKFLOW_CLAUDE", self.work.at("claude"))
            .env("WORKFLOW_NOTIFY_SEND", self.work.at("notify-send"))
            .env("WORKFLOW_MACHINE", "mini")
            .env("WORKFLOW_AGENT_INTERVAL_MS", "50")
            .current_dir(self.work.path());
        command
    }

    fn agent(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn paired(&self) {
        self.work.put("state/workflow/agent-token", "tok123");
        self.work.put("state/workflow/agent-hub", HUB);
    }

    /// The hub hands out these commands on the next poll.
    fn hub_says(&self, commands: serde_json::Value) {
        self.work
            .put("reply", &json!({ "commands": commands }).to_string());
    }

    fn calls(&self) -> Vec<String> {
        if !self.work.has("calls.log") {
            return Vec::new();
        }
        self.work
            .read("calls.log")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The calls the commands of a `--once` run made: everything after its
    /// one poll.
    fn command_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .skip_while(|c| !c.starts_with("curl "))
            .skip(1)
            .collect()
    }

    fn curl_argv(&self) -> Vec<String> {
        self.work
            .read("curl.argv")
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn pairing_saves_the_token_and_the_hub() {
    let w = World::new("agent-pair");
    w.work.put(
        "pair-reply",
        "HTTP/2 200\r\nSet-Cookie: hub_device=tok123; Path=/; HttpOnly; Secure\r\ncontent-length: 0\r\n\r\n",
    );
    let link = format!("{HUB}/pair/AB12CD");
    let out = w.agent(&["pair", &link]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("paired with {HUB}\n")
    );
    let argv = w.curl_argv();
    assert!(argv.contains(&format!("Origin: {HUB}")), "{argv:?}");
    assert_eq!(argv.last(), Some(&link));
    assert_eq!(w.work.read("state/workflow/agent-token"), "tok123");
    assert_eq!(w.work.read("state/workflow/agent-hub"), HUB);
    let mode = std::fs::metadata(w.work.at("state/workflow/agent-token"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn pairing_without_a_cookie_fails_and_saves_nothing() {
    let w = World::new("agent-pair-refused");
    w.work
        .put("pair-reply", "HTTP/2 403\r\ncontent-length: 0\r\n\r\n");
    let out = w.agent(&["pair", &format!("{HUB}/pair/AB12CD")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no hub_device cookie"),
        "{out:?}"
    );
    assert!(!w.work.has("state/workflow/agent-token"));
}

#[test]
fn an_unpaired_agent_refuses_to_run() {
    let w = World::new("agent-unpaired");
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "workflow agent: not paired; run `workflow agent pair <link>`\n"
    );
    assert!(w.calls().is_empty());
}

#[test]
fn a_poll_reports_the_machine_its_agents_and_its_checkouts() {
    let w = World::new("agent-poll");
    w.paired();
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"working","ended":0,"created":1791400000,"dir":"/src/gamma","task":"go"},
            {"id":"old","state":"stopped","ended":1791400500}]"#,
    );
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let argv = w.curl_argv();
    assert!(
        argv.contains(&"Authorization: Bearer tok123".to_string()),
        "{argv:?}"
    );
    assert!(argv.contains(&"Content-Type: application/json".to_string()));
    assert_eq!(
        argv.last().map(String::as_str),
        Some(&*format!("{HUB}/api/agent"))
    );

    let body: serde_json::Value = serde_json::from_str(&w.work.read("curl.stdin")).unwrap();
    assert_eq!(
        body,
        json!({
            "machine": "mini",
            "agents": [
                {"id": "gamma-m1", "state": "working", "ended": 0, "created": 1791400000},
                {"id": "old", "state": "stopped", "ended": 1791400500, "created": null}
            ],
            "checkouts": ["gamma", "workflow"],
            "worktrees": ["workflow"],
            "unpushed": {},
            "watched": false
        })
    );
}

/// A cloud run clones origin, so the hub must know which trunks hold work
/// origin/main lacks: it shows "push main first" in place of their prompt.
#[test]
fn a_poll_counts_the_commits_a_checkouts_main_has_not_pushed() {
    let w = World::new("agent-unpushed");
    w.paired();
    let origin = w.work.at("origin.git");
    let checkout = w.work.mkdir("gamma");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(&checkout)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&[
        "init",
        "-q",
        "--bare",
        "-b",
        "main",
        origin.to_str().unwrap(),
    ]);
    git(&["init", "-q", "-b", "main"]);
    git(&["remote", "add", "origin", origin.to_str().unwrap()]);
    git(&["commit", "-q", "--allow-empty", "-m", "start"]);
    git(&["push", "-q", "origin", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "Land t1 on the mini"]);
    git(&["commit", "-q", "--allow-empty", "-m", "Land t2 on the mini"]);
    w.work.put(
        "projects.json",
        &format!(
            r#"{{"projects":[{{"name":"gamma","checkouts":["{}"]}},{{"name":"beta","checkouts":["/src/beta"]}}]}}"#,
            checkout.display()
        ),
    );
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let body: serde_json::Value = serde_json::from_str(&w.work.read("curl.stdin")).unwrap();
    assert_eq!(body["unpushed"], json!({"gamma": 2}), "{body}");
}

/// qshell keeps its presence marker while the screen is unlocked and in
/// use, and the hub stays quiet while any machine is watched.
#[test]
fn a_poll_says_whether_someone_is_at_the_machine() {
    let w = World::new("agent-watched");
    w.paired();
    w.work.put("run/qshell-present", "");
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let body: serde_json::Value = serde_json::from_str(&w.work.read("curl.stdin")).unwrap();
    assert_eq!(body["watched"], json!(true), "{body}");
}

#[test]
fn start_and_sync_run_in_the_order_the_hub_gave_them() {
    let w = World::new("agent-order");
    w.paired();
    w.hub_says(json!([
        {"kind": "start", "project": "gamma"},
        {"kind": "sync"},
        {"kind": "start", "project": "workflow"}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    // The sync job runs on its own thread, so only the starts have an order.
    let calls = w.command_calls();
    let starts: Vec<&String> = calls
        .iter()
        .filter(|c| c.starts_with("systemd-run"))
        .collect();
    assert_eq!(
        starts,
        [
            "systemd-run --user --scope --quiet workflow go gamma",
            "systemd-run --user --scope --quiet workflow go workflow",
        ]
    );
    let syncs = calls.iter().filter(|c| *c == "mem sync --json").count();
    assert_eq!(syncs, 3, "{calls:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 3);
}

/// Start on the hub often follows an approval the hub just wrote, and
/// workflow go reads this machine's store, so a start waits for a performed
/// round as a resume does.
#[test]
fn a_start_waits_for_a_performed_sync_round() {
    let w = World::new("agent-start-busy");
    w.paired();
    w.work.put("sync-busy", "");
    w.hub_says(json!([{"kind": "start", "project": "gamma"}]));
    let mut child = w.command(&[]).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let started = |w: &World| w.calls().iter().any(|c| c.starts_with("systemd-run"));
    while !started(&w) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let ran: Vec<String> = w
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("mem sync") || c.starts_with("systemd-run"))
        .collect();
    assert_eq!(
        ran,
        [
            "mem sync --json",
            "mem sync --json",
            "systemd-run --user --scope --quiet workflow go gamma"
        ]
    );
}

/// A slow memory sync round runs on its own thread, so the resume behind it
/// in the same poll is not held up.
#[test]
fn a_resume_does_not_wait_for_a_slow_sync_round() {
    let w = World::new("agent-resume-slow-sync");
    w.paired();
    w.work.put("sync-slow", "");
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"stopped","ended":1791400500}]"#,
    );
    w.hub_says(json!([
        {"kind": "sync"},
        {"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": "yes"}
    ]));
    let started = Instant::now();
    let mut child = w.command(&[]).spawn().unwrap();
    while !w.work.has("last-arg") && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let waited = started.elapsed();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(waited < Duration::from_secs(1), "{waited:?}");
}

#[test]
fn a_resume_hands_amx_the_message_byte_for_byte() {
    let w = World::new("agent-resume-bytes");
    w.paired();
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"stopped","ended":1791400500,"created":1791400000}]"#,
    );
    let message = "use \"both\" C:\\path\nsecond line $(touch pwned) `touch pwned`";
    w.hub_says(json!([
        {"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": message}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("last-arg"), message);
    assert!(!w.work.has("pwned"));
    let calls = w.command_calls();
    assert_eq!(calls[0], "amx ls --json");
    assert!(
        calls[1].starts_with(&format!(
            "systemd-run --user --scope --quiet {} resume gamma-m1 use",
            w.work.at("amx").display()
        )),
        "{calls:?}"
    );
}

#[test]
fn a_resume_runs_though_its_sync_fails() {
    let w = World::new("agent-resume-nosync");
    w.paired();
    w.work.put("sync-fails", "always");
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"stopped","ended":1791400500}]"#,
    );
    w.hub_says(json!([
        {"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": "yes"}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("last-arg"), "yes");
    assert_eq!(
        w.command_calls().last().map(String::as_str),
        Some("mem sync --json")
    );
}

/// mem sync exits 0 when a round is already running and it did nothing, and
/// the hub's write may have landed after that round started, so the sync is
/// kept for after the next poll until a round is performed.
#[test]
fn a_sync_no_round_performed_runs_again_after_the_next_poll() {
    let w = World::new("agent-sync-busy");
    w.paired();
    w.work.put("sync-busy", "");
    w.hub_says(json!([{"kind": "sync"}]));
    let mut child = w.command(&[]).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let syncs = |w: &World| {
        w.calls()
            .iter()
            .filter(|c| c.starts_with("mem sync --json"))
            .count()
    };
    while syncs(&w) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(syncs(&w), 2, "{:?}", w.calls());
}

#[test]
fn a_resume_whose_sync_failed_syncs_again_after_the_next_poll() {
    let w = World::new("agent-resume-retry");
    w.paired();
    w.work.put("sync-fails", "once");
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"stopped","ended":1791400500}]"#,
    );
    w.hub_says(json!([
        {"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": "yes"}
    ]));
    let mut child = w.command(&[]).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let syncs = |w: &World| w.calls().iter().filter(|c| *c == "mem sync --json").count();
    while syncs(&w) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();

    // A daemon's polls interleave with its commands, so only the resume and
    // the syncs are read: the resume went first, the failed round came back.
    let ran: Vec<String> = w
        .calls()
        .into_iter()
        .filter(|c| c == "mem sync --json" || c.contains(" resume "))
        .collect();
    assert_eq!(
        ran,
        [
            &format!(
                "systemd-run --user --scope --quiet {} resume gamma-m1 yes",
                w.work.at("amx").display()
            ),
            "amx resume gamma-m1 yes",
            "mem sync --json",
            "mem sync --json",
        ]
    );
}

#[test]
fn a_resume_for_a_gone_session_starts_the_project() {
    let w = World::new("agent-resume-gone");
    w.paired();
    w.hub_says(json!([
        {"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": "yes"}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        w.command_calls(),
        [
            "amx ls --json",
            "mem sync --json",
            "systemd-run --user --scope --quiet workflow go gamma",
        ]
    );
}

#[test]
fn a_resume_for_a_live_session_runs_nothing() {
    let w = World::new("agent-resume-live");
    w.paired();
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"waiting","ended":0,"created":1791400000}]"#,
    );
    w.hub_says(json!([
        {"kind": "resume", "project": "gamma", "id": "gamma-m1", "message": "yes"}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.command_calls(), ["amx ls --json", "mem sync --json"]);
}

#[test]
fn a_notify_command_runs_notify_send_once() {
    let w = World::new("agent-notify");
    w.paired();
    w.hub_says(json!([{"kind":"notify","title":"Question","body":"Ship it?"}]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("notify.count"), "run\n");
    assert_eq!(
        w.work.read("notify.argv"),
        "-a\nworkflow\nQuestion\nShip it?\n"
    );
}

#[test]
fn a_failing_notify_send_still_counts_done() {
    let w = World::new("agent-notify-fails");
    w.paired();
    w.work.script(
        "notify-send",
        r#"echo run >>"$(dirname "$0")/notify.count"; exit 1"#,
    );
    w.hub_says(json!([{"kind":"notify","title":"t","body":"b"}]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("notify.count"), "run\n");
}

#[test]
fn a_route_in_the_answer_is_written_for_qshell() {
    let w = World::new("agent-route-file");
    w.paired();
    w.work.mkdir("run");
    w.work.put(
        "reply",
        r#"{"commands":[],"route":{"route":"device","machine":"mac-mini-m2","waiting":2}}"#,
    );
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let file: serde_json::Value =
        serde_json::from_str(&w.work.read("run/workflow-route.json")).unwrap();
    assert_eq!(
        file,
        json!({"route":"device","machine":"mac-mini-m2","waiting":2,"hub":HUB,"self":"mini"})
    );
    let leftovers: Vec<_> = std::fs::read_dir(w.work.at("run"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["workflow-route.json"]);
}

#[test]
fn an_answer_without_a_route_leaves_the_file_alone() {
    let w = World::new("agent-route-absent");
    w.paired();
    w.work.put("run/workflow-route.json", "old");
    w.hub_says(json!([]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("run/workflow-route.json"), "old");
}

/// Saiful's answer after Dulali's window reaches a session that is still
/// running as a new message, byte for byte.
#[test]
fn a_follow_up_is_sent_into_a_live_session() {
    let w = World::new("agent-follow-up-live");
    w.paired();
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"working","ended":0,"created":1791400000}]"#,
    );
    let message = "Saiful replaced Dulali's answer: \"nine\" $(touch pwned)";
    w.hub_says(json!([
        {"kind": "follow_up", "project": "gamma", "id": "gamma-m1", "message": message}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("last-arg"), message);
    assert!(!w.work.has("pwned"));
    let calls = w.command_calls();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("amx send gamma-m1 Saiful")),
        "{calls:?}"
    );
    assert!(!calls.iter().any(|c| c.contains(" resume ")), "{calls:?}");
}

/// A session that stopped since is resumed with the follow-up instead.
#[test]
fn a_follow_up_for_a_stopped_session_resumes_it() {
    let w = World::new("agent-follow-up-stopped");
    w.paired();
    w.work.put(
        "ls.json",
        r#"[{"id":"gamma-m1","state":"stopped","ended":1791400500,"created":1791400000}]"#,
    );
    w.hub_says(json!([
        {"kind": "follow_up", "project": "gamma", "id": "gamma-m1", "message": "nine"}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(w.work.read("last-arg"), "nine");
    let calls = w.command_calls();
    assert!(
        calls.iter().any(|c| c.contains(" resume gamma-m1 nine")),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("amx send")),
        "{calls:?}"
    );
}

#[test]
fn a_cloud_start_runs_workflow_go_with_cloud() {
    let w = World::new("agent-cloud-start");
    w.paired();
    w.hub_says(json!([
        {"kind": "start", "project": "gamma", "cloud": true},
        {"kind": "start", "project": "beta"}
    ]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let starts: Vec<String> = w
        .command_calls()
        .into_iter()
        .filter(|c| c.starts_with("systemd-run"))
        .collect();
    assert_eq!(
        starts,
        [
            "systemd-run --user --scope --quiet workflow go gamma --cloud",
            "systemd-run --user --scope --quiet workflow go beta",
        ]
    );
}

#[test]
fn a_cloud_answer_is_forwarded_with_claude() {
    let w = World::new("agent-cloud-answer");
    w.paired();
    w.hub_says(json!([{
        "kind": "cloud", "project": "gamma", "id": "session_01X",
        "message": "#ABCD q\nAnswer: yes"
    }]));
    let out = w.agent(&["--once"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        w.work.read("claude.argv"),
        "-p\n#ABCD q\nAnswer: yes\n--cloud\nsession_01X\n--output-format\njson\n"
    );
    let calls = w.command_calls();
    assert_eq!(calls.iter().filter(|c| c.starts_with("claude ")).count(), 1);
    assert!(
        !calls.iter().any(|c| c.starts_with("mem sync")),
        "{calls:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "cloud session_01X: sent\n"
    );
}

#[test]
fn a_cloud_answer_the_session_refused_is_not_run_again() {
    let w = World::new("agent-cloud-refused");
    w.paired();
    w.work.put(
        "claude-reply",
        r#"{"ok":false,"session_id":"session_01X","error":"session not found"}"#,
    );
    w.hub_says(json!([{
        "kind": "cloud", "project": "gamma", "id": "session_01X", "message": "hi"
    }]));
    let mut child = w
        .command(&[])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let ran = |w: &World| w.calls().iter().any(|c| c.starts_with("claude "));
    while !ran(&w) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // Several polls (50 ms apart) go by.
    std::thread::sleep(Duration::from_millis(500));
    child.kill().unwrap();
    let out = child.wait_with_output().unwrap();
    let claudes = w
        .calls()
        .iter()
        .filter(|c| c.starts_with("claude "))
        .count();
    assert_eq!(claudes, 1, "{:?}", w.calls());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "cloud session_01X: failed: session not found\n"
    );
}
