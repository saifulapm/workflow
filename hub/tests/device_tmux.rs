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
    /// The fakes the hub finds on its PATH, for a restart.
    bin: PathBuf,
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
    // The user's PATH is the state's `path` file once a test writes one.
    fixture_bin(
        &bin,
        "systemctl",
        &format!(
            "echo \"PATH=$(cat '{}/path' 2>/dev/null || echo /usr/bin:/bin)\"\n\
             echo 'GIT_CONFIG_COUNT=1'\n\
             echo 'GIT_CONFIG_KEY_0=protocol.file.allow'\n\
             echo 'GIT_CONFIG_VALUE_0=always'",
            state.display()
        ),
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
             *' ask --for '*) printf '%s|%s\\n' \"${{AMX_ID:-none}}\" \"$*\" >> \"$S/asks\"; echo '#QQQQ0001' ;;\n\
             *' tell -- '*) printf '%s\\n' \"$*\" >> \"$S/tells\" ;;\n\
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
        bin,
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
    /// A new hub on the same home, state and tmux server, the old one
    /// stopped once the new one is up.
    fn restart(&mut self) {
        let local = self._dir.join("local-again.sock");
        let env = [
            ("HUB_PEER_TOKEN", TOKEN),
            ("HUB_LOCAL_SOCKET", local.to_str().unwrap()),
            ("HUB_DEVICE_TMUX_SOCKET", self.socket.to_str().unwrap()),
        ];
        self.hub = Hub::spawn_env(&self.home, &[&self.bin], &["local", "--port", "0"], &env);
    }

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

    /// Saiful's checkout names the submodules the branch's .gitmodules does.
    fn same_gitmodules(&self) {
        std::fs::copy(
            self.seed.join(".gitmodules"),
            self.checkout.join(".gitmodules"),
        )
        .unwrap();
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

    /// cloud_walk's question #AAAA1111 asking for `line`, asked by the
    /// mini and answered `answer` from `machine`.
    fn asked(&self, line: &str, answer: &str, machine: &str) {
        self.asked_by("mac-mini-m2", line, answer, machine);
    }

    /// `asked`, of a question `asker` wrote.
    fn asked_by(&self, asker: &str, line: &str, answer: &str, machine: &str) {
        let body = format!(
            "A cloud_walk thread asks to run this line in its tmux session on {asker}:\n\n    {line}\n"
        );
        self.asked_with(asker, &body, &[], answer, machine);
    }

    /// cloud_walk's question #AAAA1111, `body` with `options`, asked by
    /// `asker` and answered `answer` from `machine`.
    fn asked_with(&self, asker: &str, body: &str, options: &[&str], answer: &str, machine: &str) {
        let questions = serde_json::json!({"questions": [
            {"short_id": "AAAA1111", "project": "cloud_walk", "machine": asker,
             "options": options, "body": body,
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
    rig.same_gitmodules();
    rig.walk(&["send", "cat lib/hello"]);
    rig.read_until("from-sub");
    assert!(rig.worktree().join("lib/hello").is_file());
}

/// The hub runs git in the worktree outside the wall, so no git file a
/// session could rewrite may steer it: a submodule's .git file planted in
/// the wall is made again before the next session's update.
#[test]
fn a_submodule_s_git_file_rewritten_in_the_wall_is_made_again() {
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
    rig.same_gitmodules();
    rig.walk(&["send", "cat lib/hello"]);
    rig.read_until("from-sub");
    rig.walk(&[
        "send",
        "printf 'gitdir: /nowhere\\n' > lib/.git && echo planted",
    ]);
    rig.read_until("planted");
    rig.walk(&["end"]);
    rig.walk(&["send", "cat lib/hello; echo again"]);
    let pane = rig.read_until("again");
    assert!(pane.contains("from-sub"), "{pane}");
    let file = std::fs::read_to_string(rig.worktree().join("lib/.git")).unwrap();
    assert!(!file.contains("/nowhere"), "{file}");
}

#[test]
fn the_worktree_s_own_git_files_are_read_only_in_the_wall() {
    let rig = rig();
    rig.walk(&[
        "send",
        "echo 'gitdir: /nowhere' > .git; echo /nowhere > $(git rev-parse --git-dir)/commondir; echo tried",
    ]);
    let pane = rig.read_until("tried");
    assert_eq!(pane.matches("Read-only file system").count(), 2, "{pane}");
    let file = std::fs::read_to_string(rig.worktree().join(".git")).unwrap();
    assert!(
        file.starts_with("gitdir: ") && !file.contains("/nowhere"),
        "{file}"
    );
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
        "device: no checkout of saifulapm/cloud-spike on mac-mini-m2 that mem knows: \
         clone it there if it has none, then run `mem project current` in the clone \
         so mem records it\n"
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
        assert_eq!(
            answer["stdout"],
            format!("ask: {line}\nasked: #QQQQ0001\n"),
            "{line}"
        );
    }
    rig.walk(&["send", "echo after"]);
    let pane = rig.read_until("after");
    assert!(!pane.contains("rm -r"), "{pane}");
    assert!(!pane.contains("git push"), "{pane}");
}

impl Rig {
    /// cloud_walk's stage and the machine that set it, its w14 run live in
    /// the cloud as session_01Walk (the thread cse_01Walk), with shop in
    /// development, set by the mini.
    fn stage(&self, stage: &str, by: &str) {
        let since = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(60);
        let row = |name: &str, stage: &str, by: &str, milestone: &str| {
            serde_json::json!({"name": name, "milestone": milestone, "remote": REMOTE,
                               "checkouts": self.checkouts(name),
                               "stage": stage, "stage_by": by})
        };
        let mut walk = row("cloud_walk", stage, by, "w14");
        walk["roadmap_status"] = "approved".into();
        walk["milestones_done"] = 1.into();
        walk["milestones_total"] = 3.into();
        walk["handoff_at"] = serde_json::Value::Null;
        walk["runner"] = serde_json::json!({"machine": "cloud", "id": "session_01Walk",
                                            "since": since.to_string(), "milestone": "w14"});
        self.projects(serde_json::json!({"projects": [
            walk,
            row("shop", "development", "mac-mini-m2", "m2"),
        ]}));
        let body = serde_json::json!({ "body": "" });
        std::fs::write(self.state.join("handoff.json"), body.to_string()).unwrap();
    }
}

/// A thread names its project in each call, so only the project's own live
/// run gets its development stage: another session starting a job under
/// that name has its deploy line held.
#[test]
fn a_deploy_line_is_held_for_a_session_that_is_not_the_project_s_run() {
    let rig = rig();
    rig.stage("development", "mac-mini-m2");
    let answer = rig.call(
        "cloud_walk",
        "cse_01Other",
        &["send", "npx wrangler deploy"],
    );
    assert_eq!(answer["exit"], 6, "{answer}");
    assert_eq!(
        answer["stdout"],
        "ask: npx wrangler deploy\nasked: #QQQQ0001\n"
    );
}

#[test]
fn a_deploy_line_runs_unasked_in_a_project_a_device_put_in_development() {
    let rig = rig();
    rig.stage("development", "mac-mini-m2");
    let tools = rig.home.join(".local/bin");
    common::fixture_bin(&tools, "npx", "echo \"ran $*\"");
    std::fs::write(
        rig.state.join("path"),
        format!("{}:/usr/bin:/bin", tools.display()),
    )
    .unwrap();
    let answer = rig.walk(&["send", "npx wrangler deploy"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    rig.read_until("ran wrangler deploy");
    assert_eq!(asks(&rig), "");
}

#[test]
fn a_deploy_line_is_held_in_a_live_project_while_another_is_in_development() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let answer = rig.walk(&["send", "npx wrangler deploy"]);
    assert_eq!(answer["exit"], 6, "{answer}");
    assert_eq!(
        answer["stdout"],
        "ask: npx wrangler deploy\nasked: #QQQQ0001\n"
    );
}

/// Only a device's word puts a project in development: a stage a cloud
/// machine wrote releases nothing.
#[test]
fn a_deploy_line_is_held_when_no_device_set_the_stage() {
    let rig = rig();
    rig.stage("development", "cloud-vm");
    let answer = rig.walk(&["send", "npx wrangler deploy"]);
    assert_eq!(answer["exit"], 6, "{answer}");
}

/// A session's job is its one project's: a call naming another project
/// takes neither that project's stage nor its job.
#[test]
fn a_call_naming_another_project_reaches_no_job_of_the_session() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    rig.walk(&["send", "echo ready"]);
    rig.read_until("ready");
    let answer = rig.call("shop", "cse_01Walk", &["send", "npx wrangler deploy"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: this session's job is cloud_walk's, not shop's\n"
    );
    rig.walk(&["send", "echo after"]);
    let pane = rig.read_until("after");
    assert!(!pane.contains("npx wrangler"), "{pane}");
}

#[test]
fn development_still_holds_a_delete_and_whatever_rides_on_a_deploy() {
    let rig = rig();
    rig.stage("development", "mac-mini-m2");
    for line in [
        "wrangler delete",
        "wrangler deploy && wrangler delete",
        "wrangler deploy; rm -r scratch",
        "npx wrangler deploy && mem answer AAAA1111 yes",
        "shopify app deploy",
    ] {
        let answer = rig.walk(&["send", line]);
        assert_eq!(answer["exit"], 6, "{line}: {answer}");
    }
}

/// What the device asked with `mem ask`: for each question, the AMX_ID
/// it ran with, `|`, then its arguments and a newline.
fn asks(rig: &Rig) -> String {
    std::fs::read_to_string(rig.state.join("asks")).unwrap_or_default()
}

#[test]
fn the_device_asks_saiful_a_held_line_s_question_itself() {
    let rig = rig();
    let answer = rig.walk(&["send", "rm -r scratch"]);
    assert_eq!(answer["exit"], 6, "{answer}");
    assert_eq!(answer["stdout"], "ask: rm -r scratch\nasked: #QQQQ0001\n");
    let asks = asks(&rig);
    assert_eq!(asks.matches("|--project").count(), 1, "{asks}");
    // No session, so the hub wakes the parked thread on his answer.
    assert!(
        asks.starts_with("none|--project cloud_walk ask --for saiful "),
        "{asks}"
    );
    assert!(asks.contains("\n    rm -r scratch\n"), "{asks}");
    assert!(asks.ends_with(" --option yes --option no\n"), "{asks}");
}

#[test]
fn a_held_line_sent_again_names_the_question_already_open() {
    let rig = rig();
    rig.asked("rm -r scratch", "", "mac-mini-m2");
    let questions = rig.state.join("questions.json");
    let open = std::fs::read_to_string(&questions)
        .unwrap()
        .replace("\"answered\":true", "\"answered\":false");
    std::fs::write(&questions, open).unwrap();
    let answer = rig.walk(&["send", "rm -r scratch"]);
    assert_eq!(answer["exit"], 6, "{answer}");
    assert_eq!(answer["stdout"], "ask: rm -r scratch\nasked: #AAAA1111\n");
    assert!(asks(&rig).is_empty());
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
    rig.asked("rm -r scratch", "yes", "cloud-vm");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert_eq!(
        sent["stderr"],
        "device: the yes for #AAAA1111 came from cloud-vm, not from a device or the hub page\n"
    );
    assert!(folder.is_dir());
}

#[test]
fn a_yes_to_a_question_a_cloud_machine_wrote_releases_nothing() {
    let rig = rig();
    let folder = scratch(&rig);
    rig.asked_by("cloud-vm", "rm -r scratch", "yes", "mac-mini-m2");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert_eq!(
        sent["stderr"],
        "device: #AAAA1111 was asked by cloud-vm, not by a device\n"
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
fn a_hub_page_yes_sends_the_held_line_once() {
    let rig = rig();
    let folder = scratch(&rig);
    rig.asked("rm -r scratch", "Yes", "hub-web");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.walk(&["send", "echo removed"]);
    rig.read_until("removed");
    assert!(!folder.exists());
    let again = rig.walk(&["send", "--yes", "#AAAA1111", "rm -r scratch"]);
    assert_eq!(again["exit"], 2, "{again}");
}

#[test]
fn a_spent_hub_page_yes_stays_spent_after_a_restart() {
    let mut rig = rig();
    let folder = scratch(&rig);
    rig.asked("rm -r scratch", "Yes", "hub-web");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.walk(&["send", "echo removed"]);
    rig.read_until("removed");
    assert!(!folder.exists());
    rig.restart();
    std::fs::create_dir_all(&folder).unwrap();
    let again = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
    assert_eq!(again["exit"], 2, "{again}");
    assert_eq!(
        again["stderr"],
        "device: #AAAA1111 already sent its line; ask again\n"
    );
    assert!(folder.is_dir());
}

#[test]
fn a_hub_page_no_keeps_the_line_held() {
    let rig = rig();
    let folder = scratch(&rig);
    for answer in ["No", "hold"] {
        rig.asked("rm -r scratch", answer, "hub-web");
        let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
        assert_eq!(sent["exit"], 2, "{answer}: {sent}");
        assert!(folder.is_dir(), "{answer}");
    }
}

#[test]
fn a_hub_page_yes_to_a_question_a_cloud_machine_wrote_releases_nothing() {
    let rig = rig();
    let folder = scratch(&rig);
    // The hub page answers; it never asks, so a question it names is no
    // device's either.
    for asker in ["cloud-vm", "hub-web"] {
        rig.asked_by(asker, "rm -r scratch", "Yes", "hub-web");
        let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
        assert_eq!(sent["exit"], 2, "{sent}");
        assert_eq!(
            sent["stderr"],
            format!("device: #AAAA1111 was asked by {asker}, not by a device\n")
        );
        assert!(folder.is_dir());
    }
}

/// The claude.ai connector and the hub sandbox sign as themselves, and a
/// thread reaches the connector: neither is the hub page.
#[test]
fn a_yes_through_the_connector_or_with_no_machine_is_refused() {
    let rig = rig();
    let folder = scratch(&rig);
    for (machine, named) in [
        ("connector", "connector"),
        ("hub", "hub"),
        ("", "no machine"),
    ] {
        rig.asked("rm -r scratch", "yes", machine);
        if machine.is_empty() {
            let show = serde_json::json!({"items": [{"kind": "answer"}]});
            std::fs::write(rig.state.join("show-01ANSWER.json"), show.to_string()).unwrap();
        }
        let sent = rig.walk(&["send", "--yes", "AAAA1111", "rm -r scratch"]);
        assert_eq!(sent["exit"], 2, "{machine}: {sent}");
        assert_eq!(
            sent["stderr"],
            format!(
                "device: the yes for #AAAA1111 came from {named}, not from a device or the hub page\n"
            )
        );
        assert!(folder.is_dir(), "{machine}");
    }
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

#[test]
fn a_missing_worktree_of_saiful_s_own_stays_on_his_list() {
    let rig = rig();
    let other = rig.checkout.with_file_name("his-other");
    git(
        &rig.checkout,
        &["worktree", "add", "-q", other.to_str().unwrap()],
    );
    // On a disk that is not mounted just now.
    let away = rig.checkout.with_file_name("his-other-away");
    std::fs::rename(&other, &away).unwrap();
    rig.walk(&["send", "echo made"]);
    rig.read_until("made");
    assert_eq!(rig.walk(&["end"])["exit"], 0);
    std::fs::remove_dir_all(rig.worktree()).unwrap();
    rig.walk(&["send", "echo again"]);
    rig.read_until("again");
    assert!(
        worktrees_of(&rig.checkout).contains(other.to_str().unwrap()),
        "{}",
        worktrees_of(&rig.checkout)
    );
}

/// Saiful's secrets, his other folders and mem's store, planted in the
/// home: none is in the wall.
#[test]
fn a_walled_session_sees_none_of_saiful_s_secrets() {
    let rig = rig();
    let hidden = [
        ".password-store",
        ".ssh",
        ".gnupg",
        ".config/gh",
        ".config/hub",
        ".claude",
        "Documents",
        ".local/share/mem/store",
        ".local/state/mem/share",
    ];
    for dir in hidden {
        std::fs::create_dir_all(rig.home.join(dir)).unwrap();
        std::fs::write(rig.home.join(dir).join("planted"), "secret\n").unwrap();
    }
    std::fs::write(
        rig.home.join(".npmrc"),
        "//registry.npmjs.org/:_authToken=x\n",
    )
    .unwrap();
    let home = rig.home.display();
    let mut line = String::new();
    for dir in hidden {
        line.push_str(&format!("ls {home}/{dir}; "));
    }
    line.push_str(&format!(
        "cat {home}/.npmrc; cat {home}/Documents/planted; echo listed"
    ));
    rig.walk(&["send", &line]);
    let pane = rig.read_until("listed");
    for dir in hidden {
        let said = format!("ls: cannot access '{home}/{dir}': No such file or directory");
        assert!(pane.contains(&said), "{said}\n{pane}");
    }
    assert!(
        pane.contains(&format!("cat: {home}/.npmrc: No such file or directory")),
        "{pane}"
    );
    assert!(
        !pane.contains("secret") && !pane.contains("_authToken"),
        "{pane}"
    );
}

#[test]
fn a_walled_session_reaches_no_agent_socket_or_session_bus() {
    let rig = rig();
    rig.walk(&[
        "send",
        "gpg-connect-agent /bye; echo gpg-$?; tmux ls; echo tmux-$?; \
         systemd-run --user true; echo run-$?; ls /run/user; echo done",
    ]);
    let pane = rig.read_until("done");
    for code in ["gpg-0", "tmux-0", "run-0"] {
        assert!(
            !pane.lines().any(|l| l.trim_end() == code),
            "{code}\n{pane}"
        );
    }
    assert!(
        pane.contains("ls: cannot access '/run/user': No such file or directory"),
        "{pane}"
    );
}

#[test]
fn the_checkout_s_git_is_read_only_and_the_worktree_s_git_works() {
    let rig = rig();
    let hooks = rig.checkout.join(".git/hooks");
    rig.walk(&[
        "send",
        &format!(
            "touch {}/planted; git status > /dev/null; echo status-$?",
            hooks.display()
        ),
    ]);
    let pane = rig.read_until("status-0");
    assert!(pane.contains("Read-only file system"), "{pane}");
    assert!(!hooks.join("planted").exists());
}

#[test]
fn a_walled_session_runs_as_the_device_s_user() {
    let rig = rig();
    let me = Command::new("id").arg("-un").output().unwrap();
    let me = String::from_utf8_lossy(&me.stdout).trim().to_string();
    rig.walk(&["send", "echo user-$(id -un)"]);
    rig.read_until(&format!("user-{me}"));
}

/// Whether a process whose command line is `marker` is still running.
fn running(marker: &str) -> bool {
    std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        std::fs::read(entry.path().join("cmdline"))
            .is_ok_and(|cmd| String::from_utf8_lossy(&cmd).replace('\0', " ").trim() == marker)
    })
}

#[test]
fn no_process_of_a_walled_session_outlives_its_end() {
    let rig = rig();
    // Seconds no other run of this test sleeps for.
    let (first, second) = (
        format!("sleep {}1", std::process::id()),
        format!("sleep {}2", std::process::id()),
    );
    rig.walk(&[
        "send",
        &format!("setsid {first} & (nohup {second} > /dev/null &); echo started"),
    ]);
    rig.read_until("started");
    wait_until(Duration::from_secs(5), || {
        running(&first) && running(&second)
    });
    rig.walk(&["end"]);
    wait_until(Duration::from_secs(5), || {
        !running(&first) && !running(&second)
    });
}

/// Whether the walled session can open a TCP connection to `host`:`port`.
fn reaches(rig: &Rig, host: &str, port: u16) -> bool {
    let tag = format!("{}-{port}", host.replace(['.', ':'], "_"));
    rig.walk(&[
        "send",
        &format!(
            "timeout 2 bash -c 'exec 3<>/dev/tcp/{host}/{port}' 2>/dev/null \
             && echo open-{tag} || echo shut-{tag}"
        ),
    ]);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let pane = rig.walk(&["read"])["stdout"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for (word, open) in [("open", true), ("shut", false)] {
            if pane
                .lines()
                .any(|l| l.trim_end() == format!("{word}-{tag}"))
            {
                return open;
            }
        }
        assert!(
            Instant::now() < until,
            "no answer for {host}:{port} in: {pane}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_walled_session_reaches_the_internet() {
    let rig = rig();
    rig.walk(&[
        "send",
        "echo code-$(curl -s -m 20 -o /dev/null -w '%{http_code}' https://api.cloudflare.com/client/v4/)",
    ]);
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        let pane = rig.walk(&["read"])["stdout"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let code = pane
            .lines()
            .filter_map(|l| l.trim_end().strip_prefix("code-"))
            .find(|c| c.len() == 3);
        if let Some(code) = code {
            assert_ne!(code, "000", "{pane}");
            return;
        }
        assert!(Instant::now() < until, "{pane}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Of the device, a walled session reaches adb's port and nothing else:
/// not the hub, not another loopback port, not the device's own addresses.
#[test]
fn of_the_device_a_walled_session_reaches_only_adb_s_port() {
    let rig = rig();
    // A running adb server is a listener on 5037 too.
    let _adb = std::net::TcpListener::bind("127.0.0.1:5037");
    let other = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = other.local_addr().unwrap().port();
    assert!(reaches(&rig, "127.0.0.1", 5037));
    assert!(!reaches(&rig, "127.0.0.1", rig.hub.port));
    assert!(!reaches(&rig, "127.0.0.1", port));
    let addrs = Command::new("ip")
        .args(["-4", "-o", "addr", "show"])
        .output()
        .unwrap();
    for addr in String::from_utf8_lossy(&addrs.stdout)
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .nth(3)?
                .split('/')
                .next()
                .map(str::to_string)
        })
        .filter(|a| !a.starts_with("127."))
    {
        assert!(!reaches(&rig, &addr, port), "{addr}:{port}");
    }
}

/// A no-wall question as the device words it, for `line`.
fn outside(line: &str) -> String {
    format!(
        "A cloud_walk thread asks to run this line outside the wall on mac-mini-m2:\n\n    {line}\n"
    )
}

#[test]
fn a_no_wall_line_is_asked_with_its_exact_text() {
    let rig = rig();
    let answer = rig.walk(&["send", "--no-wall", "gh", "auth", "status"]);
    assert_eq!(answer["exit"], 6, "{answer}");
    assert_eq!(
        answer["stdout"],
        "ask: gh auth status (outside the wall)\nasked: #QQQQ0001\n"
    );
    let asks = asks(&rig);
    assert!(asks.contains("\n    gh auth status\n"), "{asks}");
    assert!(asks.contains("outside the wall"), "{asks}");
    assert!(asks.ends_with(" --option yes --option no\n"), "{asks}");
}

#[test]
fn a_tag_push_s_question_says_its_yes_covers_three_re_pushes_for_a_day() {
    let rig = rig();
    let line = "git tag -f v1 && git push -f origin refs/tags/v1";
    let answer = rig.walk(&["send", "--no-wall", line]);
    assert_eq!(answer["exit"], 6, "{answer}");
    let asks = asks(&rig);
    assert!(asks.contains(&format!("\n    {line}\n")), "{asks}");
    assert!(
        asks.contains(
            "Yes runs it now, and this same line up to 3 more times within 24 hours of your yes \
             to push v1 again; Slack shows each re-push."
        ),
        "{asks}"
    );
    rig.walk(&["send", "--no-wall", "git push origin main"]);
    let other = asks.len();
    let asks = std::fs::read_to_string(rig.state.join("asks")).unwrap();
    assert!(!asks[other..].contains("re-push"), "{asks}");
}

/// A release line the tests send: it tags the worktree's HEAD and pushes
/// the tag to the rig's origin.
fn release(tag: &str) -> String {
    format!("git tag -f {tag} && git push -q -f origin refs/tags/{tag}")
}

/// A ULID minted `ago` before now, as mem mints an answer's id.
fn ulid_ago(ago: Duration) -> String {
    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let ms = (jiff::Timestamp::now().as_millisecond() - ago.as_millis() as i64) as u64;
    let time: String = (0..10)
        .rev()
        .map(|i| CROCKFORD[((ms >> (5 * i)) & 31) as usize] as char)
        .collect();
    format!("{time}NQV75E7DEAZJZJGS")
}

/// Question #AAAA1111 for `line`, a release line of `tag`, in the device's
/// words, answered yes on the mini `ago` before now.
fn release_yes(rig: &Rig, tag: &str, line: &str, ago: Duration) {
    let body = format!(
        "{}\nIt runs with your logins, in the thread's worktree. Yes runs it now, and this same \
         line up to 3 more times within 24 hours of your yes to push {tag} again; Slack shows \
         each re-push. The next line is walled again.",
        outside(line)
    );
    rig.asked_with("mac-mini-m2", &body, &["yes", "no"], "yes", "mac-mini-m2");
    let answer = ulid_ago(ago);
    let questions = std::fs::read_to_string(rig.state.join("questions.json"))
        .unwrap()
        .replace("01ANSWER", &answer);
    std::fs::write(rig.state.join("questions.json"), questions).unwrap();
    std::fs::rename(
        rig.state.join("show-01ANSWER.json"),
        rig.state.join(format!("show-{answer}.json")),
    )
    .unwrap();
}

fn send_release(rig: &Rig, line: &str) -> serde_json::Value {
    rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", line])
}

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn a_tag_push_s_yes_runs_the_same_line_three_more_times_and_no_fourth() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    release_yes(&rig, "v1", &line, HOUR);
    for n in 0..4 {
        let sent = send_release(&rig, &line);
        assert_eq!(sent["exit"], 0, "push {n}: {sent}");
    }
    let fifth = send_release(&rig, &line);
    assert_eq!(fifth["exit"], 2, "{fifth}");
    assert_eq!(
        fifth["stderr"],
        "device: #AAAA1111 already re-pushed v1 3 times; ask again\n"
    );
    assert_eq!(asks(&rig), "");
}

#[test]
fn a_tag_push_s_yes_covers_no_other_line_that_names_the_tag() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    release_yes(&rig, "v1", &line, HOUR);
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    for other in [
        release("v2"),
        "git commit -m \"prepare v1\"".to_string(),
        "git push -q -f origin refs/tags/v1".to_string(),
    ] {
        let sent = send_release(&rig, &other);
        assert_eq!(sent["exit"], 2, "{other}: {sent}");
        assert_eq!(
            sent["stderr"],
            format!("device: #AAAA1111 does not ask for {other}\n")
        );
    }
}

/// What the device told Saiful with `mem tell`, one call a line.
fn tells(rig: &Rig) -> String {
    std::fs::read_to_string(rig.state.join("tells")).unwrap_or_default()
}

#[test]
fn each_re_push_tells_its_diff_since_the_yes_and_the_first_push_none() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    release_yes(&rig, "v1", &line, HOUR);
    let yes = rig.origin_sha("cloud/cloud_walk-w14");
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    assert_eq!(tells(&rig), "");
    // The thread's fix reaches the device with the job's next start.
    rig.walk(&["end"]);
    rig.push("cloud/cloud_walk-w14", |seed| {
        std::fs::write(seed.join("apps/README"), "walk\nfixed\nagain\n").unwrap();
    });
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    let short = &yes[..7];
    assert_eq!(
        tells(&rig),
        format!(
            "--project cloud_walk tell -- re-push 1 of 3 of v1: 1 file changed, 2 insertions, 0 deletions since {short}\n"
        )
    );
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    assert!(
        tells(&rig).ends_with(&format!(
            "re-push 2 of 3 of v1: 1 file changed, 2 insertions, 0 deletions since {short}\n"
        )),
        "{}",
        tells(&rig)
    );
}

/// The day counts from Saiful's yes: a first push late in it leaves the
/// re-pushes no more time.
#[test]
fn a_re_push_past_a_day_from_the_yes_runs_nothing() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    release_yes(&rig, "v1", &line, 24 * HOUR + Duration::from_secs(60));
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    let again = send_release(&rig, &line);
    assert_eq!(again["exit"], 2, "{again}");
    assert_eq!(
        again["stderr"],
        "device: #AAAA1111 was given more than 24 hours ago; ask again\n"
    );
}

#[test]
fn a_re_push_just_inside_the_day_runs() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    release_yes(&rig, "v1", &line, 24 * HOUR - Duration::from_secs(60));
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    assert_eq!(send_release(&rig, &line)["exit"], 0);
}

/// A yes to a question that said "once" stays a yes for once, tag or not.
#[test]
fn a_yes_to_a_question_that_said_once_re_pushes_nothing() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    let body = format!(
        "{}\nIt runs with your logins, in the thread's worktree. Yes runs it there once; the next line is walled again.",
        outside(&line)
    );
    rig.asked_with("mac-mini-m2", &body, &["yes", "no"], "yes", "mac-mini-m2");
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    let again = send_release(&rig, &line);
    assert_eq!(again["exit"], 2, "{again}");
    assert_eq!(
        again["stderr"],
        "device: #AAAA1111 already sent its line; ask again\n"
    );
}

/// Only the project's own live run re-pushes on its yes: not another
/// session calling under its name, nor another project's thread.
#[test]
fn a_re_push_from_another_session_or_project_runs_nothing() {
    let rig = rig();
    rig.stage("live", "mac-mini-m2");
    let line = release("v1");
    release_yes(&rig, "v1", &line, HOUR);
    assert_eq!(send_release(&rig, &line)["exit"], 0);
    rig.walk(&["end"]);
    let args = ["send", "--yes", "AAAA1111", "--no-wall", line.as_str()];
    let other = rig.call("cloud_walk", "cse_01Other", &args);
    assert_eq!(other["exit"], 2, "{other}");
    assert_eq!(
        other["stderr"],
        "device: #AAAA1111 is cloud_walk's and this session is not its live run; ask again\n"
    );
    rig.call("cloud_walk", "cse_01Other", &["end"]);
    let shop = rig.call("shop", "cse_01Shop", &args);
    assert_eq!(shop["exit"], 2, "{shop}");
    assert_eq!(
        shop["stderr"],
        "device: #AAAA1111 is not a question of shop\n"
    );
    assert_eq!(send_release(&rig, &line)["exit"], 0);
}

#[test]
fn a_device_s_yes_runs_a_no_wall_line_outside_once_and_the_next_is_walled() {
    let rig = rig();
    let docs = rig.home.join("Documents");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("note"), "seen\n").unwrap();
    let line = format!("cat {}/note && echo outside-$(pwd)", docs.display());
    rig.asked_with(
        "mac-mini-m2",
        &outside(&line),
        &["yes", "no"],
        "yes",
        "mac-mini-m2",
    );
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", &line]);
    assert_eq!(sent["exit"], 0, "{sent}");
    let pane = rig.read_until(&format!("outside-{}", rig.worktree().display()));
    assert!(pane.contains("seen"), "{pane}");
    let again = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", &line]);
    assert_eq!(again["exit"], 2, "{again}");
    rig.walk(&[
        "send",
        &format!("cat {}/note || echo walled-again", docs.display()),
    ]);
    rig.read_until("walled-again");
}

#[test]
fn a_no_wall_yes_from_a_cloud_machine_runs_nothing() {
    let rig = rig();
    let line = "gh auth status";
    rig.asked_with(
        "mac-mini-m2",
        &outside(line),
        &["yes", "no"],
        "yes",
        "cloud-vm",
    );
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", line]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert!(rig.sessions().is_empty());
}

#[test]
fn a_held_line_s_yes_runs_nothing_outside_the_wall() {
    let rig = rig();
    rig.asked("rm -r scratch", "yes", "mac-mini-m2");
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", "rm -r scratch"]);
    assert_eq!(sent["exit"], 2, "{sent}");
}

/// A node on the user's PATH, in the home, that prints another version
/// behind the wall, which hides ~/Documents.
fn node_that_differs(rig: &Rig) {
    std::fs::create_dir_all(rig.home.join("Documents")).unwrap();
    let tools = rig.home.join(".local/bin");
    common::fixture_bin(
        &tools,
        "node",
        "if [ -d \"$HOME/Documents\" ]; then echo v24.21.0; else echo v24.18.0; fi",
    );
    std::fs::write(
        rig.state.join("path"),
        format!("{}:/usr/bin:/bin", tools.display()),
    )
    .unwrap();
}

#[test]
fn a_check_that_differs_is_asked_and_no_line_runs() {
    let rig = rig();
    node_that_differs(&rig);
    let answer = rig.walk(&["send", "echo hi"]);
    assert_eq!(answer["exit"], 6, "{answer}");
    assert_eq!(
        answer["stdout"],
        "ask: the wall cannot start on mac-mini-m2: node v24.18.0 inside, v24.21.0 outside\n\
         asked: #QQQQ0001\n"
    );
    let asks = asks(&rig);
    assert!(
        asks.ends_with(" --option run this job outside the wall --option hold\n"),
        "{asks}"
    );
    assert!(rig.sessions().is_empty());
}

/// A project's own cf, installed in the worktree and not in Saiful's
/// checkout, is no difference of the wall's: cf hands off to the cf the
/// folder's package.json finds, as Saiful's own shell does in an
/// installed checkout.
#[test]
fn a_tool_the_worktree_installed_for_itself_starts_the_job_unasked() {
    let rig = rig();
    let tools = rig.home.join(".local/bin");
    common::fixture_bin(
        &tools,
        "cf",
        "if [ -d node_modules/cf ]; then echo 1.0.0-beta.16; else echo 'cf v0.15.0'; fi",
    );
    std::fs::write(
        rig.state.join("path"),
        format!("{}:/usr/bin:/bin", tools.display()),
    )
    .unwrap();
    rig.walk(&["send", "echo first"]);
    rig.read_until("first");
    rig.walk(&["end"]);
    std::fs::create_dir_all(rig.worktree().join("node_modules/cf")).unwrap();
    let sent = rig.walk(&["send", "cf --version; echo second"]);
    assert_eq!(sent["exit"], 0, "{sent}");
    let pane = rig.read_until("second");
    assert!(pane.contains("1.0.0-beta.16"), "{pane}");
    assert_eq!(asks(&rig), "");
}

#[test]
fn a_device_s_answer_runs_the_job_outside_the_wall() {
    let rig = rig();
    node_that_differs(&rig);
    let body = "The wall cannot start on mac-mini-m2 for cloud_walk: node v24.18.0 inside, v24.21.0 outside\n";
    let options = ["run this job outside the wall", "hold"];
    rig.asked_with(
        "mac-mini-m2",
        body,
        &options,
        "run this job outside the wall",
        "cloud-vm",
    );
    let cloud = rig.walk(&["send", "--yes", "AAAA1111", "echo hi"]);
    assert_eq!(cloud["exit"], 2, "{cloud}");
    assert!(rig.sessions().is_empty());
    rig.asked_with(
        "mac-mini-m2",
        body,
        &options,
        "run this job outside the wall",
        "mac-mini-m2",
    );
    let sent = rig.walk(&[
        "send",
        "--yes",
        "AAAA1111",
        "test -d ~/Documents && echo unwalled",
    ]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.read_until("unwalled");
}

#[test]
fn a_hub_page_yes_runs_a_no_wall_line_outside_once() {
    let rig = rig();
    let docs = rig.home.join("Documents");
    std::fs::create_dir_all(&docs).unwrap();
    std::fs::write(docs.join("note"), "seen\n").unwrap();
    let line = format!("cat {}/note && echo outside-$(pwd)", docs.display());
    rig.asked_with(
        "mac-mini-m2",
        &outside(&line),
        &["yes", "no"],
        "Yes",
        "hub-web",
    );
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", &line]);
    assert_eq!(sent["exit"], 0, "{sent}");
    let pane = rig.read_until(&format!("outside-{}", rig.worktree().display()));
    assert!(pane.contains("seen"), "{pane}");
    let again = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", &line]);
    assert_eq!(again["exit"], 2, "{again}");
}

#[test]
fn a_hub_page_answer_runs_the_job_outside_the_wall_and_its_hold_runs_nothing() {
    let rig = rig();
    node_that_differs(&rig);
    let body = "The wall cannot start on mac-mini-m2 for cloud_walk: node v24.18.0 inside, v24.21.0 outside\n";
    let options = ["run this job outside the wall", "hold"];
    rig.asked_with("mac-mini-m2", body, &options, "hold", "hub-web");
    let held = rig.walk(&["send", "--yes", "AAAA1111", "echo hi"]);
    assert_eq!(held["exit"], 2, "{held}");
    assert!(rig.sessions().is_empty());
    rig.asked_with(
        "mac-mini-m2",
        body,
        &options,
        "run this job outside the wall",
        "hub-web",
    );
    let sent = rig.walk(&[
        "send",
        "--yes",
        "AAAA1111",
        "test -d ~/Documents && echo unwalled",
    ]);
    assert_eq!(sent["exit"], 0, "{sent}");
    rig.read_until("unwalled");
}

/// The user's PATH for the next session: `dirs`, then the system's.
fn user_path(rig: &Rig, dirs: &[&Path]) {
    let mut path: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
    path.extend(["/usr/bin".to_string(), "/bin".to_string()]);
    std::fs::write(rig.state.join("path"), path.join(":")).unwrap();
}

/// The hub runs programs from the PATH's folders outside the wall, and
/// Saiful's builds run the tool caches: a session writes in none of them.
#[test]
fn a_walled_session_writes_in_no_folder_saiful_runs() {
    let rig = rig();
    let pnpm = rig.home.join(".local/share/pnpm");
    let registry = rig.home.join(".cargo/registry");
    for dir in [&pnpm, &registry] {
        std::fs::create_dir_all(dir).unwrap();
    }
    user_path(&rig, &[&pnpm]);
    rig.walk(&[
        "send",
        "touch ~/.local/share/pnpm/tmux; echo crate > ~/.cargo/registry/planted && echo kept",
    ]);
    let pane = rig.read_until("kept");
    assert!(pane.contains("Read-only file system"), "{pane}");
    assert!(!pnpm.join("tmux").exists());
    assert!(!registry.join("planted").exists());
    let own = rig.home.join(".cache/workflow-wall/cargo-registry/planted");
    assert!(own.is_file(), "the wall's own registry keeps it");
}

/// A tool the thread's branch carries, on a relative folder of the PATH,
/// never runs outside the wall: the check's outside side runs in Saiful's
/// checkout.
#[test]
fn the_check_runs_nothing_of_the_worktree_outside_the_wall() {
    let rig = rig();
    let marker = rig.home.join("ran-outside");
    rig.push("cloud/cloud_walk-w14", |seed| {
        let bin = seed.join("vendor/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cf = bin.join("cf");
        std::fs::write(
            &cf,
            format!("#!/bin/sh\ntouch '{}'\necho cf 1.0\n", marker.display()),
        )
        .unwrap();
        common::make_executable(&cf);
    });
    std::fs::write(rig.state.join("path"), "vendor/bin:/usr/bin:/bin").unwrap();
    rig.walk(&["send", "echo started"]);
    rig.read_until("started");
    assert!(!marker.exists());
}

#[test]
fn a_shell_that_ignores_the_hangup_ends_with_its_session() {
    let rig = rig();
    let sleep = format!("sleep {}3", std::process::id());
    rig.walk(&[
        "send",
        &format!("trap '' HUP; exec nohup {sleep} > /dev/null 2>&1"),
    ]);
    wait_until(Duration::from_secs(5), || running(&sleep));
    rig.walk(&["end"]);
    wait_until(Duration::from_secs(5), || !running(&sleep));
}

/// A session that swaps a submodule's folder for a link to one of Saiful's
/// own: the next start removes the link, never what it leads to.
#[test]
fn a_link_on_a_submodule_s_path_is_removed_and_not_followed() {
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
            &["submodule", "add", "-q", sub.to_str().unwrap(), "h/lib"],
        );
    });
    rig.same_gitmodules();
    let own = rig.origin.with_file_name("own");
    std::fs::create_dir_all(own.join("lib")).unwrap();
    std::fs::write(own.join("lib/keep"), "mine\n").unwrap();
    rig.walk(&["send", "cat h/lib/hello"]);
    rig.read_until("from-sub");
    rig.walk(&[
        "send",
        &format!("mv h h.old && ln -s {} h && echo linked", own.display()),
    ]);
    rig.read_until("linked");
    rig.walk(&["end"]);
    rig.walk(&["send", "cat h/lib/hello; echo again"]);
    let pane = rig.read_until("again");
    assert!(own.join("lib/keep").is_file(), "{pane}");
    assert!(pane.contains("from-sub"), "{pane}");
}

/// The update runs with Saiful's logins, so only on the URLs his own
/// checkout names; another branch's are left, and the pane says so.
#[test]
fn submodules_a_branch_names_on_its_own_are_not_updated() {
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
    rig.walk(&["send", "ls lib; echo listed"]);
    let pane = rig.read_until("listed");
    assert!(pane.contains("hub: submodules not updated"), "{pane}");
    assert!(!rig.worktree().join("lib/hello").exists());
}

/// A job question's first line, for `why`.
fn job(why: &str) -> String {
    format!("The wall cannot start on mac-mini-m2 for cloud_walk: {why}\n")
}

#[test]
fn a_job_yes_for_another_reason_releases_nothing() {
    let rig = rig();
    node_that_differs(&rig);
    let options = ["run this job outside the wall", "hold"];
    let why = "node v1 inside, v2 outside";
    rig.asked_with(
        "mac-mini-m2",
        &job(why),
        &options,
        options[0],
        "mac-mini-m2",
    );
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "echo hi"]);
    assert_eq!(sent["exit"], 2, "{sent}");
    assert!(rig.sessions().is_empty());
}

#[test]
fn a_job_yes_is_spent_by_a_job_that_started_walled() {
    let rig = rig();
    let options = ["run this job outside the wall", "hold"];
    let why = "node v24.18.0 inside, v24.21.0 outside";
    rig.asked_with(
        "mac-mini-m2",
        &job(why),
        &options,
        options[0],
        "mac-mini-m2",
    );
    let walled = rig.walk(&["send", "--yes", "AAAA1111", "echo hi"]);
    assert_eq!(walled["exit"], 0, "{walled}");
    rig.walk(&["end"]);
    node_that_differs(&rig);
    let later = rig.walk(&["send", "--yes", "AAAA1111", "echo hi"]);
    assert_eq!(later["exit"], 2, "{later}");
    assert!(rig.sessions().is_empty());
}

#[test]
fn a_wall_that_cannot_start_is_asked_with_its_own_words() {
    let rig = rig();
    let fake = rig.origin.with_file_name("fake-bin");
    common::fixture_bin(
        &fake,
        "bwrap",
        "echo 'bwrap: setting up uid map: Permission denied' >&2; exit 1",
    );
    user_path(&rig, &[&fake]);
    let answer = rig.walk(&["send", "echo hi"]);
    assert_eq!(answer["exit"], 6, "{answer}");
    let out = answer["stdout"].as_str().unwrap();
    assert!(
        out.starts_with("ask: the wall cannot start on mac-mini-m2: "),
        "{out}"
    );
    assert!(
        out.contains("bwrap: setting up uid map: Permission denied"),
        "{out}"
    );
    assert!(!out.contains("missing inside"), "{out}");
    assert!(rig.sessions().is_empty());
}

/// mem in the wall reads an empty store, and what it writes stays there.
#[test]
fn mem_in_the_wall_reaches_no_store_of_the_device() {
    let Some(mem) = common::real_mem() else {
        panic!("mem did not build");
    };
    let rig = rig();
    let bin = rig.home.join(".local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(&mem, bin.join("mem")).unwrap();
    let store = rig.home.join(".local/share/mem/store");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("planted-question.md"), "planted-question\n").unwrap();
    user_path(&rig, &[&bin]);
    rig.walk(&[
        "send",
        "mem questions; mem log from-the-wall; echo mem-done",
    ]);
    let pane = rig.read_until("mem-done");
    assert!(!pane.contains("planted-question"), "{pane}");
    let host = Command::new("grep")
        .args(["-r", "-l", "from-the-wall"])
        .arg(&rig.home)
        .output()
        .unwrap();
    assert!(
        host.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&host.stdout)
    );
}

/// The rig, with a line counted silent after `ms` of no output.
fn silent_after(ms: &str) -> Rig {
    rig_env(&[("HUB_DEVICE_SILENT_MS", ms)])
}

/// The pane's last line that is not blank.
fn last_line(answer: &serde_json::Value) -> String {
    answer["stdout"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// Reads until the read ends with a device status line, and returns the
/// read.
fn read_status(rig: &Rig) -> serde_json::Value {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let answer = rig.walk(&["read"]);
        if last_line(&answer).starts_with("device: ") {
            return answer;
        }
        assert!(Instant::now() < until, "no status line in: {answer}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_silent_line_s_read_ends_with_what_it_still_waits_on() {
    let rig = silent_after("300");
    rig.walk(&["send", "sleep 300"]);
    let read = read_status(&rig);
    let status = last_line(&read);
    assert_eq!(
        status,
        "device: no output for 0 min; the line still waits on: sleep"
    );
    for wall in ["pasta", "bwrap", "bash"] {
        assert!(!status.contains(wall), "{status}");
    }
    rig.walk(&["end"]);
    let gone = rig.walk(&["read"]);
    assert_eq!(gone["exit"], 2, "{gone}");
    assert!(
        gone["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("no tmux session cloud-cloud_walk-w14; send a line first"),
        "{gone}"
    );
}

#[test]
fn a_filter_whose_input_a_background_command_holds_is_named() {
    let rig = silent_after("300");
    rig.walk(&[
        "send",
        "bash -c 'sleep 300 & echo hi' | grep hi | cut -c1-5",
    ]);
    let read = read_status(&rig);
    assert_eq!(
        last_line(&read),
        "device: no output for 0 min; the line still waits on: grep, cut"
    );
    let pane = read["stdout"].as_str().unwrap_or_default();
    assert!(!pane.lines().any(|l| l.trim_end() == "hi"), "{pane}");
}

#[test]
fn a_line_that_keeps_printing_gets_no_status_line() {
    let rig = silent_after("300");
    rig.walk(&["send", "while :; do echo tick; sleep 0.2; done"]);
    rig.read_until("tick");
    for _ in 0..10 {
        let read = rig.walk(&["read"]);
        assert!(!last_line(&read).starts_with("device: "), "{read}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_line_sent_moments_ago_gets_no_status_line() {
    let rig = silent_after("5000");
    rig.walk(&["send", "sleep 300"]);
    std::thread::sleep(Duration::from_millis(1500));
    let read = rig.walk(&["read"]);
    assert!(!last_line(&read).starts_with("device: "), "{read}");
}

#[test]
fn an_ended_line_gets_no_status_line_even_with_a_command_left_in_the_background() {
    let rig = silent_after("300");
    rig.walk(&["send", "sleep 300 & echo ended-1"]);
    rig.read_until("ended-1");
    std::thread::sleep(Duration::from_millis(2000));
    let read = rig.walk(&["read"]);
    assert!(!last_line(&read).starts_with("device: "), "{read}");
}

#[test]
fn the_status_line_is_never_typed_into_the_pane() {
    let rig = silent_after("300");
    rig.walk(&["send", "sleep 300"]);
    let first = read_status(&rig);
    let second = rig.walk(&["read"]);
    let without = |read: &serde_json::Value| {
        let pane = read["stdout"].as_str().unwrap_or_default();
        assert_eq!(pane.matches("device: no output").count(), 1, "{pane}");
        pane.lines()
            .filter(|l| !l.starts_with("device: "))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(last_line(&second), last_line(&first));
    assert_eq!(without(&first), without(&second));
}

#[test]
fn past_five_commands_the_status_line_counts_the_rest() {
    let rig = silent_after("300");
    rig.walk(&["send", &["sleep 300"; 7].join(" | ")]);
    let read = read_status(&rig);
    assert_eq!(
        last_line(&read),
        "device: no output for 0 min; the line still waits on: sleep, sleep, sleep, sleep, sleep and 2 more"
    );
}

#[test]
fn a_silent_no_wall_line_ends_its_read_with_what_it_still_waits_on() {
    let rig = silent_after("300");
    let line = "sleep 300";
    rig.asked_with(
        "mac-mini-m2",
        &outside(line),
        &["yes", "no"],
        "yes",
        "mac-mini-m2",
    );
    let sent = rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", line]);
    assert_eq!(sent["exit"], 0, "{sent}");
    let read = read_status(&rig);
    assert_eq!(
        last_line(&read),
        "device: no output for 0 min; the line still waits on: sleep"
    );
}

#[test]
fn an_ended_no_wall_line_gets_no_status_line() {
    let rig = silent_after("300");
    let line = "sleep 300 & echo outside-done";
    rig.asked_with(
        "mac-mini-m2",
        &outside(line),
        &["yes", "no"],
        "yes",
        "mac-mini-m2",
    );
    rig.walk(&["send", "--yes", "AAAA1111", "--no-wall", line]);
    rig.read_until("exit: 0");
    std::thread::sleep(Duration::from_millis(2000));
    let read = rig.walk(&["read"]);
    assert!(!last_line(&read).starts_with("device: "), "{read}");
}
