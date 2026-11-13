//! `device amx`: the one local amx agent a cloud thread starts on this
//! device, against a fake amx (tests/fake_amx.sh) that answers with what a
//! real one printed.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, status_of};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

struct Rig {
    hub: Hub,
    home: PathBuf,
    /// What fake mem and fake amx keep: projects.json and handoff.json, read
    /// back, and record, the words of each amx call.
    state: PathBuf,
    _dir: TempDir,
}

fn rig() -> Rig {
    rig_env(&[])
}

fn rig_env(env: &[(&str, &str)]) -> Rig {
    let dir = TempDir::new("device-amx");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "mac-mini-m2\n").unwrap();
    let bin = dir.join("bin");
    let tools = dir.join("tools");
    let state = dir.join("state");
    std::fs::create_dir_all(&state).unwrap();
    fixture_bin(
        &bin,
        "systemctl",
        &format!("echo 'PATH={}:/usr/bin:/bin'", tools.display()),
    );
    fixture_bin(
        &bin,
        "mem",
        &format!(
            "S='{}'\n\
             case \"$*\" in\n\
             projects*) cat \"$S/projects.json\" 2>/dev/null || echo '{{\"projects\":[]}}' ;;\n\
             handoff*) cat \"$S/handoff.json\" 2>/dev/null || echo '{{}}' ;;\n\
             esac",
            state.display()
        ),
    );
    fixture_bin(
        &tools,
        "amx",
        &format!(
            "S='{}' exec sh '{}/tests/fake_amx.sh' \"$@\"",
            state.display(),
            env!("CARGO_MANIFEST_DIR"),
        ),
    );
    let local = dir.join("local.sock");
    let mut all = vec![
        ("HUB_PEER_TOKEN", TOKEN),
        ("HUB_LOCAL_SOCKET", local.to_str().unwrap()),
    ];
    all.extend_from_slice(env);
    let hub = Hub::spawn_env(&home, &[&bin], &["local", "--port", "0"], &all);
    let rig = Rig {
        hub,
        home,
        state,
        _dir: dir,
    };
    rig.cloud_run("w14", None);
    rig
}

impl Rig {
    /// cloud_walk building `milestone`, and shop building m2. cloud_walk's
    /// run is in the cloud, as session_01Walk, building w14 since a minute
    /// ago; its latest handoff, `handoff`, was written now, or never when it
    /// is None.
    fn cloud_run(&self, milestone: &str, handoff: Option<&str>) {
        self.cloud_run_by("session_01Walk", milestone, handoff);
    }

    /// cloud_run, with the run's runner id `runner`.
    fn cloud_run_by(&self, runner: &str, milestone: &str, handoff: Option<&str>) {
        let now = jiff::Timestamp::now();
        let since = now - jiff::SignedDuration::from_secs(60);
        let handoff_at = handoff.map_or(serde_json::Value::Null, |_| now.to_string().into());
        let projects = serde_json::json!({"projects": [
            {"name": "cloud_walk", "roadmap_status": "approved", "checkouts": [],
             "milestone": milestone, "milestones_done": 1, "milestones_total": 3,
             "handoff_at": handoff_at,
             "runner": {"machine": "cloud", "id": runner,
                        "since": since.to_string(), "milestone": "w14"}},
            {"name": "shop", "milestone": "m2"},
        ]});
        std::fs::write(self.state.join("projects.json"), projects.to_string()).unwrap();
        let body = serde_json::json!({ "body": handoff.unwrap_or_default() });
        std::fs::write(self.state.join("handoff.json"), body.to_string()).unwrap();
    }

    fn call(&self, project: &str, session: &str, args: &[&str]) -> serde_json::Value {
        let body = serde_json::json!({
            "verb": "amx", "args": args, "project": project, "session": session,
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

    /// cloud_walk's thread starts its agent, rec-t8a.
    fn started(&self) {
        let answer = self.walk(&["new", "write", "the", "readme"]);
        assert_eq!(answer["exit"], 0, "{answer}");
    }

    /// The words of each amx call, as the fake recorded them.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.state.join("record"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn record(&self) -> PathBuf {
        self.home.join("state/hub/device-amx.json")
    }
}

#[test]
fn new_starts_the_agent_in_the_thread_s_folder_and_prints_its_name() {
    let rig = rig();
    let answer = rig.walk(&["new", "write", "the", "readme"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    assert_eq!(answer["stdout"], "rec-t8a\n");
    let folder = rig.home.join(".cache/workflow-cloud/cloud-cloud_walk-w14");
    assert_eq!(
        rig.calls(),
        [format!(
            "[new][--dir][{}][write the readme]",
            folder.display()
        )]
    );
    assert!(folder.is_dir());
}

#[test]
fn ls_prints_only_the_thread_s_agent() {
    let rig = rig();
    assert_eq!(rig.walk(&["ls"])["stdout"], "[]\n");
    rig.started();
    let answer = rig.walk(&["ls"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    let listed: Vec<serde_json::Value> =
        serde_json::from_str(answer["stdout"].as_str().unwrap()).unwrap();
    let ids: Vec<_> = listed.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["rec-t8a"]);
    assert_eq!(listed[0]["task"], "Reply with the single word ready.");
}

#[test]
fn result_prints_the_agent_s_answer() {
    let rig = rig();
    rig.started();
    let answer = rig.walk(&["result", "rec-t8a"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    assert_eq!(answer["stdout"], "ready\n");
    assert_eq!(rig.calls()[1], "[result][rec-t8a][--timeout][80]");
}

#[test]
fn a_result_that_times_out_says_to_ask_again() {
    let rig = rig();
    rig.started();
    std::fs::write(rig.state.join("result-times-out"), "").unwrap();
    let answer = rig.walk(&["result", "rec-t8a"]);
    assert_eq!(answer["exit"], 9, "{answer}");
    assert_eq!(answer["stdout"], "again: device amx result rec-t8a\n");
}

#[test]
fn status_send_answer_and_logs_pass_through_for_the_thread_s_agent() {
    let rig = rig();
    rig.started();
    let status = rig.walk(&["status", "rec-t8a"]);
    assert_eq!(status["exit"], 0, "{status}");
    assert!(
        status["stdout"]
            .as_str()
            .unwrap()
            .starts_with("rec-t8a  starting\n"),
        "{status}"
    );
    assert_eq!(rig.walk(&["send", "rec-t8a", "go", "on"])["exit"], 0);
    assert_eq!(rig.walk(&["answer", "rec-t8a", "down", "enter"])["exit"], 0);
    let logs = rig.walk(&["logs", "rec-t8a"]);
    assert!(
        logs["stdout"]
            .as_str()
            .unwrap()
            .contains("Yes, I trust this folder"),
        "{logs}"
    );
    assert_eq!(
        rig.calls()[1..],
        [
            "[status][rec-t8a]",
            "[send][rec-t8a][go][on]",
            "[answer][rec-t8a][down][enter]",
            "[logs][rec-t8a]",
        ]
    );
}

#[test]
fn stop_forces_the_agent_down_and_forgets_it() {
    let rig = rig();
    rig.started();
    let stopped = rig.walk(&["stop", "rec-t8a"]);
    assert_eq!(stopped["exit"], 0, "{stopped}");
    assert_eq!(stopped["stdout"], "rec-t8a stopped\n");
    assert_eq!(rig.calls()[1..], ["[stop][--force][rec-t8a]"]);
    assert!(!rig.record().exists());
    assert_eq!(rig.walk(&["ls"])["stdout"], "[]\n");
}

#[test]
fn an_agent_the_thread_did_not_start_is_refused() {
    let rig = rig();
    rig.started();
    let answer = rig.walk(&["status", "rec-t8b"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: rec-t8b is not this thread's agent\n"
    );
    let other = rig.call("shop", "cse_01Shop", &["stop", "rec-t8a"]);
    assert_eq!(
        other["stderr"],
        "device: rec-t8a is not this thread's agent\n"
    );
    assert_eq!(rig.calls().len(), 1, "{:?}", rig.calls());
}

#[test]
fn uninstall_is_refused() {
    let rig = rig();
    let answer = rig.walk(&["uninstall"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: amx uninstall is not allowed here\n"
    );
    assert!(rig.calls().is_empty());
}

#[test]
fn a_second_thread_s_new_is_busy_while_one_agent_runs() {
    let rig = rig();
    rig.started();
    let answer = rig.call("shop", "cse_01Shop", &["new", "check", "the", "cart"]);
    assert_eq!(answer["exit"], 5, "{answer}");
    assert_eq!(answer["stdout"], "busy: cloud_walk\n");
    assert_eq!(rig.calls().len(), 1, "{:?}", rig.calls());
}

#[test]
fn a_task_word_that_is_a_flag_is_refused() {
    let rig = rig();
    let answer = rig.walk(&["new", "--model", "opus", "write", "it"]);
    assert_eq!(answer["exit"], 2, "{answer}");
    assert!(rig.calls().is_empty(), "{:?}", rig.calls());
}

/// The sweep runs every 200 ms.
const SWEEP: [(&str, &str); 1] = [("HUB_DEVICE_SWEEP_MS", "200")];

/// Waits a few sweeps after the run ends, and checks that the hub stopped
/// rec-t8a, forgot it, and left rec-t8b alone.
fn stopped_by_the_sweep(rig: &Rig) {
    std::thread::sleep(Duration::from_secs(2));
    let calls = rig.calls();
    assert_eq!(calls[1..], ["[stop][--force][rec-t8a]"], "{calls:?}");
    assert!(!rig.record().exists());
}

#[test]
fn the_agent_is_stopped_once_the_run_parks() {
    let rig = rig_env(&SWEEP);
    rig.started();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(rig.calls().len(), 1, "{:?}", rig.calls());
    rig.cloud_run("w14", Some("parked: waiting on q1"));
    stopped_by_the_sweep(&rig);
}

#[test]
fn the_agent_is_stopped_once_the_run_lands() {
    let rig = rig_env(&SWEEP);
    rig.started();
    rig.cloud_run("w15", None);
    stopped_by_the_sweep(&rig);
}

#[test]
fn a_stop_that_fails_is_tried_again_next_sweep() {
    let rig = rig_env(&SWEEP);
    rig.started();
    std::fs::write(rig.state.join("stop-fails"), "").unwrap();
    rig.cloud_run("w15", None);
    std::thread::sleep(Duration::from_secs(1));
    assert!(rig.record().exists(), "{:?}", rig.calls());
    let tries = rig.calls().len();
    assert!(tries > 2, "{:?}", rig.calls());
    std::fs::remove_file(rig.state.join("stop-fails")).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(!rig.record().exists(), "{:?}", rig.calls());
}

#[test]
fn an_agent_amx_no_longer_knows_is_forgotten_on_stop() {
    let rig = rig();
    rig.started();
    std::fs::write(rig.state.join("stop-gone"), "").unwrap();
    rig.walk(&["stop", "rec-t8a"]);
    assert!(!rig.record().exists());
    let again = rig.call("shop", "cse_01Shop", &["new", "look", "at", "the", "phone"]);
    assert_eq!(again["exit"], 0, "{again}");
}

#[test]
fn the_agent_is_stopped_once_another_session_runs_the_project() {
    let rig = rig_env(&SWEEP);
    rig.cloud_run("w14", None);
    rig.started();
    rig.cloud_run_by("session_01Next", "w14", None);
    stopped_by_the_sweep(&rig);
}

/// Each mem write the hub trusts by the machine that wrote it, with a line
/// that makes it: a thread's tmux and amx agent are shells on the device, so
/// each must be held there, or the thread writes its own release.
const TRUSTED_WRITES: &[(&str, &str)] = &[
    ("an answer's machine", "mem answer AAAA1111 yes"),
    (
        "an answer's machine",
        "mem --project cloud_walk answer AAAA1111 yes",
    ),
    ("device_by", "mem project set device nuc"),
    ("device_by", "mem project unset device"),
    (
        "cloud_secrets_by",
        "mem project set cloud-secrets 'X=Shopify/x'",
    ),
    ("cloud_secrets_by", "mem project unset cloud-secrets"),
];

/// The ways hub/src reads a writer: a `*_by` key of a project, or the
/// machine of a mem item it was given.
fn writers_read_in_hub_src() -> Vec<String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(src).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in text.lines() {
            if line.contains(r#"first()?["machine"]"#) {
                found.push("an answer's machine".to_string());
            }
            let mut rest = line;
            while let Some(at) = rest.find(r#"_by"]"#) {
                let key = rest[..at]
                    .rsplit('"')
                    .next()
                    .map(|k| format!("{k}_by"))
                    .unwrap();
                found.push(key);
                rest = &rest[at + 5..];
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn every_write_the_hub_trusts_by_its_writer_is_held_in_tmux_and_amx() {
    let mut listed: Vec<String> = TRUSTED_WRITES.iter().map(|(k, _)| k.to_string()).collect();
    listed.sort();
    listed.dedup();
    assert_eq!(
        writers_read_in_hub_src(),
        listed,
        "hub/src trusts a writer this list does not name: add each mem write \
         that makes it to TRUSTED_WRITES, and hold it in hub/src/held.rs"
    );
    let rig = rig();
    for (_, line) in TRUSTED_WRITES {
        assert!(hub::held::held(line), "tmux would send: {line}");
        let words: Vec<&str> = line.split(' ').collect();
        let mut new = vec!["new"];
        new.extend(&words);
        let answer = rig.walk(&new);
        assert_eq!(answer["exit"], 2, "amx new took: {line}: {answer}");
    }
    rig.started();
    for (_, line) in TRUSTED_WRITES {
        let mut send = vec!["send", "rec-t8a"];
        send.extend(line.split(' '));
        let answer = rig.walk(&send);
        assert_eq!(answer["exit"], 2, "amx send took: {line}: {answer}");
    }
    assert_eq!(rig.calls().len(), 1, "{:?}", rig.calls());
}
