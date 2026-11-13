//! `POST /device`: what this machine does for a session that is somewhere
//! else. `whoami` names the machine, `run` runs one of a few programs,
//! `browser` drives the logged-in Chromium through playwright-cli, `tmux`
//! drives the session's own tmux session (device_tmux), `amx` drives the one
//! amx agent the thread started (device_amx), and every call leaves one line
//! in the project's mem log.

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::http::{Request, Response};
use crate::memcli::MemCli;
use crate::proc::{Ended, output_within};

/// The only programs `run` starts.
const ALLOWED: [&str; 3] = ["adb", "podman", "shopify"];

/// The machines that serve device calls. A project's own device counts only
/// when one of them set it, so a cloud thread's mem cannot move its calls.
pub const DEVICES: [&str; 3] = ["mac-mini-m2", "nuc", "macbook-m2"];

/// Under Cloudflare's 100 s for an origin, so a stopped call still reaches
/// the caller through the tunnel.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// How long `systemctl` and `mem` get; both answer in milliseconds.
pub(crate) const HELPER_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Marks the session's current page as one it opened. The session's
/// playwright-cli daemon keeps its Page objects from call to call, so the mark
/// lasts as long as the session. The extension also hands a session any tab
/// Saiful drags into its group, and that one carries no mark.
const TAG_CURRENT: &str = "async page => { page.__hubTab = true; }";

/// The start of the hub's code that asks whether a page is the session's
/// own: one it marked, or one such a page opened, as a link with
/// target=_blank does.
macro_rules! own {
    () => {
        "const own = async p => { if (p.__hubTab) return true; \
         const o = await p.opener().catch(() => null); return !!o && own(o); }; "
    };
}

/// Closes every page the session opened, each judged before any closes, so
/// a popup still has its opener. Closing the last one ends the extension's
/// connection and the session with it, which playwright-cli reports as
/// "Session closed" on stderr with exit 1.
const CLOSE_OWN: &str = concat!(
    "async page => { ",
    own!(),
    "const mine = []; for (const p of page.context().pages()) if (await own(p)) mine.push(p); \
     for (const p of mine) await p.close(); return mine.length; }"
);

/// -1 when the current page is the session's own, else the number of the
/// first one that is, else -2.
const FIRST_OWN: &str = concat!(
    "async page => { ",
    own!(),
    "if (await own(page)) return -1; const pages = page.context().pages(); \
     for (let i = 0; i < pages.length; i++) if (await own(pages[i])) return i; return -2; }"
);

/// How long a session may go without a call before the hub closes its tabs.
const DEFAULT_IDLE: Duration = Duration::from_secs(15 * 60);

/// The file in a session's folder that says when its last call ended, and
/// for which project.
const LAST_CALL: &str = "last-call.json";

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
    /// The run's question that asked Saiful which device to use.
    #[serde(default)]
    question: Option<String>,
}

/// What a call came to: the process-shaped answer, and why it was refused
/// when the device decided not to run anything.
pub(crate) struct Outcome {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit: i32,
    pub(crate) refused: Option<String>,
    pub(crate) files: BTreeMap<String, String>,
}

impl Outcome {
    pub(crate) fn refuse(reason: String) -> Outcome {
        Outcome {
            refused: Some(reason.clone()),
            ..Outcome::fail(2, format!("device: {reason}\n"))
        }
    }

    /// An answer that exits 0 with `stdout`.
    pub(crate) fn done(stdout: String) -> Outcome {
        Outcome {
            stdout,
            ..Outcome::fail(0, String::new())
        }
    }

    /// An answer with nothing on stdout, for when the device stopped itself.
    pub(crate) fn fail(exit: i32, stderr: String) -> Outcome {
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
    let mut outcome = match call.verb.as_str() {
        "whoami" => Outcome {
            stdout: format!("{machine}\n"),
            stderr: String::new(),
            exit: 0,
            refused: None,
            files: BTreeMap::new(),
        },
        "run" => run(&call.args),
        "browser" => browser(call.session.as_deref(), &call.project, &call.args),
        "tmux" => crate::device_tmux::call(call.session.as_deref(), &call.project, &call.args),
        "amx" => crate::device_amx::call(call.session.as_deref(), &call.project, &call.args),
        other => return Response::text(400, &format!("not a device verb: {other}")),
    };
    // What a terminal or an agent printed may hold a token.
    if matches!(call.verb.as_str(), "tmux" | "amx") {
        outcome.stdout = crate::mask::mask(&outcome.stdout);
        outcome.stderr = crate::mask::mask(&outcome.stderr);
    }
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

fn browser(session: Option<&str>, project: &str, args: &[String]) -> Outcome {
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
    let held = session_lock(session);
    let _held = held.lock().unwrap_or_else(|e| e.into_inner());
    let called = cli.call(args);
    let record = cli.dir.join(LAST_CALL);
    // A detach that could not close the tabs keeps the session's record,
    // so the idle sweep tries again.
    if args[0] == "detach" && matches!(&called, Ok(outcome) if outcome.exit == 0) {
        let _ = std::fs::remove_file(&record);
    } else {
        let last = LastCall {
            project: project.to_string(),
            at: unix_now(),
        };
        if let Err(e) = std::fs::write(&record, serde_json::to_vec(&last).unwrap_or_default()) {
            eprintln!("hub: device: cannot write {}: {e}", record.display());
        }
    }
    match called {
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

/// When a session's last call ended, and for which project.
#[derive(Serialize, Deserialize)]
struct LastCall {
    project: String,
    /// Unix milliseconds.
    at: u64,
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// One lock per session, so the sweep never ends a session while a call of
/// it runs.
fn session_lock(session: &str) -> Arc<Mutex<()>> {
    static LOCKS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
        LazyLock::new(Default::default);
    let mut locks = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(locks.entry(session.to_string()).or_default())
}

/// `HUB_DEVICE_IDLE_MS`, else 15 minutes.
pub(crate) fn idle_limit(ms: Option<&str>) -> Duration {
    ms.and_then(|ms| ms.parse().ok())
        .map_or(DEFAULT_IDLE, Duration::from_millis)
}

/// Runs until the process ends: every `HUB_DEVICE_SWEEP_MS` (at most 15 s)
/// closes the tabs of each session whose last call ended longer than the
/// idle limit ago, or whose cloud run has landed or parked, and detaches it,
/// and ends the tmux job when it is due (device_tmux).
pub fn sweep_sessions() -> ! {
    let limit = idle_limit(std::env::var("HUB_DEVICE_IDLE_MS").ok().as_deref());
    let every = std::env::var("HUB_DEVICE_SWEEP_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(
            (limit / 2).min(Duration::from_secs(15)),
            Duration::from_millis,
        );
    let mem = MemCli::new();
    loop {
        std::thread::sleep(every);
        // Read once a round, and only when a session is there to judge.
        let projects = std::cell::OnceCell::new();
        crate::device_tmux::sweep(&mem, &projects);
        crate::device_amx::sweep(&mem, &projects);
        let Ok(root) = crate::config::state_home().map(|h| h.join("hub/device")) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            end_when(&name, &entry.path(), |last| {
                Duration::from_millis(unix_now().saturating_sub(last.at)) >= limit
                    || run_over(&name, &last.project, &mem, &projects)
            });
        }
    }
}

/// Whether `session` is the cloud run of `project` and that run has landed,
/// parked or ended. A session that is no cloud run's, such as a local walk's
/// `local-<host>`, never is.
pub(crate) fn run_over(
    session: &str,
    project: &str,
    mem: &MemCli,
    projects: &std::cell::OnceCell<Vec<serde_json::Value>>,
) -> bool {
    // A thread sends CLAUDE_CODE_REMOTE_SESSION_ID, and the runner record
    // names the same session with `session_` for `cse_`, as fleet reads it.
    let runner = match session.strip_prefix("cse_") {
        Some(rest) => format!("session_{rest}"),
        None => session.to_string(),
    };
    let rows = projects.get_or_init(|| mem.refresh(&["projects", "--json"]).rows("projects"));
    rows.iter()
        .filter(|row| row["name"].as_str() == Some(project))
        .any(|row| {
            let summary = crate::page_home::summary_of(row);
            crate::live::cloud_run(
                &summary,
                || crate::doorbell::handoff_body(mem, project),
                jiff::Timestamp::now(),
            )
            .is_some_and(|run| run.id == runner && run.state != crate::live::CloudState::Live)
        })
}

/// Ends `session` when `due` holds for its last call: closes the tabs it
/// opened while Chromium is up, then detaches it. A Chromium that is down
/// stays down, since only attach starts it.
fn end_when(session: &str, dir: &Path, due: impl Fn(&LastCall) -> bool) {
    let held = session_lock(session);
    let _held = held.lock().unwrap_or_else(|e| e.into_inner());
    let record = dir.join(LAST_CALL);
    let Some(last) = std::fs::read(&record)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LastCall>(&bytes).ok())
    else {
        return;
    };
    if !due(&last) {
        return;
    }
    let cli = Cli {
        deadline: Instant::now() + timeout(),
        path: user_path(),
        session,
        dir: dir.to_path_buf(),
    };
    if cli.attached().unwrap_or(false) {
        let up = crate::config::config_home()
            .is_ok_and(|home| chromium_up(&home.join("chromium/SingletonLock")));
        if up && !cli.run_code(CLOSE_OWN).is_ok_and(|done| closed(&done)) {
            eprintln!(
                "hub: device: could not close the tabs of {session}; trying again next round"
            );
            return;
        }
        let flag = format!("-s={session}");
        let _ = cli.run(&[&flag, "detach"], None);
    }
    let _ = std::fs::remove_file(&record);
    eprintln!(
        "hub: device: ended browser session {session} of {}",
        last.project
    );
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
        let detach = args[0] == "detach";
        if self.attached()? {
            if detach {
                let done = self.run_code(CLOSE_OWN)?;
                if !closed(&done) {
                    return Ok(Outcome::fail(
                        done.code.unwrap_or(128),
                        format!(
                            "device: closing the session's tabs failed, so it stays attached: {}",
                            String::from_utf8_lossy(&done.stderr)
                        ),
                    ));
                }
            }
        } else if !detach {
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
            // With the token the session works in the extension's connect
            // tab. Without it Saiful picked one of his own on the connect
            // page, and that one stays his.
            if token.is_some() {
                self.run_code(TAG_CURRENT)?;
            }
        }
        if let Some(refused) = self.not_its_tab(args)? {
            return Ok(refused);
        }
        let mut argv = vec![format!("-s={}", self.session)];
        argv.extend(args.iter().cloned());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let done = self.run(&argv, None)?;
        // tab-new opens and selects the tab before it loads the page, so a
        // page that fails to load still leaves the session a new tab.
        if args[0] == "tab-new" {
            self.run_code(TAG_CURRENT)?;
        }
        if args[0] == "tab-close" && done.code == Some(0) {
            self.keep_own_current()?;
        }
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

    /// After a tab-close playwright-core makes the tab at the closed one's
    /// place current, which may be one Saiful dragged into the group. This
    /// selects the session's own instead, or opens it a blank one.
    fn keep_own_current(&self) -> Result<(), Stop> {
        let done = self.run_code(FIRST_OWN)?;
        let flag = format!("-s={}", self.session);
        match result_of(&done).as_deref() {
            Some("-2") => {
                self.run(&[&flag, "tab-new"], None)?;
                self.run_code(TAG_CURRENT)?;
            }
            Some(i) if i.parse::<usize>().is_ok() => {
                self.run(&[&flag, "tab-select", i], None)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The refusal of a tab-select or tab-close whose tab the session did not
    /// open, such as one Saiful dragged into its group. playwright-cli numbers
    /// its tabs in the order of the context's pages.
    fn not_its_tab(&self, args: &[String]) -> Result<Option<Outcome>, Stop> {
        if !matches!(args[0].as_str(), "tab-select" | "tab-close") {
            return Ok(None);
        }
        let (code, which) = match args.get(1) {
            None => (
                concat!("async page => { ", own!(), "return await own(page); }").to_string(),
                "the current tab".to_string(),
            ),
            Some(index) => match index.parse::<usize>() {
                // Only a number the hub parsed goes into the code.
                Ok(i) => (
                    format!(
                        "async page => {{ {}const p = page.context().pages()[{i}]; \
                         return !!p && await own(p); }}",
                        own!()
                    ),
                    format!("tab {i}"),
                ),
                Err(_) => {
                    return Ok(Some(Outcome::refuse(format!(
                        "{} needs a tab number, not {index}",
                        args[0]
                    ))));
                }
            },
        };
        let done = self.run_code(&code)?;
        if result_of(&done).as_deref() == Some("true") {
            return Ok(None);
        }
        Ok(Some(Outcome::refuse(format!(
            "{which} is not one this session opened"
        ))))
    }

    /// The hub's own code, never a caller's, run in the session's page.
    fn run_code(&self, code: &str) -> Result<crate::proc::Finished, Stop> {
        let flag = format!("-s={}", self.session);
        self.run(&[&flag, "run-code", code], None)
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
        // The dotfiles' wrapper, found on the user manager's PATH since the
        // hub's own, pinned by its unit, does not have the dotfiles' bin
        // (~/Codes/dotfiles/bin in v2); v1's ~/.dotfiles/bin when it is not.
        let wrapper = find_in(&self.path, "playwright-cli").unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            Path::new(&home).join(".dotfiles/bin/playwright-cli")
        });
        let mut command = Command::new(wrapper);
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

/// The value a run-code returned: the line after `### Result`.
fn result_of(done: &crate::proc::Finished) -> Option<String> {
    let stdout = String::from_utf8_lossy(&done.stdout);
    let mut lines = stdout.lines().skip_while(|line| *line != "### Result");
    lines.nth(1).map(str::to_string)
}

/// Whether CLOSE_OWN closed the session's tabs: it answered, or closing the
/// last one ended the session.
fn closed(done: &crate::proc::Finished) -> bool {
    done.code == Some(0) || String::from_utf8_lossy(&done.stderr).contains("Session closed")
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
    // dotfiles v1's app-run, else its own user unit, as v2's playwright-cli
    // wrapper starts it over ssh.
    let mut command = if let Some(app_run) = find_in(path, "app-run") {
        let mut command = Command::new(app_run);
        command.arg("chromium-browser");
        command
    } else if let Some(systemd_run) = find_in(path, "systemd-run") {
        let mut command = Command::new(systemd_run);
        command.args([
            "--user",
            "--quiet",
            "--collect",
            "--property=ExitType=cgroup",
            "--",
            "chromium-browser",
        ]);
        command
    } else {
        return;
    };
    let spawned = command
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    // app-run daemonizes and systemd-run returns once the unit runs, so this
    // only reaps it.
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

pub(crate) fn find_in(path: &str, program: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// One line in the project's mem log for the call, its tokens cut.
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
    let line = crate::mask::mask(&line);
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

/// The device the call's project names in mem, when one of `DEVICES` set
/// it and it is one of them. None when it has none, a cloud thread set it,
/// or mem cannot be read.
pub fn project_device(request: &Request) -> Option<String> {
    let call: Call = serde_json::from_slice(&request.body).ok()?;
    let rows = MemCli::new().read(&["projects", "--json"]).rows("projects");
    let row = rows
        .iter()
        .find(|row| row["name"].as_str() == Some(call.project.as_str()))?;
    let by = row["device_by"].as_str()?;
    let device = row["device"].as_str()?;
    (DEVICES.contains(&by) && DEVICES.contains(&device)).then(|| device.to_string())
}

/// The device Saiful named for this run, in answer to the call's question.
/// It counts only when the question is the call's project's, its answer is
/// one of `DEVICES` and was given on one of them (never a thread's own
/// `mem answer`), and no handoff came after the answer: a park or a landing
/// ends the run. None when anything cannot be read.
pub fn run_device(request: &Request) -> Option<String> {
    let call: Call = serde_json::from_slice(&request.body).ok()?;
    let short_id = call.question?;
    let mem = MemCli::new();
    let rows = mem
        .read(&["--project", &call.project, "questions", "--json"])
        .rows("questions");
    let question = rows
        .iter()
        .find(|q| q["short_id"].as_str() == Some(short_id.as_str()))?;
    if question["project"].as_str() != Some(call.project.as_str())
        || question["answered"].as_bool() != Some(true)
    {
        return None;
    }
    let device = question["answer"].as_str()?.trim();
    if !DEVICES.contains(&device) {
        return None;
    }
    let given = mem
        .read(&["show", question["answer_id"].as_str()?, "--json"])
        .rows("items");
    if !DEVICES.contains(&given.first()?["machine"].as_str()?) {
        return None;
    }
    // The answer's own time counts, not the ask's: a question with no answer
    // in 30 minutes parks the run, and his later answer carries it on.
    let answered = hubpages::model::ulid_millis(question["answer_id"].as_str()?)?;
    let projects = mem.read(&["projects", "--json"]).rows("projects");
    let row = projects
        .iter()
        .find(|row| row["name"].as_str() == Some(call.project.as_str()))?;
    let handed_off = match &row["handoff_at"] {
        serde_json::Value::Null => None,
        at => Some(
            at.as_str()?
                .parse::<jiff::Timestamp>()
                .ok()?
                .as_millisecond(),
        ),
    };
    handed_off
        .is_none_or(|at| answered > at)
        .then(|| device.to_string())
}

/// The answer when `device` cannot take the call: exit 3 naming it and the
/// devices `online` now, logged under the machine `none`. The client reads
/// `device` and `online` to ask which one to use.
pub fn unserved(request: &Request, device: &str, online: &[String]) -> Response {
    let call: Call = match serde_json::from_slice(&request.body) {
        Ok(call) => call,
        Err(e) => return Response::text(400, &format!("expected verb, args and project: {e}")),
    };
    let named = if online.is_empty() {
        "none".to_string()
    } else {
        online.join(", ")
    };
    let outcome = Outcome {
        exit: 3,
        ..Outcome::refuse(format!(
            "no device online (device: {device}); online: {named}"
        ))
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
            "device": device,
            "online": online,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_idle_limit_is_15_minutes_unless_set() {
        assert_eq!(idle_limit(None), Duration::from_secs(900));
        assert_eq!(idle_limit(Some("not a number")), Duration::from_secs(900));
        assert_eq!(idle_limit(Some("1000")), Duration::from_secs(1));
    }
}
