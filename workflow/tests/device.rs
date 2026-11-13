//! `workflow device` through the real binary, against a fake `curl` on PATH:
//! it records its argv and stdin, writes a canned body to the `-o` file,
//! prints a canned status and exits a canned code.

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Output;
use std::sync::RwLock;

use scratch::Scratch;
use serde_json::{Value, json};

/// A fixture written while another test forks leaves "Text file busy" on
/// exec, so fixtures are written under the write lock and spawns wait.
static FORKS: RwLock<()> = RwLock::new(());

fn world(tag: &str) -> Scratch {
    let work = Scratch::new(tag);
    let writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.mkdir("cwd");
    work.mkdir("state");
    let at = |rel: &str| work.at(rel).display().to_string();
    work.script(
        "bin/curl",
        &format!(
            "for a in \"$@\"; do echo \"$a\" >> {argv}; done\n\
             cat > {stdin}\n\
             while [ $# -gt 0 ]; do [ \"$1\" = -o ] && out=$2; shift; done\n\
             [ -n \"$out\" ] && cp {body} \"$out\"\n\
             printf %s \"$(cat {status})\"\n\
             exit $(cat {code})",
            argv = at("curl.argv"),
            stdin = at("curl.stdin"),
            body = at("curl.body"),
            status = at("curl.status"),
            code = at("curl.code"),
        ),
    );
    // A mem that keeps its argv, one argument a line, and prints what
    // `mem ask` prints for a new question.
    work.script(
        "bin/mem",
        &format!(
            "for a in \"$@\"; do echo \"$a\" >> {argv}; done\n\
             echo '#{QUESTION}'",
            argv = at("mem.argv"),
        ),
    );
    drop(writing);
    answer(&work, 200, 0, "");
    work
}

const QUESTION: &str = "ABCD1234";

/// What the fake curl does next: the status it prints, the code it exits
/// with and the body it leaves in the `-o` file.
fn answer(work: &Scratch, status: u32, code: i32, body: &str) {
    let _writing = FORKS.write().unwrap_or_else(|e| e.into_inner());
    work.put("curl.status", &status.to_string());
    work.put("curl.code", &code.to_string());
    work.put("curl.body", body);
}

fn hub_says(machine: Option<&str>, stdout: &str, stderr: &str, exit: i32, files: Value) -> String {
    json!({"machine": machine, "stdout": stdout, "stderr": stderr, "exit": exit, "files": files})
        .to_string()
}

fn device(work: &Scratch, args: &[&str]) -> Output {
    let _forking = FORKS.read().unwrap_or_else(|e| e.into_inner());
    std::process::Command::new(env!("CARGO_BIN_EXE_workflow"))
        .arg("device")
        .args(args)
        .current_dir(work.at("cwd"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                work.at("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("HOME", work.at("home"))
        .env("XDG_STATE_HOME", work.at("state"))
        .env("MEM_PROJECT", "alpha")
        .env("WORKFLOW_MEM", work.at("bin/mem"))
        .env("CLAUDE_CODE_REMOTE_SESSION_ID", "session_01X")
        .env_remove("WORKFLOW_DEVICE_URL")
        .env_remove("WORKFLOW_DEVICE_TIMEOUT_S")
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn sent(work: &Scratch) -> Value {
    serde_json::from_str(&work.read("curl.stdin")).unwrap()
}

#[test]
fn a_browser_call_posts_the_request_and_plays_back_the_answer() {
    let work = world("device-browser");
    answer(
        &work,
        200,
        0,
        &hub_says(
            Some("nuc"),
            "Page: claude\n",
            "",
            0,
            json!({".playwright-cli/page-x.yml": "- heading\n"}),
        ),
    );
    let out = device(&work, &["browser", "goto", "https://claude.ai/code"]);
    assert_eq!(stdout(&out), "Page: claude\n", "{out:?}");
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        sent(&work),
        json!({"verb": "browser", "args": ["goto", "https://claude.ai/code"],
               "session": "session_01X", "project": "alpha"})
    );
    let argv = work.read("curl.argv");
    assert!(
        argv.lines()
            .any(|l| l == "https://device.on-rexa.com/device"),
        "{argv}"
    );
    assert_eq!(work.read("cwd/.playwright-cli/page-x.yml"), "- heading\n");
}

#[test]
fn arguments_arrive_unjoined() {
    let work = world("device-args");
    answer(&work, 200, 0, &hub_says(None, "", "", 0, json!({})));
    device(&work, &["run", "adb", "shell", "echo \"a b\""]);
    assert_eq!(sent(&work)["args"], json!(["adb", "shell", "echo \"a b\""]));
    device(&work, &["browser", "-s=x", "goto"]);
    assert_eq!(sent(&work)["args"], json!(["-s=x", "goto"]));
}

#[test]
fn the_device_decides_stderr_and_exit() {
    let work = world("device-exit");
    answer(
        &work,
        200,
        0,
        &hub_says(None, "", "device: rm is not allowed here\n", 2, json!({})),
    );
    let out = device(&work, &["run", "rm", "x"]);
    assert_eq!(stderr(&out), "device: rm is not allowed here\n");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn no_args_asks_the_local_hub_and_a_wrong_verb_exits_2() {
    let work = world("device-verb");
    let out = device(&work, &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).starts_with("workflow device: the local hub did not answer"),
        "{out:?}"
    );
    let out = device(&work, &["rm", "x"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out),
        "device: rm is not a verb (whoami, run, browser, tmux, amx, with-secrets)\n"
    );
}

#[test]
fn tmux_and_amx_calls_go_to_the_device_and_keep_its_exit() {
    let work = world("device-tmux");
    answer(
        &work,
        200,
        0,
        &hub_says(Some("mac-mini-m2"), "", "busy: shop\n", 5, json!({})),
    );
    let out = device(&work, &["tmux", "send", "echo hello-7"]);
    assert_eq!(out.status.code(), Some(5), "{out:?}");
    assert_eq!(stderr(&out), "busy: shop\n");
    assert_eq!(sent(&work)["verb"], "tmux");
    assert_eq!(sent(&work)["args"], json!(["send", "echo hello-7"]));
    let out = device(&work, &["amx", "ls"]);
    assert_eq!(out.status.code(), Some(5), "{out:?}");
    assert_eq!(sent(&work)["verb"], "amx");
}

#[test]
fn access_refusal_exits_4() {
    let work = world("device-access");
    for status in [403, 401, 302] {
        answer(&work, status, 0, "<html>login</html>");
        let out = device(&work, &["whoami"]);
        assert_eq!(stderr(&out), "device: refused by Access\n", "{status}");
        assert_eq!(out.status.code(), Some(4), "{status}");
    }
}

#[test]
fn no_device_online_exits_3() {
    let work = world("device-offline");
    answer(&work, 530, 0, "error code: 1033");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "device: no device online\n");
    assert_eq!(out.status.code(), Some(3));
    answer(&work, 0, 7, "");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "device: no device online\n");
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn a_curl_timeout_exits_124() {
    let work = world("device-timeout");
    answer(&work, 0, 28, "");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "device: the device did not answer in 110 s\n");
    assert_eq!(out.status.code(), Some(124));
}

#[test]
fn another_answer_exits_1_with_its_start() {
    let work = world("device-odd");
    answer(&work, 418, 0, "teapot");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "device: unexpected answer 418: teapot\n");
    assert_eq!(out.status.code(), Some(1));
    answer(&work, 200, 0, "not json");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "device: unexpected answer 200: not json\n");
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn a_change_of_machine_says_so_but_the_first_call_does_not() {
    let work = world("device-machine");
    answer(
        &work,
        200,
        0,
        &hub_says(Some("mac-mini-m2"), "", "", 0, json!({})),
    );
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "");
    answer(&work, 200, 0, &hub_says(Some("nuc"), "", "", 0, json!({})));
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "device: now on nuc (was mac-mini-m2)\n");
    let out = device(&work, &["whoami"]);
    assert_eq!(stderr(&out), "");
}

/// Cloudflare's edge gives an origin 100 s and then answers 524 itself
/// (review of m2-device): that is a timeout too, not a strange answer.
#[test]
fn the_edge_s_timeout_exits_124() {
    let work = world("device-edge-timeout");
    answer(&work, 524, 0, "<!DOCTYPE html>error code: 524");
    let out = device(&work, &["browser", "goto", "https://x.test"]);
    assert_eq!(stderr(&out), "device: the device did not answer in 100 s\n");
    assert_eq!(out.status.code(), Some(124));
}

#[test]
fn a_file_outside_the_folder_is_skipped() {
    let work = world("device-escape");
    answer(
        &work,
        200,
        0,
        &hub_says(
            None,
            "",
            "",
            0,
            json!({
                "../x": "a",
                "/tmp/workflow-device-test-x": "b",
                ".claude/settings.json": "d",
                ".playwright-cli/y": "c",
            }),
        ),
    );
    let out = device(&work, &["whoami"]);
    assert!(!work.has("x"));
    assert!(!std::path::Path::new("/tmp/workflow-device-test-x").exists());
    assert_eq!(work.read("cwd/.playwright-cli/y"), "c");
    // Only playwright-cli's own folder: an answer never writes the checkout.
    assert!(!work.has("cwd/.claude/settings.json"));
    let err = stderr(&out);
    assert!(err.contains("device: skipped ../x\n"), "{err}");
    assert!(
        err.contains("device: skipped /tmp/workflow-device-test-x\n"),
        "{err}"
    );
    assert!(
        err.contains("device: skipped .claude/settings.json\n"),
        "{err}"
    );
}

#[test]
fn ask_files_a_question_naming_the_online_devices_and_later_calls_carry_it() {
    let work = world("device-ask");
    let offline = json!({
        "machine": null, "stdout": "", "exit": 3, "files": {},
        "stderr": "device: no device online (device: mac-mini-m2); online: nuc\n",
        "device": "mac-mini-m2", "online": ["nuc"],
    });
    answer(&work, 200, 0, &offline.to_string());
    let out = device(&work, &["ask"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out), format!("#{QUESTION}\n"), "{out:?}");
    assert_eq!(
        stderr(&out),
        format!("device: wait on it with workflow wait-answer {QUESTION}\n")
    );
    assert_eq!(sent(&work)["verb"], "whoami");
    assert_eq!(
        work.read("mem.argv").lines().collect::<Vec<_>>(),
        [
            "ask",
            "--for",
            "saiful",
            "--project",
            "alpha",
            "alpha's device mac-mini-m2 is offline. Which device should this run use?",
            "--option",
            "nuc",
            "--option",
            "I will turn it on",
            "--recommend",
            "nuc",
        ]
    );
    assert_eq!(work.read("state/workflow/device-question"), QUESTION);

    answer(
        &work,
        200,
        0,
        &hub_says(Some("nuc"), "nuc\n", "", 0, json!({})),
    );
    device(&work, &["whoami"]);
    assert_eq!(
        sent(&work),
        json!({"verb": "whoami", "args": [], "session": "session_01X",
               "project": "alpha", "question": QUESTION})
    );
}

#[test]
fn ask_with_no_device_online_recommends_turning_it_on() {
    let work = world("device-ask-none");
    let offline = json!({
        "machine": null, "stdout": "", "exit": 3, "files": {},
        "stderr": "device: no device online (device: nuc); online: none\n",
        "device": "nuc", "online": [],
    });
    answer(&work, 200, 0, &offline.to_string());
    let out = device(&work, &["ask"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let argv = work.read("mem.argv");
    assert!(
        argv.ends_with(
            "is offline. Which device should this run use?\n\
             --option\nI will turn it on\n--recommend\nI will turn it on\n"
        ),
        "{argv}"
    );
}

#[test]
fn ask_with_the_device_online_asks_nothing() {
    let work = world("device-ask-online");
    answer(
        &work,
        200,
        0,
        &hub_says(Some("mac-mini-m2"), "mac-mini-m2\n", "", 0, json!({})),
    );
    let out = device(&work, &["ask"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(
        stdout(&out),
        "device: mac-mini-m2 is online; nothing to ask\n"
    );
    assert!(!work.has("mem.argv"));
    assert!(!work.has("state/workflow/device-question"));
}

#[test]
fn ask_fails_as_a_call_would_when_the_hub_cannot_be_reached() {
    let work = world("device-ask-down");
    answer(&work, 530, 0, "error code: 1033");
    let out = device(&work, &["ask"]);
    assert_eq!(stderr(&out), "device: no device online\n");
    assert_eq!(out.status.code(), Some(3));
    assert!(!work.has("mem.argv"));
}

/// A theme token's shape, 38 characters, built here so no literal of it is
/// committed.
fn theme_token() -> String {
    format!("shpat_{}", "0123456789abcdef".repeat(2))
}

#[test]
fn with_secrets_runs_the_command_here_with_the_device_s_entries_set() {
    let work = world("device-with-secrets");
    let mut said: Value =
        serde_json::from_str(&hub_says(Some("mac-mini-m2"), "", "", 0, json!({}))).unwrap();
    said["secrets"] = json!({"SHOPIFY_CLI_THEME_TOKEN": theme_token()});
    answer(&work, 200, 0, &said.to_string());
    let out = device(
        &work,
        &[
            "with-secrets",
            "pawsome",
            "--",
            "sh",
            "-c",
            "echo ${#SHOPIFY_CLI_THEME_TOKEN} $WORKFLOW_WITH_SECRETS",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout(&out), "38 pawsome\n");
    assert!(!stderr(&out).contains(&theme_token()), "{out:?}");
    let sent = sent(&work);
    assert_eq!(sent["verb"], "with-secrets");
    assert_eq!(sent["project"], "pawsome");
    assert_eq!(
        sent["args"],
        json!([
            "--",
            "sh",
            "-c",
            "echo ${#SHOPIFY_CLI_THEME_TOKEN} $WORKFLOW_WITH_SECRETS"
        ])
    );
}

#[test]
fn with_secrets_keeps_the_command_s_exit_and_the_device_s_refusal() {
    let work = world("device-with-secrets-exit");
    answer(&work, 200, 0, &hub_says(None, "", "", 0, json!({})));
    let out = device(
        &work,
        &["with-secrets", "pawsome", "--", "sh", "-c", "exit 4"],
    );
    assert_eq!(out.status.code(), Some(4), "{out:?}");
    answer(
        &work,
        200,
        0,
        &hub_says(
            None,
            "",
            "device: pass is locked on mac-mini-m2\n",
            7,
            json!({}),
        ),
    );
    let out = device(&work, &["with-secrets", "pawsome", "--", "true"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    assert_eq!(stderr(&out), "device: pass is locked on mac-mini-m2\n");
}

#[test]
fn with_secrets_needs_a_project_and_a_command() {
    let work = world("device-with-secrets-usage");
    for args in [
        &["with-secrets"][..],
        &["with-secrets", "pawsome"],
        &["with-secrets", "pawsome", "--"],
        &["with-secrets", "pawsome", "true"],
    ] {
        let out = device(&work, args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert_eq!(
            stderr(&out),
            "usage: device with-secrets <project> -- <command>\n"
        );
    }
}
