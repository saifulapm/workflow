//! `workflow device <verb> [args...]`: one call to the chosen device's hub,
//! played back here as if the program had run on this machine. `use` and the
//! bare form choose and read the machine, on this machine's local hub.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::exit;

const VERBS: [&str; 5] = ["whoami", "run", "browser", "tmux", "amx"];
/// What `device use` takes, and the machine name each stands for.
const MACHINES: [(&str, &str); 3] = [
    ("mini", "mac-mini-m2"),
    ("nuc", "nuc"),
    ("macbook", "macbook-m2"),
];
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

/// `name` under this machine's `workflow` state folder.
fn state_file(name: &str) -> Option<PathBuf> {
    let state = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| crate::paths::home().map(|h| h.join(".local/state")))?;
    Some(state.join("workflow").join(name))
}

/// The last machine that answered, kept so a change of device can be said.
fn machine_file() -> Option<PathBuf> {
    state_file("device-machine")
}

/// The question `device ask` filed, which every later call names so the hub
/// can follow Saiful's answer to it.
fn question_file() -> Option<PathBuf> {
    state_file("device-question")
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

fn say_choice(msg: impl AsRef<str>) {
    eprintln!("workflow device: {}", msg.as_ref());
}

/// This machine's local hub's socket: `HUB_LOCAL_SOCKET`, else `hub/local.sock`
/// under the state directory, the hub's own rule.
fn local_socket() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HUB_LOCAL_SOCKET").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let state = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| crate::paths::home().map(|h| h.join(".local/state")))?;
    Some(state.join("hub/local.sock"))
}

/// One request to the local hub on its socket: its status and body.
fn ask(socket: &Path, method: &str, body: &str) -> std::io::Result<(String, String)> {
    let mut stream = UnixStream::connect(socket)?;
    let timeout = Some(Duration::from_secs(5));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let request = format!(
        "{method} /device/choice HTTP/1.1\r\nHost: local\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    let (head, answer) = reply.split_once("\r\n\r\n").unwrap_or((&reply, ""));
    let status = head.split_whitespace().nth(1).unwrap_or("").to_string();
    Ok((status, answer.to_string()))
}

/// `workflow device use mini|nuc|macbook` and bare `workflow device`: set or
/// read which machine this machine's local hub sends device calls to.
fn cmd_choice(name: Option<&str>) -> i32 {
    let Some(socket) = local_socket() else {
        say_choice("cannot find this machine's state directory; set HOME");
        return exit::FAILED;
    };
    let reply = match name {
        None => ask(&socket, "GET", ""),
        Some(machine) => ask(&socket, "POST", &json!({ "device": machine }).to_string()),
    };
    let (status, answer) = match reply {
        Ok(r) => r,
        Err(err) => {
            say_choice(format!(
                "the local hub did not answer on {}: {err}",
                socket.display()
            ));
            return exit::FAILED;
        }
    };
    if status != "200" {
        say_choice(format!(
            "the local hub refused: HTTP {status} {}",
            answer.trim()
        ));
        return exit::FAILED;
    }
    let chosen = serde_json::from_str::<Value>(&answer)
        .ok()
        .and_then(|a| a["device"].as_str().map(str::to_string));
    let Some(chosen) = chosen else {
        say_choice("the local hub's answer is not a device");
        return exit::FAILED;
    };
    println!("device: {chosen}");
    exit::OK
}

/// `workflow device [use mini|nuc|macbook | <verb> [args...]]`.
pub fn cmd_device(args: &[String]) -> i32 {
    let Some((verb, rest)) = args.split_first() else {
        return cmd_choice(None);
    };
    if verb == "use" {
        let machine = match rest {
            [name] => MACHINES.iter().find(|(short, _)| short == name),
            _ => None,
        };
        let Some((_, machine)) = machine else {
            eprintln!("usage: workflow device use mini|nuc|macbook");
            return exit::USAGE;
        };
        return cmd_choice(Some(machine));
    }
    if verb != "ask" && !VERBS.contains(&verb.as_str()) {
        say(&format!(
            "{verb} is not a verb (whoami, run, browser, tmux, amx)"
        ));
        return exit::USAGE;
    }
    let Some(project) =
        env("MEM_PROJECT").or_else(|| crate::memcli::project_current().map(|p| p.name))
    else {
        say("no project here (set MEM_PROJECT)");
        return exit::USAGE;
    };
    if verb == "ask" {
        return cmd_ask(&project);
    }
    match call(verb, rest, &project) {
        Ok(answer) => play_back(&answer),
        Err(code) => code,
    }
}

/// One call to the device's hub: its answer, or the exit code once the
/// reason it has none is said.
fn call(verb: &str, args: &[String], project: &str) -> Result<Value, i32> {
    let mut body = json!({"verb": verb, "args": args, "session": session(), "project": project});
    let question = question_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty());
    if let Some(question) = question {
        body["question"] = json!(question);
    }
    let timeout_s = env("WORKFLOW_DEVICE_TIMEOUT_S")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_S);
    let (status, text) = match post(&body.to_string(), timeout_s) {
        Reply::Answer(status, text) => (status, text),
        Reply::CurlFailed(6 | 7) => {
            say("no device online");
            return Err(3);
        }
        Reply::CurlFailed(28) => {
            say(&format!("the device did not answer in {timeout_s} s"));
            return Err(124);
        }
        Reply::CurlFailed(code) => {
            say(&format!("curl failed with exit {code}"));
            return Err(exit::FAILED);
        }
    };
    match status {
        401 | 403 | 302 => {
            say("refused by Access");
            return Err(4);
        }
        524 => {
            say(&format!("the device did not answer in {EDGE_TIMEOUT_S} s"));
            return Err(124);
        }
        502 | 503 | 504 | 530 => {
            say("no device online");
            return Err(3);
        }
        _ => {}
    }
    let answer = (status == 200)
        .then(|| serde_json::from_str::<Value>(&text).ok())
        .flatten()
        .filter(|a| a.get("exit").is_some_and(Value::is_i64));
    answer.ok_or_else(|| {
        let start: String = text.chars().take(200).collect();
        say(&format!("unexpected answer {status}: {start}"));
        exit::FAILED
    })
}

/// `device ask`: when the device is offline, ask Saiful which online device
/// this run should use, and keep the question so later calls name it.
fn cmd_ask(project: &str) -> i32 {
    let answer = match call("whoami", &[], project) {
        Ok(answer) => answer,
        Err(code) => return code,
    };
    let exit = answer["exit"].as_i64();
    if exit == Some(0) {
        let machine = answer["machine"].as_str().unwrap_or("the device");
        println!("device: {machine} is online; nothing to ask");
        return exit::OK;
    }
    let target = answer["device"].as_str();
    let online = answer["online"].as_array();
    let (Some(3), Some(target), Some(online)) = (exit, target, online) else {
        return play_back(&answer);
    };
    let turn_on = "I will turn it on";
    let online: Vec<&str> = online.iter().filter_map(Value::as_str).collect();
    let text = format!("{project}'s device {target} is offline. Which device should this run use?");
    let mut args = vec!["ask", "--for", "saiful", "--project", project, &text];
    for option in online.iter().chain([&turn_on]) {
        args.extend(["--option", option]);
    }
    args.extend(["--recommend", online.first().unwrap_or(&turn_on)]);
    let asked = Command::new(crate::memcli::bin())
        .args(&args)
        .stderr(Stdio::inherit())
        .output();
    let printed = match asked {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => {
            say(&format!("mem ask failed with {}", out.status));
            return exit::FAILED;
        }
        Err(err) => {
            say(&format!("mem did not start: {err}"));
            return exit::FAILED;
        }
    };
    let id = printed
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('#'))
        .find(|id| id.len() == 8 && id.chars().all(|c| c.is_ascii_alphanumeric()));
    let Some(id) = id else {
        say(&format!("mem ask printed no question: {}", printed.trim()));
        return exit::FAILED;
    };
    let written = question_file().is_some_and(|file| {
        file.parent()
            .is_none_or(|dir| std::fs::create_dir_all(dir).is_ok())
            && std::fs::write(&file, id).is_ok()
    });
    if !written {
        say(&format!("cannot keep #{id} in the state folder"));
        return exit::FAILED;
    }
    println!("#{id}");
    say(&format!("wait on it with workflow wait-answer {id}"));
    exit::OK
}

/// The hub's answer played back here, as if the program had run here.
fn play_back(answer: &Value) -> i32 {
    if let Some(machine) = answer["machine"].as_str() {
        note_machine(machine);
    }
    print!("{}", answer["stdout"].as_str().unwrap_or_default());
    let _ = std::io::stdout().flush();
    eprint!("{}", answer["stderr"].as_str().unwrap_or_default());
    write_files(&answer["files"]);
    answer["exit"].as_i64().unwrap_or(1) as i32
}
