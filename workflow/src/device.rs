//! `workflow device <verb> [args...]`: one call to the routed device's hub,
//! played back here as if the program had run on this machine.

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::exit;

const VERBS: [&str; 3] = ["whoami", "run", "browser"];
const DEFAULT_URL: &str = "https://device.on-rexa.com";
/// Cloudflare's edge gives the device 100 s and then answers 524; the
/// device stops a call at 90 s, so a stopped call still gets back.
const DEFAULT_TIMEOUT_S: u64 = 110;
const EDGE_TIMEOUT_S: u64 = 100;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The session id, reduced to what the hub accepts: [A-Za-z0-9_-], 64 chars.
fn session() -> String {
    let raw = env("CLAUDE_CODE_REMOTE_SESSION_ID").unwrap_or_else(|| {
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .or_else(|_| std::fs::read_to_string("/etc/hostname"))
            .unwrap_or_default();
        format!("local-{}", host.trim())
    });
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(64)
        .collect()
}

/// The last machine that answered, kept so a change of device can be said.
fn machine_file() -> Option<PathBuf> {
    let state = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| crate::paths::home().map(|h| h.join(".local/state")))?;
    Some(state.join("workflow/device-machine"))
}

fn say(msg: &str) {
    eprintln!("device: {msg}");
}

/// How a POST to the hub ended.
enum Reply {
    Answer(u32, String),
    /// curl's own exit code, when it failed.
    CurlFailed(i32),
}

/// The one transport: curl, with the body on stdin.
fn post(body: &str, timeout_s: u64) -> Reply {
    let url = format!(
        "{}/device",
        env("WORKFLOW_DEVICE_URL").unwrap_or_else(|| DEFAULT_URL.to_string())
    );
    let tmp = std::env::temp_dir().join(format!("workflow-device-{}", std::process::id()));
    let spawned = Command::new("curl")
        .args(["-sS", "--max-time", &timeout_s.to_string()])
        .args([
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ])
        .arg("-o")
        .arg(&tmp)
        .args(["-w", "%{http_code}"])
        .arg(&url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return Reply::CurlFailed(7);
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let Ok(out) = child.wait_with_output() else {
        return Reply::CurlFailed(7);
    };
    let answer = std::fs::read(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        return Reply::CurlFailed(out.status.code().unwrap_or(7));
    }
    let status = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    Reply::Answer(status, String::from_utf8_lossy(&answer).to_string())
}

/// A path under `.playwright-cli/`, where playwright-cli writes, and nowhere
/// else: the folder is a checkout, and its settings are not the device's.
fn safe(rel: &str) -> bool {
    let path = Path::new(rel);
    path.starts_with(".playwright-cli")
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn write_files(files: &Value) {
    let Some(files) = files.as_object() else {
        return;
    };
    for (rel, text) in files {
        let text = text.as_str().unwrap_or_default();
        let written = safe(rel) && {
            let path = Path::new(rel);
            path.parent()
                .is_none_or(|dir| std::fs::create_dir_all(dir).is_ok())
                && std::fs::write(path, text).is_ok()
        };
        if !written {
            say(&format!("skipped {rel}"));
        }
    }
}

/// Say so when the answering machine is not the one that answered last.
fn note_machine(machine: &str) {
    let Some(file) = machine_file() else {
        return;
    };
    if let Ok(was) = std::fs::read_to_string(&file) {
        let was = was.trim();
        if was != machine {
            say(&format!("now on {machine} (was {was})"));
        }
    }
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, machine);
}

/// `workflow device <verb> [args...]`.
pub fn cmd_device(args: &[String]) -> i32 {
    let Some((verb, rest)) = args.split_first() else {
        eprintln!("usage: workflow device <whoami|run|browser> [args...]");
        return exit::USAGE;
    };
    if !VERBS.contains(&verb.as_str()) {
        say(&format!("{verb} is not a verb (whoami, run, browser)"));
        return exit::USAGE;
    }
    let Some(project) =
        env("MEM_PROJECT").or_else(|| crate::memcli::project_current().map(|p| p.name))
    else {
        say("no project here (set MEM_PROJECT)");
        return exit::USAGE;
    };
    let body = json!({"verb": verb, "args": rest, "session": session(), "project": project});
    let timeout_s = env("WORKFLOW_DEVICE_TIMEOUT_S")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_S);
    let (status, text) = match post(&body.to_string(), timeout_s) {
        Reply::Answer(status, text) => (status, text),
        Reply::CurlFailed(6 | 7) => {
            say("no device online");
            return 3;
        }
        Reply::CurlFailed(28) => {
            say(&format!("the device did not answer in {timeout_s} s"));
            return 124;
        }
        Reply::CurlFailed(code) => {
            say(&format!("curl failed with exit {code}"));
            return exit::FAILED;
        }
    };
    match status {
        401 | 403 | 302 => {
            say("refused by Access");
            return 4;
        }
        524 => {
            say(&format!("the device did not answer in {EDGE_TIMEOUT_S} s"));
            return 124;
        }
        502 | 503 | 504 | 530 => {
            say("no device online");
            return 3;
        }
        _ => {}
    }
    let answer = (status == 200)
        .then(|| serde_json::from_str::<Value>(&text).ok())
        .flatten()
        .filter(|a| a.get("exit").is_some_and(Value::is_i64));
    let Some(answer) = answer else {
        let start: String = text.chars().take(200).collect();
        say(&format!("unexpected answer {status}: {start}"));
        return exit::FAILED;
    };
    if let Some(machine) = answer["machine"].as_str() {
        note_machine(machine);
    }
    print!("{}", answer["stdout"].as_str().unwrap_or_default());
    let _ = std::io::stdout().flush();
    eprint!("{}", answer["stderr"].as_str().unwrap_or_default());
    write_files(&answer["files"]);
    answer["exit"].as_i64().unwrap_or(1) as i32
}
