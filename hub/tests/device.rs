//! `POST /device/here` on the local hub's peer port: a call served on this
//! machine, as a hub sends it on when the route names this one.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Hub, TempDir, body_of, fixture_bin, status_of, wait_for};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

struct Rig {
    hub: Hub,
    /// Where the fake adb lives: on the user manager's PATH, not the hub's.
    tools: PathBuf,
    /// One line of argv per `mem` call.
    mem_log: PathBuf,
    _dir: TempDir,
}

fn rig(machine: &str, env: &[(&str, &str)]) -> Rig {
    let dir = TempDir::new("device");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), format!("{machine}\n")).unwrap();
    let tools = dir.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let bin = dir.join("bin");
    let mem_log = dir.join("mem.log");
    fixture_bin(
        &bin,
        "systemctl",
        &format!(
            "echo 'HOME=/x'; echo 'PATH={}:/usr/bin:/bin'",
            tools.display()
        ),
    );
    fixture_bin(
        &bin,
        "mem",
        &format!("echo \"$*\" >> '{}'", mem_log.display()),
    );
    let socket = dir.join("local.sock");
    let mut all = vec![
        ("HUB_PEER_TOKEN", TOKEN),
        ("HUB_LOCAL_SOCKET", socket.to_str().unwrap()),
    ];
    all.extend_from_slice(env);
    let hub = Hub::spawn_env(&home, &[&bin], &["local", "--port", "0"], &all);
    Rig {
        hub,
        tools,
        mem_log,
        _dir: dir,
    }
}

fn post(hub: &Hub, body: &str) -> String {
    hub.raw(&format!(
        "POST /device/here HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
         Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}

fn call(hub: &Hub, verb: &str, args: &[&str]) -> serde_json::Value {
    let body = serde_json::json!({"verb": verb, "args": args, "project": "demo"});
    let response = post(hub, &body.to_string());
    assert_eq!(status_of(&response), 200, "{response}");
    serde_json::from_str(body_of(&response)).expect("json answer")
}

fn mem_lines(log: &Path) -> Vec<String> {
    wait_for("the mem log line", Duration::from_secs(5), || log.exists());
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn whoami_answers_the_machine_name() {
    let rig = rig("nuc", &[]);
    let answer = call(&rig.hub, "whoami", &[]);
    assert_eq!(answer["machine"], "nuc");
    assert_eq!(answer["stdout"], "nuc\n");
    assert_eq!(answer["stderr"], "");
    assert_eq!(answer["exit"], 0);
    assert_eq!(answer["files"], serde_json::json!({}));
    assert_eq!(
        mem_lines(&rig.mem_log),
        vec!["--project=demo log -- device nuc whoami exit 0"]
    );
}

#[test]
fn run_uses_the_user_managers_path_and_passes_the_exit_code_through() {
    let rig = rig("nuc", &[]);
    let seen = rig.tools.join("adb.argv");
    fixture_bin(
        &rig.tools,
        "adb",
        &format!(
            "echo \"$*\" > '{}'; echo fake-adb-out; echo fake-adb-err >&2; exit 3",
            seen.display()
        ),
    );
    let answer = call(&rig.hub, "run", &["adb", "devices"]);
    assert_eq!(answer["stdout"], "fake-adb-out\n");
    assert_eq!(answer["stderr"], "fake-adb-err\n");
    assert_eq!(answer["exit"], 3);
    assert_eq!(std::fs::read_to_string(&seen).unwrap(), "devices\n");
    assert_eq!(
        mem_lines(&rig.mem_log),
        vec!["--project=demo log -- device nuc run adb devices exit 3"]
    );
}

#[test]
fn a_program_off_the_allowlist_never_runs() {
    let rig = rig("nuc", &[]);
    let marker = rig.tools.join("rm.ran");
    fixture_bin(&rig.tools, "rm", &format!("touch '{}'", marker.display()));
    let answer = call(&rig.hub, "run", &["rm", "-rf", "/tmp/x"]);
    assert_eq!(answer["exit"], 2);
    assert_eq!(answer["stderr"], "device: rm is not allowed here\n");
    assert_eq!(answer["stdout"], "");
    assert!(!marker.exists());
    assert_eq!(
        mem_lines(&rig.mem_log),
        vec![
            "--project=demo log -- device nuc run rm -rf /tmp/x exit 2 refused: rm is not allowed here"
        ]
    );
}

#[test]
fn a_program_given_as_a_path_is_refused() {
    let rig = rig("nuc", &[]);
    let answer = call(&rig.hub, "run", &["/usr/bin/adb", "devices"]);
    assert_eq!(answer["exit"], 2);
    assert_eq!(
        answer["stderr"],
        "device: /usr/bin/adb is not allowed here\n"
    );
}

#[test]
fn run_without_a_program_is_refused() {
    let rig = rig("nuc", &[]);
    let answer = call(&rig.hub, "run", &[]);
    assert_eq!(answer["exit"], 2);
    assert_eq!(answer["stderr"], "device: run needs a program\n");
    assert_eq!(
        mem_lines(&rig.mem_log),
        vec!["--project=demo log -- device nuc run exit 2 refused: run needs a program"]
    );
}

#[test]
fn arguments_with_spaces_and_quotes_arrive_whole() {
    let rig = rig("nuc", &[]);
    let seen = rig.tools.join("adb.argv");
    fixture_bin(
        &rig.tools,
        "adb",
        &format!(
            "for a in \"$@\"; do printf '[%s]\\n' \"$a\"; done > '{}'",
            seen.display()
        ),
    );
    let answer = call(&rig.hub, "run", &["adb", "shell", "echo \"a b\""]);
    assert_eq!(answer["exit"], 0);
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        "[shell]\n[echo \"a b\"]\n"
    );
}

#[test]
fn a_run_past_the_timeout_is_stopped() {
    let rig = rig("nuc", &[("HUB_DEVICE_TIMEOUT_MS", "300")]);
    fixture_bin(&rig.tools, "adb", "exec sleep 5");
    let started = Instant::now();
    let answer = call(&rig.hub, "run", &["adb", "devices"]);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(answer["exit"], 124);
    assert_eq!(
        answer["stderr"],
        "device: adb ran past 0.3 s and was stopped\n"
    );
}

#[test]
fn a_body_without_a_project_is_a_bad_request() {
    let rig = rig("nuc", &[]);
    let response = post(&rig.hub, r#"{"verb":"whoami","args":[]}"#);
    assert_eq!(status_of(&response), 400, "{response}");
    let response = post(&rig.hub, "not json");
    assert_eq!(status_of(&response), 400, "{response}");
}
