//! Remote mode: with `MEM_REMOTE` set, every verb runs on the mem server's
//! store. The client sends its argv to `POST $MEM_REMOTE/cli` and prints what
//! the server's mem printed, so this machine's store and index are never
//! opened.
//!
//! HTTP goes through `curl` as a child process, argv and never a shell: mem
//! has no HTTP crate, and the hub calls curl the same way.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::exit;

/// curl's `--max-time`, in seconds.
const TIMEOUT_SECS: &str = "60";

/// The server URL, when `MEM_REMOTE` names one.
pub fn url() -> Option<String> {
    std::env::var("MEM_REMOTE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The request body for `argv` (without argv[0]). `MEM_PROJECT` goes first
/// as `--project=` unless the argv names a project itself; `stdin` is read
/// only for a verb given `--stdin`.
pub fn request(
    argv: &[String],
    env_project: Option<&str>,
    stdin: impl FnOnce() -> String,
    remote: Option<String>,
) -> Value {
    let named = argv
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--project" || a.starts_with("--project="));
    let mut sent = Vec::with_capacity(argv.len() + 1);
    if let Some(project) = env_project.filter(|_| !named) {
        sent.push(format!("--project={project}"));
    }
    sent.extend(argv.iter().cloned());
    let input = argv.iter().any(|a| a == "--stdin").then(stdin);
    json!({"argv": sent, "stdin": input, "remote": remote})
}

/// Runs this invocation on the server at `url` and returns the exit code.
pub fn run(url: &str) -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env_project = std::env::var("MEM_PROJECT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let body = request(&argv, env_project.as_deref(), read_stdin, origin());
    match post(url, &body.to_string()) {
        Ok(reply) => {
            let field = |key: &str| reply.get(key).and_then(Value::as_str).unwrap_or("");
            let _ = std::io::stdout().write_all(field("stdout").as_bytes());
            let _ = std::io::stderr().write_all(field("stderr").as_bytes());
            reply
                .get("code")
                .and_then(Value::as_i64)
                .map_or(exit::STORE, |c| c as i32)
        }
        Err(why) => {
            eprintln!("mem: {url} {why}");
            exit::STORE
        }
    }
}

fn read_stdin() -> String {
    let mut text = String::new();
    let _ = std::io::stdin().read_to_string(&mut text);
    text
}

/// `git remote get-url origin` of the working directory's repo.
fn origin() -> Option<String> {
    let out = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !url.is_empty()).then_some(url)
}

/// POSTs `body` to `<url>/cli` and returns the server's reply, or the rest of
/// the line that says why there is none.
fn post(url: &str, body: &str) -> Result<Value, String> {
    let mut command = Command::new("curl");
    command.args([
        "-sS",
        "--max-time",
        TIMEOUT_SECS,
        "-X",
        "POST",
        "-H",
        "Content-Type: application/json",
    ]);
    if let Ok(header) = std::env::var("MEM_REMOTE_HEADER")
        && !header.trim().is_empty()
    {
        command.args(["-H", header.trim()]);
    }
    // The status goes on a line of its own after the body.
    command
        .args(["--data-binary", "@-", "-w", "\n%{http_code}"])
        .arg(format!("{}/cli", url.trim_end_matches('/')))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!("did not answer: {why}"));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (reply, status) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));
    if status != "200" {
        return Err(format!("answered {status}"));
    }
    serde_json::from_str(reply).map_err(|_| format!("answered {status} with no reply mem reads"))
}
