//! `POST /device`: what this machine does for a session that is somewhere
//! else. `whoami` names the machine, `run` runs one of a few programs, and
//! every call leaves one line in the project's mem log.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::http::{Request, Response};
use crate::proc::{Ended, output_within};

/// The only programs `run` starts.
const ALLOWED: [&str; 3] = ["adb", "podman", "shopify"];

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// How long `systemctl` and `mem` get; both answer in milliseconds.
const HELPER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Call {
    verb: String,
    #[serde(default)]
    args: Vec<String>,
    project: String,
}

/// What a call came to: the process-shaped answer, and why it was refused
/// when the device decided not to run anything.
struct Outcome {
    stdout: String,
    stderr: String,
    exit: i32,
    refused: Option<String>,
}

impl Outcome {
    fn refuse(reason: String) -> Outcome {
        Outcome {
            stdout: String::new(),
            stderr: format!("device: {reason}\n"),
            exit: 2,
            refused: Some(reason),
        }
    }
}

pub fn serve(machine: &str, request: &Request) -> Response {
    let call: Call = match serde_json::from_slice(&request.body) {
        Ok(call) => call,
        Err(e) => return Response::text(400, &format!("expected verb, args and project: {e}")),
    };
    let outcome = match call.verb.as_str() {
        "whoami" => Outcome {
            stdout: format!("{machine}\n"),
            stderr: String::new(),
            exit: 0,
            refused: None,
        },
        "run" => run(&call.args),
        "browser" => Outcome::refuse("browser is not ready here".to_string()),
        other => return Response::text(400, &format!("not a device verb: {other}")),
    };
    log(machine, &call, &outcome);
    Response::new(
        200,
        "application/json",
        json!({
            "machine": machine,
            "stdout": outcome.stdout,
            "stderr": outcome.stderr,
            "exit": outcome.exit,
            "files": {},
        })
        .to_string(),
    )
}

fn run(args: &[String]) -> Outcome {
    let Some((program, rest)) = args.split_first() else {
        return Outcome::refuse("run needs a program".to_string());
    };
    if !ALLOWED.contains(&program.as_str()) {
        return Outcome::refuse(format!("{program} is not allowed here"));
    }
    let path = user_path();
    let Some(found) = find_in(&path, program) else {
        return Outcome {
            stdout: String::new(),
            stderr: format!("device: {program} was not found\n"),
            exit: 127,
            refused: None,
        };
    };
    let timeout = timeout();
    let mut command = Command::new(found);
    command.args(rest).env("PATH", &path);
    match output_within(&mut command, timeout) {
        Ended::Exited(done) => Outcome {
            stdout: String::from_utf8_lossy(&done.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&done.stderr).into_owned(),
            // A child killed by a signal has no code, and proc does not keep
            // the signal, so this is 128 alone.
            exit: done.code.unwrap_or(128),
            refused: None,
        },
        Ended::TimedOut => Outcome {
            stdout: String::new(),
            stderr: format!(
                "device: {program} ran past {} s and was stopped\n",
                timeout.as_secs_f64()
            ),
            exit: 124,
            refused: None,
        },
        Ended::Failed(e) => Outcome {
            stdout: String::new(),
            stderr: format!("device: {program} did not start: {e}\n"),
            exit: 126,
            refused: None,
        },
    }
}

fn timeout() -> Duration {
    std::env::var("HUB_DEVICE_TIMEOUT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// The user manager's PATH, which has what the user's shell has (adb, mise's
/// node); the hub's own is pinned by its unit to a short list. Falls back to
/// the hub's own when systemctl cannot say.
fn user_path() -> String {
    let own = std::env::var("PATH").unwrap_or_default();
    let mut command = Command::new("systemctl");
    command.args(["--user", "show-environment"]);
    match output_within(&mut command, HELPER_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => String::from_utf8_lossy(&done.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("PATH="))
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .unwrap_or(own),
        _ => own,
    }
}

fn find_in(path: &str, program: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// One line in the project's mem log for the call.
fn log(machine: &str, call: &Call, outcome: &Outcome) {
    let mut line = format!("device {machine} {}", call.verb);
    for arg in &call.args {
        line.push(' ');
        line.push_str(arg);
    }
    line.push_str(&format!(" exit {}", outcome.exit));
    if let Some(reason) = &outcome.refused {
        line.push_str(&format!(" refused: {reason}"));
    }
    let mut command = Command::new("mem");
    command.args([&format!("--project={}", call.project), "log", "--", &line]);
    match output_within(&mut command, HELPER_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => {}
        Ended::Exited(done) => eprintln!(
            "hub: device: mem log failed: {}",
            String::from_utf8_lossy(&done.stderr).trim()
        ),
        Ended::TimedOut => eprintln!("hub: device: mem log took too long"),
        Ended::Failed(e) => eprintln!("hub: device: mem log did not start: {e}"),
    }
}
