//! `POST /device` with verb `browser`, against a fake playwright-cli.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Hub, TempDir, body_of, fixture_bin, status_of, wait_for};

const TOKEN: &str = "peer-token-for-tests-0123456789abcdef";

/// The route that serves on this machine whatever the hub's route is.
const ROUTE: &str = "/device/here";

/// What the fake goto does unless a test brings its own: write a snapshot in
/// its cwd and link it the way playwright-cli does.
const SNAPSHOT_GOTO: &str = "mkdir -p .playwright-cli\n\
     printf 'snapshot of %s\\n' \"$2\" > .playwright-cli/page-x.yml\n\
     echo \"### Page\"\n\
     echo \"- Page URL: $2\"\n\
     echo \"- [Snapshot](.playwright-cli/page-x.yml)\"";

struct Rig {
    hub: Hub,
    home: PathBuf,
    /// On the user manager's PATH, not the hub's.
    tools: PathBuf,
    /// One line per fake call: cwd, the token it was given, then argv,
    /// tab-separated.
    record: PathBuf,
    mem_log: PathBuf,
    _dir: TempDir,
}

fn rig(goto: &str, env: &[(&str, &str)]) -> Rig {
    let dir = TempDir::new("device-browser");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "nuc\n").unwrap();
    // A Chromium that is up: its lock names a live pid, this test's own.
    std::fs::create_dir_all(home.join("config/chromium")).unwrap();
    std::os::unix::fs::symlink(
        format!("testhost-{}", std::process::id()),
        home.join("config/chromium/SingletonLock"),
    )
    .unwrap();
    let tools = dir.join("tools");
    let bin = dir.join("bin");
    let record = dir.join("record");
    let sessions = dir.join("sessions");
    let mem_log = dir.join("mem.log");
    fixture_bin(
        &bin,
        "systemctl",
        &format!("echo 'PATH={}:/usr/bin:/bin'", tools.display()),
    );
    fixture_bin(
        &bin,
        "mem",
        &format!("echo \"$*\" >> '{}'", mem_log.display()),
    );
    fixture_bin(
        &tools,
        "playwright-extension-token",
        "echo '  fake-token-123  '",
    );
    fixture_bin(
        &home.join(".dotfiles/bin"),
        "playwright-cli",
        &format!(
            "line=\"$(pwd)\ttoken=$PLAYWRIGHT_MCP_EXTENSION_TOKEN\"\n\
             for a in \"$@\"; do line=\"$line\t$a\"; done\n\
             printf '%s\\n' \"$line\" >> '{record}'\n\
             if [ \"$1\" = list ]; then\n\
               echo '### Browsers'\n\
               if [ -s '{sessions}' ]; then\n\
                 while read -r s; do\n\
                   echo \"- $s:\"; echo '  - status: open'\n\
                   echo '  - browser-type: chrome-for-testing (attached)'\n\
                 done < '{sessions}'\n\
               else echo '  (no browsers)'; fi\n\
               exit 0\n\
             fi\n\
             session=\"${{1#-s=}}\"; shift\n\
             case \"$1\" in\n\
             attach) echo \"$session\" >> '{sessions}'; echo attached ;;\n\
             goto)\n{goto}\n;;\n\
             *) echo \"ran $*\" ;;\n\
             esac",
            record = record.display(),
            sessions = sessions.display(),
        ),
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
        home,
        tools,
        record,
        mem_log,
        _dir: dir,
    }
}

impl Rig {
    fn session_dir(&self, session: &str) -> String {
        self.home
            .join("state/hub/device")
            .join(session)
            .display()
            .to_string()
    }

    /// Every recorded call, its fields split at the tabs.
    fn calls(&self) -> Vec<Vec<String>> {
        let Ok(text) = std::fs::read_to_string(&self.record) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| line.split('\t').map(str::to_string).collect())
            .collect()
    }
}

fn browser(hub: &Hub, session: Option<&str>, args: &[&str]) -> serde_json::Value {
    let mut body = serde_json::json!({"verb": "browser", "args": args, "project": "demo"});
    if let Some(session) = session {
        body["session"] = session.into();
    }
    let body = body.to_string();
    let response = hub.raw(&format!(
        "POST {ROUTE} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
         Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    ));
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

fn fields(cwd: &str, token: &str, argv: &[&str]) -> Vec<String> {
    let mut all = vec![cwd.to_string(), format!("token={token}")];
    all.extend(argv.iter().map(|a| a.to_string()));
    all
}

#[test]
fn the_first_call_attaches_the_session_and_the_second_does_not() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    let answer = browser(&rig.hub, Some("s1"), &["goto", "https://claude.ai/code"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    assert_eq!(
        answer["stdout"],
        "### Page\n- Page URL: https://claude.ai/code\n- [Snapshot](.playwright-cli/page-x.yml)\n"
    );
    let cwd = rig.session_dir("s1");
    assert_eq!(
        rig.calls(),
        vec![
            fields(&cwd, "", &["list"]),
            fields(
                &cwd,
                "fake-token-123",
                &["-s=s1", "attach", "--extension=chromium"]
            ),
            fields(&cwd, "", &["-s=s1", "goto", "https://claude.ai/code"]),
        ]
    );
    assert_eq!(
        mem_lines(&rig.mem_log),
        vec!["--project=demo log -- device nuc browser goto https://claude.ai/code exit 0"]
    );

    let answer = browser(&rig.hub, Some("s1"), &["snapshot"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    assert_eq!(answer["stdout"], "ran snapshot\n");
    let calls = rig.calls();
    assert_eq!(
        calls[3..],
        [
            fields(&cwd, "", &["list"]),
            fields(&cwd, "", &["-s=s1", "snapshot"]),
        ]
    );
}

#[test]
fn the_snapshot_a_command_links_comes_back_in_files() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    let answer = browser(&rig.hub, Some("s1"), &["goto", "https://claude.ai/code"]);
    assert_eq!(
        answer["files"],
        serde_json::json!({
            ".playwright-cli/page-x.yml": "snapshot of https://claude.ai/code\n"
        })
    );
    let written = Path::new(&rig.session_dir("s1")).join(".playwright-cli/page-x.yml");
    assert!(written.is_file(), "{}", written.display());
}

#[test]
fn two_sessions_at_once_each_attach_and_run_in_their_own_folder() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    let (one, two) = std::thread::scope(|scope| {
        let one = scope.spawn(|| browser(&rig.hub, Some("s1"), &["goto", "https://a.test"]));
        let two = scope.spawn(|| browser(&rig.hub, Some("s2"), &["goto", "https://b.test"]));
        (one.join().unwrap(), two.join().unwrap())
    });
    assert_eq!(one["exit"], 0, "{one}");
    assert_eq!(two["exit"], 0, "{two}");
    assert_eq!(
        one["files"][".playwright-cli/page-x.yml"],
        "snapshot of https://a.test\n"
    );
    assert_eq!(
        two["files"][".playwright-cli/page-x.yml"],
        "snapshot of https://b.test\n"
    );
    let calls = rig.calls();
    for (session, url) in [("s1", "https://a.test"), ("s2", "https://b.test")] {
        let cwd = rig.session_dir(session);
        let flag = format!("-s={session}");
        let attach = fields(
            &cwd,
            "fake-token-123",
            &[&flag, "attach", "--extension=chromium"],
        );
        let goto = fields(&cwd, "", &[&flag, "goto", url]);
        assert!(calls.contains(&attach), "{calls:?}");
        assert!(calls.contains(&goto), "{calls:?}");
    }
    for call in &calls {
        assert!(
            !call
                .iter()
                .any(|a| ["close", "close-all", "kill-all"].contains(&a.as_str())),
            "{call:?}"
        );
    }
}

#[test]
fn commands_that_end_the_browser_or_take_its_session_never_run() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    for (args, reason) in [
        (&["close"][..], "browser close is not allowed here"),
        (&["kill-all"][..], "browser kill-all is not allowed here"),
        (
            &["-s=other", "goto", "x"][..],
            "browser -s=other is not allowed here",
        ),
    ] {
        let answer = browser(&rig.hub, Some("s1"), args);
        assert_eq!(answer["exit"], 2, "{answer}");
        assert_eq!(answer["stderr"], format!("device: {reason}\n"));
    }
    assert_eq!(rig.calls(), Vec::<Vec<String>>::new());
}

/// Only the walk's own commands run: nothing that runs code in node, writes
/// or reads a file of the device's choosing, hands back the profile's
/// cookies, or serves a dashboard (review of m2-device). The command must
/// come first, so a flag cannot hide an open or attach behind it.
#[test]
fn only_a_walk_s_commands_run() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    for (args, reason) in [
        (
            &["run-code", "async page => 1"][..],
            "browser run-code is not allowed here",
        ),
        (
            &["upload", "/home/x/.ssh/id"][..],
            "browser upload is not allowed here",
        ),
        (
            &["state-save", "/tmp/s.json"][..],
            "browser state-save is not allowed here",
        ),
        (
            &["cookie-list"][..],
            "browser cookie-list is not allowed here",
        ),
        (
            &["show", "--port", "9"][..],
            "browser show is not allowed here",
        ),
        (
            &["screenshot"][..],
            "browser screenshot is not allowed here",
        ),
        (
            &["response-body", "1"][..],
            "browser response-body is not allowed here",
        ),
        (
            &["--browser", "x", "open"][..],
            "browser --browser is not allowed here",
        ),
        (
            &["snapshot", "--filename=/home/x/.config/a"][..],
            "browser --filename=/home/x/.config/a is not allowed here",
        ),
    ] {
        let answer = browser(&rig.hub, Some("s1"), args);
        assert_eq!(answer["exit"], 2, "{args:?}: {answer}");
        assert_eq!(answer["stderr"], format!("device: {reason}\n"), "{args:?}");
    }
    assert_eq!(rig.calls(), Vec::<Vec<String>>::new());
    for args in [
        &["goto", "https://x.test"][..],
        &["find", "Save"],
        &["detach"],
    ] {
        let answer = browser(&rig.hub, Some("s1"), args);
        assert_eq!(answer["exit"], 0, "{args:?}: {answer}");
    }
}

#[test]
fn an_argument_with_quotes_and_spaces_arrives_whole() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    let answer = browser(&rig.hub, Some("s1"), &["fill", "e5", "a \"b\" c"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    let calls = rig.calls();
    assert_eq!(
        calls.last().unwrap()[2..],
        ["-s=s1", "fill", "e5", "a \"b\" c"]
    );
}

#[test]
fn a_call_while_chromium_is_down_starts_it_before_attaching() {
    let rig = rig(SNAPSHOT_GOTO, &[("HUB_DEVICE_CHROMIUM_WAIT_MS", "3000")]);
    let lock = rig.home.join("config/chromium/SingletonLock");
    std::fs::remove_file(&lock).unwrap();
    std::os::unix::fs::symlink("testhost-999999999", &lock).unwrap();
    fixture_bin(
        &rig.tools,
        "app-run",
        &format!(
            "printf 'app-run\\t%s\\n' \"$*\" >> '{record}'\n\
             rm -f '{lock}'\n\
             ln -s 'testhost-{pid}' '{lock}'",
            record = rig.record.display(),
            lock = lock.display(),
            pid = std::process::id(),
        ),
    );
    let answer = browser(&rig.hub, Some("s1"), &["goto", "https://claude.ai/code"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    let calls = rig.calls();
    let started = calls
        .iter()
        .position(|c| c[..] == ["app-run", "chromium-browser"])
        .unwrap_or_else(|| panic!("app-run never ran: {calls:?}"));
    let attached = calls
        .iter()
        .position(|c| c.get(3).is_some_and(|a| a == "attach"))
        .unwrap_or_else(|| panic!("never attached: {calls:?}"));
    assert!(started < attached, "{calls:?}");
}

#[test]
fn a_browser_call_needs_a_session() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    for session in [None, Some("../up"), Some("")] {
        let answer = browser(&rig.hub, session, &["goto", "x"]);
        assert_eq!(answer["exit"], 2, "{answer}");
        assert_eq!(answer["stderr"], "device: browser needs a session\n");
    }
    assert_eq!(rig.calls(), Vec::<Vec<String>>::new());
}

#[test]
fn a_call_past_the_timeout_is_stopped() {
    let rig = rig("exec sleep 5", &[("HUB_DEVICE_TIMEOUT_MS", "300")]);
    let started = Instant::now();
    let answer = browser(&rig.hub, Some("s1"), &["goto", "x"]);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(answer["exit"], 124, "{answer}");
    assert_eq!(
        answer["stderr"],
        "device: browser ran past 0.3 s and was stopped\n"
    );
}

#[test]
fn a_link_outside_the_session_folder_is_not_sent() {
    let rig = rig(
        "echo '- [a](../../etc/passwd)'; echo '- [b](/etc/passwd)'",
        &[],
    );
    // So the climbing link would name a real file if it were followed.
    let climbed = rig.home.join("state/hub/etc");
    std::fs::create_dir_all(&climbed).unwrap();
    std::fs::write(climbed.join("passwd"), "secret\n").unwrap();
    let answer = browser(&rig.hub, Some("s1"), &["goto", "x"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    assert_eq!(answer["files"], serde_json::json!({}));
}

/// dotfiles v2 keeps its bin at ~/Codes/dotfiles/bin, not ~/.dotfiles/bin:
/// the wrapper on the user manager's PATH is the one that runs.
#[test]
fn the_wrapper_on_the_user_path_runs_when_there_is_no_dotfiles_folder() {
    let rig = rig(SNAPSHOT_GOTO, &[]);
    std::fs::rename(
        rig.home.join(".dotfiles/bin/playwright-cli"),
        rig.tools.join("playwright-cli"),
    )
    .unwrap();
    std::fs::remove_dir_all(rig.home.join(".dotfiles")).unwrap();
    let answer = browser(&rig.hub, Some("s1"), &["goto", "https://claude.ai/code"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    let calls = rig.calls();
    assert!(
        calls.iter().any(|c| c.get(3).is_some_and(|a| a == "goto")),
        "{calls:?}"
    );
}

/// dotfiles v2 has no app-run: Chromium starts as its own user unit, the way
/// v2's playwright-cli wrapper starts it over ssh.
#[test]
fn with_no_app_run_chromium_starts_through_systemd_run() {
    let rig = rig(SNAPSHOT_GOTO, &[("HUB_DEVICE_CHROMIUM_WAIT_MS", "3000")]);
    let lock = rig.home.join("config/chromium/SingletonLock");
    std::fs::remove_file(&lock).unwrap();
    std::os::unix::fs::symlink("testhost-999999999", &lock).unwrap();
    fixture_bin(
        &rig.tools,
        "systemd-run",
        &format!(
            "printf 'systemd-run\\t%s\\n' \"$*\" >> '{record}'\n\
             rm -f '{lock}'\n\
             ln -s 'testhost-{pid}' '{lock}'",
            record = rig.record.display(),
            lock = lock.display(),
            pid = std::process::id(),
        ),
    );
    let answer = browser(&rig.hub, Some("s1"), &["goto", "https://claude.ai/code"]);
    assert_eq!(answer["exit"], 0, "{answer}");
    let calls = rig.calls();
    let started = calls
        .iter()
        .position(|c| {
            c[..]
                == [
                    "systemd-run",
                    "--user --quiet --collect --property=ExitType=cgroup -- chromium-browser",
                ]
        })
        .unwrap_or_else(|| panic!("systemd-run never ran: {calls:?}"));
    let attached = calls
        .iter()
        .position(|c| c.get(3).is_some_and(|a| a == "attach"))
        .unwrap_or_else(|| panic!("never attached: {calls:?}"));
    assert!(started < attached, "{calls:?}");
}
