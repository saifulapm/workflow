//! `POST /device`: what this machine does for a session that is somewhere
//! else. `whoami` names the machine, `run` runs one of a few programs,
//! `browser` drives the logged-in Chromium through playwright-cli, and every
//! call leaves one line in the project's mem log.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::http::{Request, Response};
use crate::proc::{Ended, output_within};

/// The only programs `run` starts.
const ALLOWED: [&str; 3] = ["adb", "podman", "shopify"];

/// Under Cloudflare's 100 s for an origin, so a stopped call still reaches
/// the caller through the tunnel.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// How long `systemctl` and `mem` get; both answer in milliseconds.
const HELPER_TIMEOUT: Duration = Duration::from_secs(10);

/// The playwright-cli commands a walk drives a page with, and nothing else:
/// no run-code (node, as this user), no file it names (upload, state-save,
/// screenshot, response-body), no cookies or storage, no dashboard, and
/// nothing that opens, closes or takes a browser session.
const BROWSER_ALLOWED: [&str; 32] = [
    "goto",
    "snapshot",
    "find",
    "click",
    "dblclick",
    "fill",
    "type",
    "hover",
    "select",
    "check",
    "uncheck",
    "drag",
    "press",
    "keydown",
    "keyup",
    "mousemove",
    "mousedown",
    "mouseup",
    "mousewheel",
    "go-back",
    "go-forward",
    "reload",
    "tab-list",
    "tab-new",
    "tab-select",
    "tab-close",
    "resize",
    "dialog-accept",
    "dialog-dismiss",
    "eval",
    "console",
    "detach",
];

/// Flags that name another session or browser, or a file to write.
const BROWSER_FLAGS_REFUSED: [&str; 9] = [
    "-s",
    "--session",
    "--extension",
    "--config",
    "--browser",
    "--cdp",
    "--profile",
    "--filename",
    "--host",
];

/// How long a Chromium the device started gets to put its lock down.
const DEFAULT_CHROMIUM_WAIT: Duration = Duration::from_secs(10);

/// A linked file over this is left out; so is every file once the answer
/// holds this many bytes.
const FILE_LIMIT: u64 = 4 << 20;
const FILES_LIMIT: u64 = 8 << 20;

#[derive(Deserialize)]
struct Call {
    verb: String,
    #[serde(default)]
    args: Vec<String>,
    project: String,
    #[serde(default)]
    session: Option<String>,
}

/// What a call came to: the process-shaped answer, and why it was refused
/// when the device decided not to run anything.
struct Outcome {
    stdout: String,
    stderr: String,
    exit: i32,
    refused: Option<String>,
    files: BTreeMap<String, String>,
}

impl Outcome {
    fn refuse(reason: String) -> Outcome {
        Outcome {
            refused: Some(reason.clone()),
            ..Outcome::fail(2, format!("device: {reason}\n"))
        }
    }

    /// An answer with nothing on stdout, for when the device stopped itself.
    fn fail(exit: i32, stderr: String) -> Outcome {
        Outcome {
            stdout: String::new(),
            stderr,
            exit,
            refused: None,
            files: BTreeMap::new(),
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
            files: BTreeMap::new(),
        },
        "run" => run(&call.args),
        "browser" => browser(call.session.as_deref(), &call.args),
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
            "files": outcome.files,
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
        return Outcome::fail(127, format!("device: {program} was not found\n"));
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
            files: BTreeMap::new(),
        },
        Ended::TimedOut => Outcome::fail(
            124,
            format!(
                "device: {program} ran past {} s and was stopped\n",
                timeout.as_secs_f64()
            ),
        ),
        Ended::Failed(e) => Outcome::fail(126, format!("device: {program} did not start: {e}\n")),
    }
}

fn browser(session: Option<&str>, args: &[String]) -> Outcome {
    let Some(session) = session.filter(|s| {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }) else {
        return Outcome::refuse("browser needs a session".to_string());
    };
    if args.is_empty() {
        return Outcome::refuse("browser needs a command".to_string());
    }
    // The command comes first, so no flag before it can turn it into another.
    if !BROWSER_ALLOWED.contains(&args[0].as_str()) {
        return Outcome::refuse(format!("browser {} is not allowed here", args[0]));
    }
    // The device owns the session, the browser and where files go.
    if let Some(arg) = args[1..].iter().find(|a| {
        let name = a.split('=').next().unwrap_or_default();
        BROWSER_FLAGS_REFUSED.contains(&name)
    }) {
        return Outcome::refuse(format!("browser {arg} is not allowed here"));
    }
    let timeout = timeout();
    let cli = Cli {
        deadline: Instant::now() + timeout,
        path: user_path(),
        session,
        dir: match session_dir(session) {
            Ok(dir) => dir,
            Err(e) => return Outcome::fail(126, format!("device: {e:#}\n")),
        },
    };
    match cli.call(args) {
        Ok(outcome) => outcome,
        Err(Stop::TimedOut) => Outcome::fail(
            124,
            format!(
                "device: browser ran past {} s and was stopped\n",
                timeout.as_secs_f64()
            ),
        ),
        Err(Stop::Failed(e)) => Outcome::fail(126, format!("device: browser did not start: {e}\n")),
    }
}

/// Where a session's playwright-cli runs, so its snapshot files never mix
/// with another session's.
fn session_dir(session: &str) -> anyhow::Result<PathBuf> {
    let dir = crate::config::state_home()?
        .join("hub/device")
        .join(session);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Why a browser call ended before playwright-cli answered it.
enum Stop {
    TimedOut,
    Failed(String),
}

/// playwright-cli for one session, every run of it inside one deadline.
struct Cli<'a> {
    deadline: Instant,
    path: String,
    session: &'a str,
    dir: PathBuf,
}

impl Cli<'_> {
    fn call(&self, args: &[String]) -> Result<Outcome, Stop> {
        if !self.attached()? {
            ensure_chromium(&self.path, self.deadline);
            let flag = format!("-s={}", self.session);
            let token = self.token();
            let done = self.run(&[&flag, "attach", "--extension=chromium"], token.as_deref())?;
            if done.code != Some(0) {
                return Ok(Outcome::fail(
                    done.code.unwrap_or(128),
                    format!(
                        "device: attach failed: {}",
                        String::from_utf8_lossy(&done.stderr)
                    ),
                ));
            }
        }
        let mut argv = vec![format!("-s={}", self.session)];
        argv.extend(args.iter().cloned());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let done = self.run(&argv, None)?;
        let stdout = String::from_utf8_lossy(&done.stdout).into_owned();
        let mut stderr = String::from_utf8_lossy(&done.stderr).into_owned();
        let files = linked_files(&stdout, &self.dir, &mut stderr);
        Ok(Outcome {
            stdout,
            stderr,
            exit: done.code.unwrap_or(128),
            refused: None,
            files,
        })
    }

    /// Whether `playwright-cli list` names this session.
    fn attached(&self) -> Result<bool, Stop> {
        let done = self.run(&["list"], None)?;
        let name = format!("- {}:", self.session);
        Ok(String::from_utf8_lossy(&done.stdout)
            .lines()
            .any(|line| line.trim_end() == name))
    }

    /// The extension's token, so attach does not wait for a click in
    /// Chromium. Attach goes ahead without it when there is none.
    fn token(&self) -> Option<String> {
        let found = find_in(&self.path, "playwright-extension-token")?;
        let mut command = Command::new(found);
        command.env("PATH", &self.path);
        match output_within(&mut command, self.left()) {
            Ended::Exited(done) if done.code == Some(0) => {
                Some(String::from_utf8_lossy(&done.stdout).trim().to_string())
                    .filter(|t| !t.is_empty())
            }
            _ => None,
        }
    }

    fn run(&self, args: &[&str], token: Option<&str>) -> Result<crate::proc::Finished, Stop> {
        // By full path: the hub's own PATH, pinned by its unit, does not
        // have the dotfiles' bin.
        let home = std::env::var_os("HOME").unwrap_or_default();
        let mut command = Command::new(Path::new(&home).join(".dotfiles/bin/playwright-cli"));
        command
            .args(args)
            .env("PATH", &self.path)
            .current_dir(&self.dir);
        if let Some(token) = token {
            command.env("PLAYWRIGHT_MCP_EXTENSION_TOKEN", token);
        }
        match output_within(&mut command, self.left()) {
            Ended::Exited(done) => Ok(done),
            Ended::TimedOut => Err(Stop::TimedOut),
            Ended::Failed(e) => Err(Stop::Failed(e)),
        }
    }

    fn left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// Starts Chromium when its lock names no live process, and waits for the
/// lock to come up. Attach runs afterwards either way and says what is wrong.
fn ensure_chromium(path: &str, deadline: Instant) {
    let Ok(lock) = crate::config::config_home().map(|h| h.join("chromium/SingletonLock")) else {
        return;
    };
    if chromium_up(&lock) {
        return;
    }
    let Some(app_run) = find_in(path, "app-run") else {
        return;
    };
    let spawned = Command::new(app_run)
        .arg("chromium-browser")
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    // app-run daemonizes, so this only reaps it.
    if let Ok(mut child) = spawned {
        std::thread::spawn(move || child.wait());
    }
    let wait = std::env::var("HUB_DEVICE_CHROMIUM_WAIT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CHROMIUM_WAIT);
    let until = deadline.min(Instant::now() + wait);
    while Instant::now() < until && !chromium_up(&lock) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The lock is a symlink to `<host>-<pid>`; Chromium is up while that pid is.
fn chromium_up(lock: &Path) -> bool {
    std::fs::read_link(lock).is_ok_and(|target| {
        target
            .to_string_lossy()
            .rsplit_once('-')
            .and_then(|(_, pid)| pid.parse::<u32>().ok())
            .is_some_and(|pid| Path::new("/proc").join(pid.to_string()).exists())
    })
}

/// The files `stdout` links as `](<path>)` that sit inside `dir`, by the path
/// as written, so the caller can write them where a local run would have.
fn linked_files(stdout: &str, dir: &Path, stderr: &mut String) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let Ok(root) = dir.canonicalize() else {
        return files;
    };
    let mut total = 0;
    for piece in stdout.split("](").skip(1) {
        let Some((link, _)) = piece.split_once(')') else {
            continue;
        };
        let rel = Path::new(link);
        if link.is_empty()
            || rel.is_absolute()
            || rel.components().any(|c| c == Component::ParentDir)
            || files.contains_key(link)
        {
            continue;
        }
        let Ok(found) = dir.join(rel).canonicalize() else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&found) else {
            continue;
        };
        if !found.starts_with(&root) || !meta.is_file() {
            continue;
        }
        let size = meta.len();
        if size > FILE_LIMIT || total + size > FILES_LIMIT {
            stderr.push_str(&format!("device: left out {link} ({size} bytes)\n"));
            continue;
        }
        let Ok(bytes) = std::fs::read(&found) else {
            continue;
        };
        total += size;
        files.insert(
            link.to_string(),
            String::from_utf8_lossy(&bytes).into_owned(),
        );
    }
    files
}

pub fn timeout() -> Duration {
    std::env::var("HUB_DEVICE_TIMEOUT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// The user manager's PATH, which has what the user's shell has (adb, mise's
/// node); the hub's own is pinned by its unit to a short list. Falls back to
/// the hub's own when systemctl cannot say.
pub(crate) fn user_path() -> String {
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

/// The answer when no machine can take the call: exit 3 with `why`, logged
/// under the machine `none`.
pub fn unserved(request: &Request, why: &str) -> Response {
    let call: Call = match serde_json::from_slice(&request.body) {
        Ok(call) => call,
        Err(e) => return Response::text(400, &format!("expected verb, args and project: {e}")),
    };
    let outcome = Outcome {
        exit: 3,
        ..Outcome::refuse(why.to_string())
    };
    log("none", &call, &outcome);
    Response::new(
        200,
        "application/json",
        json!({
            "machine": null,
            "stdout": outcome.stdout,
            "stderr": outcome.stderr,
            "exit": outcome.exit,
            "files": {},
        })
        .to_string(),
    )
}
