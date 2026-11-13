//! `device tmux`: a thread's own tmux session on this device, against a real
//! tmux on a private socket, so no test reaches Saiful's own server.

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use common::{Hub, TempDir, body_of, fixture_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

struct Rig {
    hub: Hub,
    home: PathBuf,
    socket: PathBuf,
    /// What fake mem keeps: projects.json, read back, and log, the lines
    /// `mem log` was given.
    state: PathBuf,
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
    fixture_bin(&bin, "systemctl", "echo 'PATH=/usr/bin:/bin'");
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "S='{}'\n\
             for a in \"$@\"; do last=\"$a\"; done\n\
             case \"$*\" in\n\
             projects*) cat \"$S/projects.json\" 2>/dev/null || echo '{{\"projects\":[]}}' ;;\n\
             *' log -- '*) printf '%s %s\\n' \"$1\" \"$last\" >> \"$S/log\" ;;\n\
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
        _dir: dir,
    };
    rig.milestones(&[("cloud_walk", "w14"), ("shop", "m2")]);
    rig
}

impl Rig {
    /// Each project's open milestone, as `mem projects --json` names it.
    fn milestones(&self, rows: &[(&str, &str)]) {
        let rows: Vec<_> = rows
            .iter()
            .map(|(name, milestone)| serde_json::json!({"name": name, "milestone": milestone}))
            .collect();
        std::fs::write(
            self.state.join("projects.json"),
            serde_json::json!({ "projects": rows }).to_string(),
        )
        .unwrap();
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
    let folder = rig.home.join(".cache/workflow-cloud/cloud-cloud_walk-w14");
    rig.read_until(folder.to_str().unwrap());
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
    rig.milestones(&[("cloud_walk", "w1")]);
    rig.walk(&["send", "true"]);
    rig.milestones(&[("cloud_walk", "w14")]);
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
