//! `device tmux`: a thread's own tmux session on this device, against a real
//! tmux on a private socket, so no test reaches Saiful's own server.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::{Hub, TempDir, body_of, fixture_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

/// The projects' repo, as mem names its remote.
const REMOTE: &str = "github.com/saifulapm/cloud-spike";

struct Rig {
    hub: Hub,
    home: PathBuf,
    socket: PathBuf,
    /// What fake mem keeps: projects.json, read back, and log, the lines
    /// `mem log` was given.
    state: PathBuf,
    /// The repo's bare origin, with branches main, cloud/cloud_walk-w14 and
    /// cloud/shop-m2.
    origin: PathBuf,
    /// A clone the tests push to origin from.
    seed: PathBuf,
    /// Saiful's own clone, on main; cloud_walk's checkout is its apps/.
    checkout: PathBuf,
    _dir: TempDir,
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The private server only, by its socket.
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .arg("kill-server")
            .output();
    }
}

fn rig() -> Rig {
    rig_env(&[])
}

fn rig_env(env: &[(&str, &str)]) -> Rig {
    let dir = TempDir::new("device-tmux");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini-m2\n").unwrap();
    let bin = dir.join("bin");
    let state = dir.join("mem");
    std::fs::create_dir_all(&state).unwrap();
    // The user env lets git clone a submodule from a local path.
    fixture_bin(
        &bin,
        "systemctl",
        "echo 'PATH=/usr/bin:/bin'\n\
         echo 'GIT_CONFIG_COUNT=1'\n\
         echo 'GIT_CONFIG_KEY_0=protocol.file.allow'\n\
         echo 'GIT_CONFIG_VALUE_0=always'",
    );
    let origin = dir.join("origin.git");
    let seed = dir.join("seed");
    let checkout = dir.join("checkout");
    git(
        dir.path(),
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git(dir.path(), &["clone", "-q", "origin.git", "seed"]);
    std::fs::create_dir_all(seed.join("apps")).unwrap();
    std::fs::write(seed.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(seed.join("apps/README"), "walk\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "first"]);
    git(&seed, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
    // The cloud branches are a commit ahead of main.
    std::fs::write(seed.join("cloud"), "built\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-q", "-m", "cloud"]);
    git(
        &seed,
        &[
            "push",
            "-q",
            "origin",
            "HEAD:refs/heads/cloud/cloud_walk-w14",
            "HEAD:refs/heads/cloud/shop-m2",
        ],
    );
    git(dir.path(), &["clone", "-q", "origin.git", "checkout"]);
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "S='{}'\n\
             for a in \"$@\"; do last=\"$a\"; done\n\
             case \"$*\" in\n\
             *' log -- '*) printf '%s %s\\n' \"$1\" \"$last\" >> \"$S/log\" ;;\n\
             projects*) cat \"$S/projects.json\" 2>/dev/null || echo '{{\"projects\":[]}}' ;;\n\
             *' questions --json'*) cat \"$S/questions.json\" 2>/dev/null || echo '{{\"questions\":[]}}' ;;\n\
             'show '*) cat \"$S/show-$2.json\" 2>/dev/null || echo '{{\"items\":[]}}' ;;\n\
             *handoff*) cat \"$S/handoff.json\" 2>/dev/null || echo '{{}}' ;;\n\
             esac",
            state.display()
        ),
    );
    let socket = dir.join("tmux.sock");
    let local = dir.join("local.sock");
    let mut all = vec![
        ("HUB_PEER_TOKEN", TOKEN),
        ("HUB_LOCAL_SOCKET", local.to_str().unwrap()),
        ("HUB_DEVICE_TMUX_SOCKET", socket.to_str().unwrap()),
    ];
    all.extend_from_slice(env);
    let hub = Hub::spawn_env(&home, &[&bin], &["local", "--port", "0"], &all);
    let rig = Rig {
        hub,
        home,
        socket,
        state,
        origin,
        seed,
        checkout,
        _dir: dir,
    };
    rig.milestones(&[("cloud_walk", "w14"), ("shop", "m2")]);
    rig
}

/// git in `dir`, with none of the user's own config; its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl Rig {
    /// Each project's open milestone, as `mem projects --json` names it, all
    /// of the one remote; cloud_walk's checkout is on this device.
    fn milestones(&self, rows: &[(&str, &str)]) {
        let rows: Vec<_> = rows
            .iter()
            .map(|(name, milestone)| {
                serde_json::json!({"name": name, "milestone": milestone, "remote": REMOTE,
                                   "checkouts": self.checkouts(name)})
            })
            .collect();
        self.projects(serde_json::json!({ "projects": rows }));
    }

    /// What `mem projects --json` answers.
    fn projects(&self, json: serde_json::Value) {
        std::fs::write(self.state.join("projects.json"), json.to_string()).unwrap();
    }

    /// The project's checkouts on this device: cloud_walk's is a subfolder
    /// of Saiful's clone, shop has none.
    fn checkouts(&self, project: &str) -> Vec<String> {
        match project {
            "cloud_walk" => vec![self.checkout.join("apps").display().to_string()],
            _ => vec![],
        }
    }

    /// The worktree folder the sessions work in.
    fn worktree(&self) -> PathBuf {
        self.home.join(".cache/workflow-cloud/cloud-spike")
    }

    /// The sha of origin's `branch`.
    fn origin_sha(&self, branch: &str) -> String {
        git(
            &self.origin,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }

    /// Moves origin's `branch` on by a commit made in `change`, run in a
    /// clone at its tip.
    fn push(&self, branch: &str, change: impl FnOnce(&Path)) {
        git(&self.seed, &["fetch", "-q", "origin"]);
        git(
            &self.seed,
            &["checkout", "-q", "--detach", &format!("origin/{branch}")],
        );
        change(&self.seed);
        git(&self.seed, &["add", "."]);
        git(&self.seed, &["commit", "-q", "-m", "next"]);
        git(
            &self.seed,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        );
    }

    /// cloud_walk building `milestone`, run in the cloud by session_01Walk
    /// (the thread cse_01Walk) since a minute ago; its latest handoff,
    /// `handoff`, was written now, or never when it is None.
    fn cloud_run(&self, milestone: &str, handoff: Option<&str>) {
        self.cloud_run_by("session_01Walk", milestone, handoff);
    }

    /// cloud_run, with the run's runner id `runner`.
    fn cloud_run_by(&self, runner: &str, milestone: &str, handoff: Option<&str>) {
        let now = jiff::Timestamp::now();
        let since = now - jiff::SignedDuration::from_secs(60);
        let handoff_at = handoff.map_or(serde_json::Value::Null, |_| now.to_string().into());
        let projects = serde_json::json!({"projects": [
            {"name": "cloud_walk", "roadmap_status": "approved",
             "remote": REMOTE, "checkouts": self.checkouts("cloud_walk"),
             "milestone": milestone, "milestones_done": 1, "milestones_total": 3,
             "handoff_at": handoff_at,
             "runner": {"machine": "cloud", "id": runner,
                        "since": since.to_string(), "milestone": "w14"}},
            {"name": "shop", "milestone": "m2", "remote": REMOTE, "checkouts": []},
        ]});
        self.projects(projects);
        let body = serde_json::json!({ "body": handoff.unwrap_or_default() });
        std::fs::write(self.state.join("handoff.json"), body.to_string()).unwrap();
    }

    /// cloud_walk's question #AAAA1111 asking for `line`, answered
    /// `answer` from `machine`.
    fn asked(&self, line: &str, answer: &str, machine: &str) {
        let questions = serde_json::json!({"questions": [
            {"short_id": "AAAA1111", "project": "cloud_walk",
             "body": format!("May I run this on the mini?\n\n    {line}\n"),
             "answered": true, "answer": answer, "answer_id": "01ANSWER"},
        ]});
        std::fs::write(self.state.join("questions.json"), questions.to_string()).unwrap();
        let show = serde_json::json!({"items": [{"kind": "answer", "machine": machine}]});
        std::fs::write(self.state.join("show-01ANSWER.json"), show.to_string()).unwrap();
    }

    fn call(&self, project: &str, session: &str, args: &[&str]) -> serde_json::Value {
        let body = serde_json::json!({
            "verb": "tmux", "args": args, "project": project, "session": session,
        })
        .to_string();
        let response = self.hub.raw(&format!(
            "POST /device/here HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
             Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert_eq!(status_of(&response), 200, "{response}");
        serde_json::from_str(body_of(&response)).unwrap()
    }

    /// A call of cloud_walk's thread.
    fn walk(&self, args: &[&str]) -> serde_json::Value {
        self.call("cloud_walk", "cse_01Walk", args)
    }

    /// Reads the pane until a line of it is `line`, the command's output
    /// rather than the command as typed.
    fn read_until(&self, line: &str) -> String {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let answer = self.walk(&["read"]);
            let pane = answer["stdout"].as_str().unwrap_or_default().to_string();
            if pane.lines().any(|l| l.trim_end() == line) {
                return pane;
            }
            assert!(Instant::now() < until, "no line {line:?} in: {answer}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn sessions(&self) -> Vec<String> {
        let out = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.state.join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn a_sent_line_runs_and_its_output_reads_back() {
    let rig = rig();
    let sent = rig.walk(&["send", "echo hello-7"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.read_until("hello-7");
}

#[test]
fn the_session_is_named_for_the_project_and_milestone_and_works_in_its_own_folder() {
    let rig = rig();
    assert_eq!(rig.walk(&["send", "pwd"])["exit"], 0);
    assert_eq!(rig.sessions(), ["cloud-cloud_walk-w14"]);
    let folder = rig.home.join(".cache/workflow-cloud/cloud-spike");
    rig.read_until(folder.to_str().unwrap());
    // A worktree of the repo, at origin's cloud branch.
    rig.walk(&["send", "git rev-parse HEAD"]);
    rig.read_until(&rig.origin_sha("cloud/cloud_walk-w14"));
}

#[test]
fn making_the_worktree_leaves_saiful_s_checkout_as_it_was() {
    let rig = rig();
    std::fs::write(rig.checkout.join("apps/README"), "edited\n").unwrap();
    std::fs::write(rig.checkout.join("notes"), "mine\n").unwrap();
    let branch = git(&rig.checkout, &["branch", "--show-current"]);
    let status = git(&rig.checkout, &["status", "--porcelain"]);
    assert_eq!(status, "M apps/README\n?? notes");
    rig.walk(&["send", "echo made"]);
    rig.read_until("made");
    assert_eq!(git(&rig.checkout, &["branch", "--show-current"]), branch);
    assert_eq!(git(&rig.checkout, &["status", "--porcelain"]), status);
}

#[test]
fn a_later_session_keeps_the_folder_and_moves_to_the_branch_s_new_tip() {
    let rig = rig();
    rig.walk(&[
        "send",
        "mkdir -p node_modules/x && echo kept > node_modules/x/file",
    ]);
    rig.walk(&["send", "echo wrote"]);
    rig.read_until("wrote");
    assert_eq!(rig.walk(&["end"])["exit"], 0);
    rig.push("cloud/cloud_walk-w14", |seed| {
        std::fs::write(seed.join("apps/README"), "walk on\n").unwrap();
    });
    let sha = rig.origin_sha("cloud/cloud_walk-w14");
    rig.walk(&["send", "cat node_modules/x/file"]);
    rig.read_until("kept");
    rig.walk(&["send", "git rev-parse HEAD"]);
    rig.read_until(&sha);
    rig.walk(&["send", "pwd"]);
    rig.read_until(rig.worktree().to_str().unwrap());
}

#[test]
fn a_submodule_is_checked_out_by_the_session_s_first_line() {
    let rig = rig();
    let sub = rig.origin.with_file_name("sub");
    std::fs::create_dir_all(&sub).unwrap();
    git(&sub, &["init", "-q", "-b", "main"]);
    std::fs::write(sub.join("hello"), "from-sub\n").unwrap();
    git(&sub, &["add", "."]);
    git(&sub, &["commit", "-q", "-m", "sub"]);
    rig.push("cloud/cloud_walk-w14", |seed| {
        git(
            seed,
            &["submodule", "add", "-q", sub.to_str().unwrap(), "lib"],
        );
    });
    rig.walk(&["send", "cat lib/hello"]);
    rig.read_until("from-sub");
    assert!(rig.worktree().join("lib/hello").is_file());
}

#[test]
fn a_project_with_no_checkout_of_its_repo_is_refused() {
    let rig = rig();
    rig.projects(serde_json::json!({"projects": [
        {"name": "cloud_walk", "milestone": "w14", "remote": REMOTE, "checkouts": []},
    ]}));
    let answer = rig.walk(&["send", "true"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: no checkout of saifulapm/cloud-spike on mac-mini-m2\n"
    );
    assert!(rig.sessions().is_empty(), "{:?}", rig.sessions());
}

#[test]
fn end_kills_the_session() {
    let rig = rig();
    assert_eq!(rig.walk(&["send", "true"])["exit"], 0);
    assert_eq!(rig.sessions().len(), 1);
    let ended = rig.walk(&["end"]);
    assert_eq!(ended["exit"], 0, "{ended}");
    assert!(rig.sessions().is_empty(), "{:?}", rig.sessions());
}

#[test]
fn each_send_leaves_one_line_in_the_project_s_log() {
    let rig = rig();
    rig.walk(&["send", "echo hello-7"]);
    assert_eq!(
        rig.log(),
        ["--project=cloud_walk device mac-mini-m2 tmux send echo hello-7 exit 0"]
    );
}

#[test]
fn the_session_runs_bash() {
    let rig = rig();
    rig.walk(&["send", "echo shell-$0"]);
    rig.read_until("shell-/usr/bin/bash");
}

#[test]
fn a_session_does_not_match_another_by_its_prefix() {
    let rig = rig();
    let made = Command::new("tmux")
        .arg("-S")
        .arg(&rig.socket)
        .args(["new-session", "-d", "-s", "cloud-cloud_walk-w1"])
        .status()
        .unwrap();
    assert!(made.success());
    let read = rig.walk(&["read"]);
    assert_eq!(read["exit"], 2, "{read}");
    assert_eq!(rig.sessions(), ["cloud-cloud_walk-w1"]);
}

#[test]
fn a_key_name_is_sent_as_the_key() {
    let rig = rig();
    rig.walk(&["send", "echo ready"]);
    rig.read_until("ready");
    rig.walk(&["send", "sleep 30"]);
    std::thread::sleep(Duration::from_millis(300));
    rig.walk(&["send", "C-c"]);
    rig.walk(&["send", "echo after-c-c"]);
    rig.read_until("after-c-c");
}

#[test]
fn a_project_with_no_open_milestone_is_refused() {
    let rig = rig();
    let answer = rig.call("nothing", "cse_01Other", &["send", "true"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert!(rig.sessions().is_empty());
}

#[test]
fn a_second_project_finds_the_device_busy_until_the_first_job_ends() {
    let rig = rig();
    assert_eq!(rig.walk(&["send", "true"])["exit"], 0);
    let busy = rig.call("shop", "cse_01Shop", &["send", "true"]);
    assert_eq!(busy["exit"], 5, "{busy}");
    assert_eq!(busy["stdout"], "busy: cloud_walk\n");
    assert_eq!(rig.sessions(), ["cloud-cloud_walk-w14"]);
    rig.walk(&["end"]);
    let sent = rig.call("shop", "cse_01Shop", &["send", "true"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    assert_eq!(rig.sessions(), ["cloud-shop-m2"]);
}

#[test]
fn an_idle_job_is_ended() {
    let rig = rig_env(&[("HUB_DEVICE_IDLE_MS", "1000")]);
    rig.walk(&["send", "true"]);
    wait_until(Duration::from_secs(5), || rig.sessions().is_empty());
    let sent = rig.call("shop", "cse_01Shop", &["send", "true"]);
    assert_eq!(sent["exit"], 0, "{sent}");
}

#[test]
fn a_job_in_use_ends_at_the_cap_and_the_thread_is_told_once() {
    let rig = rig_env(&[("HUB_DEVICE_JOB_CAP_MS", "2000")]);
    let until = Instant::now() + Duration::from_secs(6);
    let told = loop {
        let answer = rig.walk(&["send", "true"]);
        if answer["exit"] != 0 {
            break answer;
        }
        assert!(Instant::now() < until, "the job never hit its cap");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(told["exit"], 2, "{told}");
    assert_eq!(
        told["stderr"],
        "device: the job ended at the 20-minute cap\n"
    );
    assert!(rig.sessions().is_empty(), "{:?}", rig.sessions());
    let again = rig.walk(&["send", "true"]);
    assert_eq!(again["exit"], 0, "{again}");
}

#[test]
fn the_cap_ends_a_job_with_no_call_coming() {
    let rig = rig_env(&[
        ("HUB_DEVICE_JOB_CAP_MS", "1000"),
        ("HUB_DEVICE_SWEEP_MS", "200"),
    ]);
    rig.walk(&["send", "true"]);
    wait_until(Duration::from_secs(5), || rig.sessions().is_empty());
    let told = rig.walk(&["read"]);
    assert_eq!(
        told["stderr"], "device: the job ended at the 20-minute cap\n",
        "{told}"
    );
}

#[test]
fn a_parked_run_s_job_is_ended() {
    let rig = rig_env(&[("HUB_DEVICE_SWEEP_MS", "200")]);
    rig.cloud_run("w14", None);
    rig.walk(&["send", "true"]);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(rig.sessions(), ["cloud-cloud_walk-w14"]);
    rig.cloud_run("w14", Some("parked: waiting on q1"));
    wait_until(Duration::from_secs(5), || rig.sessions().is_empty());
}

#[test]
fn a_landed_run_s_job_is_ended() {
    let rig = rig_env(&[("HUB_DEVICE_SWEEP_MS", "200")]);
    rig.cloud_run("w14", None);
    rig.walk(&["send", "true"]);
    rig.cloud_run("w15", None);
    wait_until(Duration::from_secs(5), || rig.sessions().is_empty());
}

fn wait_until(within: Duration, done: impl Fn() -> bool) {
    let until = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < until, "not done in {within:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

const HELD: [&str; 10] = [
    "rm -r scratch",
    "git push --force",
    "git push -f",
    "gh repo delete x",
    "cf dns records delete 1",
    "wrangler delete",
    "wrangler deploy",
    "shopify app release",
    "echo 'rm -r x'",
    "true && rm -r x",
];

#[test]
fn a_listed_line_is_held_and_never_reaches_the_pane() {
    let rig = rig();
    rig.walk(&["send", "echo ready"]);
    rig.read_until("ready");
    for line in HELD {
        let answer = rig.walk(&["send", line]);
        assert_eq!(answer["exit"], 6, "{line}: {answer}");
        assert_eq!(answer["stdout"], format!("ask: {line}\n"), "{line}");
    }
    rig.walk(&["send", "echo after"]);
    let pane = rig.read_until("after");
    assert!(!pane.contains("rm -r"), "{pane}");
    assert!(!pane.contains("git push"), "{pane}");
}

#[test]
fn a_line_off_the_list_is_sent() {
    let rig = rig();
    for line in [
        "wrangler deploy --env staging",
        "workflow shopify deploy shortcart",
        "gh repo view x",
        "C-c",
    ] {
        let answer = rig.walk(&["send", line]);
        assert_eq!(answer["exit"], 0, "{line}: {answer}");
    }
}

/// A scratch folder in the job's folder, made through the session.
fn scratch(rig: &Rig) -> PathBuf {
    rig.walk(&["send", "mkdir -p scratch && echo made"]);
    rig.read_until("made");
    let folder = rig.worktree().join("scratch");
    assert!(folder.is_dir());
    folder
}

#[test]
fn a_device_written_yes_sends_the_held_line_once() {
    let rig = rig();
    let folder = scratch(&rig);
    rig.asked("rm -r scratch", "yes", "mac-mini-m2");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.walk(&["send", "echo removed"]);
    rig.read_until("removed");
    assert!(!folder.exists());
    let again = rig.walk(&["send", "--yes", "#AAAA1111", "rm -r scratch"]);
    assert_eq!(again["exit"], 2, "{again}");
}

#[test]
fn a_yes_written_by_a_cloud_machine_is_refused() {
    let rig = rig();
    let folder = scratch(&rig);
    rig.asked("rm -r scratch", "yes", "cloud-abc");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert_eq!(
        sent["stderr"],
        "device: the yes for #AAAA1111 came from a cloud machine\n"
    );
    assert!(folder.is_dir());
}

#[test]
fn a_no_keeps_the_line_held() {
    let rig = rig();
    let folder = scratch(&rig);
    rig.asked("rm -r scratch", "No", "mac-mini-m2");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert!(folder.is_dir());
}

#[test]
fn a_yes_releases_only_the_line_it_asked_for() {
    let rig = rig();
    let folder = scratch(&rig);
    rig.asked("rm -r other", "yes", "mac-mini-m2");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert!(folder.is_dir());
}

#[test]
fn a_pane_s_tokens_read_back_cut() {
    let rig = rig();
    // The quotes keep the typed line from holding a whole token: only the
    // output does.
    rig.walk(&[
        "send",
        "echo shpat_\"\"0123456789abcdef0123456789abcdef cfut_\"\"abc123 shpss_\"\"def456 tskey-\"\"ghi789; echo printed",
    ]);
    let pane = rig.read_until("printed");
    for cut in ["shpat_***", "cfut_***", "shpss_***", "tskey-***"] {
        assert!(pane.contains(cut), "{cut} in {pane}");
    }
    for raw in ["shpat_0", "cfut_a", "shpss_d", "tskey-g"] {
        assert!(!pane.contains(raw), "{raw} in {pane}");
    }
}

#[test]
fn a_sent_token_is_cut_in_the_log() {
    let rig = rig();
    // Built here, so no token shape sits in the source for a secret scanner.
    let line = format!("echo shpat_{}", "0123456789abcdef".repeat(2));
    rig.walk(&["send", &line]);
    assert_eq!(
        rig.log(),
        ["--project=cloud_walk device mac-mini-m2 tmux send echo shpat_*** exit 0"]
    );
}

#[test]
fn a_line_that_continues_onto_the_next_send_is_refused() {
    let rig = rig();
    let answer = rig.walk(&["send", "rm \\"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert!(rig.sessions().is_empty(), "{:?}", rig.sessions());
}

#[test]
fn a_sent_line_cannot_be_run_again_from_the_shell_s_history() {
    let rig = rig();
    rig.walk(&["send", "echo once >> count"]);
    rig.walk(&["send", "Up"]);
    rig.walk(&["send", "Enter"]);
    rig.walk(&["send", "!!"]);
    rig.walk(&["send", "echo lines-$(wc -l < count)"]);
    rig.read_until("lines-1");
}

#[test]
fn the_session_holds_none_of_the_hub_s_secrets() {
    let rig = rig();
    rig.walk(&["send", "echo token-${HUB_PEER_TOKEN:-none}"]);
    rig.read_until("token-none");
}

#[test]
fn a_job_ends_once_another_session_runs_the_project() {
    let rig = rig_env(&[("HUB_DEVICE_SWEEP_MS", "200")]);
    rig.cloud_run("w14", None);
    rig.walk(&["send", "true"]);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(rig.sessions(), ["cloud-cloud_walk-w14"]);
    rig.cloud_run_by("session_01Next", "w14", None);
    wait_until(Duration::from_secs(5), || rig.sessions().is_empty());
}

/// The checkout's own list of its worktrees, by path.
fn worktrees_of(checkout: &Path) -> String {
    git(checkout, &["worktree", "list", "--porcelain"])
}

#[test]
fn a_worktree_unused_past_the_limit_is_removed() {
    let rig = rig_env(&[
        ("HUB_DEVICE_WORKTREE_UNUSED_MS", "1000"),
        ("HUB_DEVICE_SWEEP_MS", "200"),
    ]);
    rig.walk(&["send", "echo made"]);
    rig.read_until("made");
    let folder = rig.worktree();
    // In use: a job holds it, past the limit too.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(folder.is_dir(), "a live job's worktree stays");
    assert_eq!(rig.walk(&["end"])["exit"], 0);
    wait_until(Duration::from_secs(5), || !folder.exists());
    assert!(
        !worktrees_of(&rig.checkout).contains(folder.to_str().unwrap()),
        "{}",
        worktrees_of(&rig.checkout)
    );
    // The next session makes it again.
    rig.walk(&["send", "echo again"]);
    rig.read_until("again");
    assert!(folder.is_dir());
}

#[test]
fn a_worktree_kept_in_use_stays_by_default() {
    let rig = rig_env(&[("HUB_DEVICE_SWEEP_MS", "200")]);
    rig.walk(&["send", "echo made"]);
    rig.read_until("made");
    assert_eq!(rig.walk(&["end"])["exit"], 0);
    std::thread::sleep(Duration::from_millis(1000));
    assert!(rig.worktree().is_dir(), "14 days by default");
}

#[test]
fn a_worktree_folder_deleted_by_hand_is_made_again() {
    let rig = rig();
    rig.walk(&["send", "echo made"]);
    rig.read_until("made");
    assert_eq!(rig.walk(&["end"])["exit"], 0);
    std::fs::remove_dir_all(rig.worktree()).unwrap();
    let sent = rig.walk(&["send", "echo again"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.read_until("again");
}
