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
    branch: Option<String>,
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
    json!({"argv": sent, "stdin": input, "remote": remote, "branch": branch})
}

/// Runs this invocation on the server at `url` and returns the exit code.
pub fn run(url: &str) -> i32 {
    let (code, stdout, stderr) = run_captured(url);
    let _ = std::io::stdout().write_all(stdout.as_bytes());
    let _ = std::io::stderr().write_all(stderr.as_bytes());
    code
}

/// `run`, handing back what the server's mem printed instead of printing it.
pub fn run_captured(url: &str) -> (i32, String, String) {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env_project = std::env::var("MEM_PROJECT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let body = request(
        &argv,
        env_project.as_deref(),
        read_stdin,
        origin(),
        git(&["branch", "--show-current"]),
    );
    match post(url, &body.to_string()) {
        Ok(reply) => {
            let field = |key: &str| {
                reply
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let code = reply
                .get("code")
                .and_then(Value::as_i64)
                .map_or(exit::STORE, |c| c as i32);
            (code, field("stdout"), field("stderr"))
        }
        Err(why) => (exit::STORE, String::new(), format!("mem: {url} {why}\n")),
    }
}

/// `run`, for a machine whose local hub carries questions and answers: the
/// server writes a question, then the local hub is handed it, and a wait is
/// the local hub's alone, since the server refuses one.
pub fn run_with_local_hub(url: &str, cli: &crate::cli::Cli) -> i32 {
    let socket = match crate::paths::Dirs::from_env() {
        Ok(dirs) => crate::localhub::socket_path(&dirs.state),
        Err(_) => return run(url),
    };
    match &cli.command {
        Some(crate::cli::Command::Questions {
            wait: Some(id),
            timeout,
            ..
        }) => {
            let Some(within) = crate::questions::parse_timeout(timeout) else {
                return run(url);
            };
            let short = crate::localhub::short_id(id);
            match crate::localhub::wait_until(&socket, &short, within) {
                crate::localhub::Waited::Answered(text) => {
                    print!("{text}");
                    exit::OK
                }
                crate::localhub::Waited::Nothing => {
                    eprintln!(
                        "mem: #{short} is still unanswered — park the work with `mem handoff`"
                    );
                    exit::WAIT_TIMEOUT
                }
                crate::localhub::Waited::NoHub => run(url),
            }
        }
        Some(crate::cli::Command::Ask {
            question,
            options,
            audience,
            ..
        }) => {
            let (code, stdout, stderr) = run_captured(url);
            let _ = std::io::stdout().write_all(stdout.as_bytes());
            let _ = std::io::stderr().write_all(stderr.as_bytes());
            // `#<id>`, or with --json the reply's short_id.
            let id = stdout
                .lines()
                .find_map(|l| l.trim().strip_prefix('#'))
                .map(str::to_string)
                .or_else(|| {
                    serde_json::from_str::<Value>(&stdout).ok()?["short_id"]
                        .as_str()
                        .map(str::to_string)
                });
            if code == exit::OK
                && let Some(id) = id
            {
                let project = cli.project.clone().or_else(|| {
                    std::env::var("MEM_PROJECT")
                        .ok()
                        .map(|p| p.trim().to_string())
                        .filter(|p| !p.is_empty())
                });
                crate::localhub::ask(
                    &socket,
                    &crate::localhub::Asked {
                        id: &id,
                        project: project.as_deref(),
                        text: question,
                        options,
                        audience: Some(crate::verbs::person_or_orchestrator(
                            audience.and_then(crate::cli::Audience::stored),
                        )),
                    },
                );
            }
            code
        }
        _ => run(url),
    }
}

fn read_stdin() -> String {
    let mut text = String::new();
    let _ = std::io::stdin().read_to_string(&mut text);
    text
}

/// `git remote get-url origin` of the working directory's repo.
fn origin() -> Option<String> {
    git(&["remote", "get-url", "origin"])
}

/// What `git <args>` prints in the working directory, trimmed; none when it
/// fails or prints nothing. The branch tells the server which of the
/// projects sharing a remote a cloud run is.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
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
