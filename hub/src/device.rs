//! `POST /device`: what this machine does for a session that is somewhere
//! else. `whoami` names the machine, `browser` drives the logged-in Chromium
//! through playwright-cli, `tmux` drives the session's own tmux session
//! (device_tmux), `amx` drives the one amx agent the thread started
//! (device_amx), `with-secrets` reads a project's pass entries for one cloud
//! command (device_secrets), and every call leaves one line in the project's
//! mem log. `run` is gone: its programs go through tmux.

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

/// The machines that serve device calls. A project's own device counts only
/// when one of them set it, so a cloud thread's mem cannot move its calls.
pub const DEVICES: [&str; 3] = ["mac-mini-m2", "nuc", "macbook-m2"];

/// The machine the hub page signs every answer Saiful taps there with. It
/// answers only for a person signed in to Access on a paired browser, so
/// it is his word as a device's is; it asks nothing and serves no call.
pub const HUB_PAGE: &str = "hub-web";

/// Whether what `machine` wrote is Saiful's word: a device's or the hub
/// page's, never a cloud machine's.
pub fn his_word(machine: &str) -> bool {
    DEVICES.contains(&machine) || machine == HUB_PAGE
}

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

/// Marks every page the session's daemon knows as seen, before it opens a
/// tab, so TAG_NEW can tell the new tab from one it fell back to.
const SEEN_ALL: &str =
    "async page => { for (const p of page.context().pages()) p.__hubSeen = true; }";

/// After a tab-new, marks the current page as one the session opened, when
/// it is the new one: a tab-new whose page closes (tab-new about:blank
/// does, through the extension) leaves another tab current, which may be
/// another session's. The session's playwright-cli daemon keeps its Page
/// objects from call to call, so the mark lasts as long as the session and
/// no other session's daemon sees it. A tab Saiful drags into the group
/// carries no mark.
/// It returns the page's guid when it marks it, which is the same in every
/// daemon on the connection, so the hub can close the tab through another
/// one, and null when it does not.
const TAG_NEW: &str = "async page => { if (!page.__hubSeen) { page.__hubTab = true; \
     return page._guid || ''; } return null; }";

/// The file in a session's folder that lists the guid of each tab it
/// opened: what the hub closes it by when the session's daemon is gone or
/// no longer knows its marks.
const OWN_IDS: &str = "tabs.json";

/// The file in the connection's folder that holds the guid of the hub's own
/// tab, the one that keeps the connection up when no session has a tab.
const HUB_TAB: &str = "hub-tab";

/// Returns null while the hub's tab, guid `id`, is open. With no guid kept
/// it takes the connection's first page when that is blank, as it is when
/// the connection is new; otherwise it opens a blank page, which stays open
/// where tab-new about:blank does not, and returns that page's guid.
fn hub_tab_code(id: Option<&str>) -> String {
    format!(
        "async page => {{ const id = {}; const pages = page.context().pages(); \
         if (pages.some(p => p._guid === id)) return null; \
         if (id === null && pages.length && pages[0].url() === 'about:blank') \
         return pages[0]._guid || ''; \
         const p = await page.context().newPage(); return p._guid || ''; }}",
        serde_json::to_string(&id).unwrap_or_else(|_| "null".to_string())
    )
}

/// The start of the hub's code that asks whether a page is the session's
/// own: one it marked, or one such a page opened, as a link with
/// target=_blank does.
macro_rules! own {
    () => {
        "const own = async p => { if (p.__hubTab) return true; \
         const o = await p.opener().catch(() => null); return !!o && own(o); }; "
    };
}

/// Whether the current page is the session's own.
const OWN_CURRENT: &str = concat!("async page => { ", own!(), "return await own(page); }");

/// Closes every page the session opened, by its mark or by one of the
/// guids in `ids`, each judged before any closes, so a popup still has its
/// opener, and returns the URL of each of its pages still open after.
/// Closing the last one ends the extension's connection and the session
/// with it, which playwright-cli reports as "Session closed" on stderr with
/// exit 1.
fn close_own_code(ids: &[String]) -> String {
    format!(
        "async page => {{ const ids = {}; \
         const own = async p => {{ if (p.__hubTab || ids.includes(p._guid)) return true; \
         const o = await p.opener().catch(() => null); return !!o && own(o); }}; \
         const mine = []; for (const p of page.context().pages()) if (await own(p)) mine.push(p); \
         for (const p of mine) await p.close(); const left = []; \
         for (const p of page.context().pages()) if (await own(p)) left.push(p.url()); \
         return left; }}",
        serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_string())
    )
}

/// The guids of the tabs the session in `dir` opened. Only a guid the hub
/// checked goes into its code.
fn own_ids(dir: &Path) -> Vec<String> {
    std::fs::read(dir.join(OWN_IDS))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|id| is_guid(id))
        .collect()
}

fn is_guid(id: &str) -> bool {
    id.strip_prefix("page@")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// -1 when the current page is the session's own, else the number of the
/// first one that is, else -2.
const FIRST_OWN: &str = concat!(
    "async page => { ",
    own!(),
    "if (await own(page)) return -1; const pages = page.context().pages(); \
     for (let i = 0; i < pages.length; i++) if (await own(pages[i])) return i; return -2; }"
);

/// The session's own tabs, each with the number playwright-cli gives it
/// among all the connection's tabs.
const OWN_TABS: &str = concat!(
    "async page => { ",
    own!(),
    "const pages = page.context().pages(); const mine = []; \
     for (let i = 0; i < pages.length; i++) if (await own(pages[i])) mine.push({ i, \
     current: pages[i] === page, title: await pages[i].title(), url: pages[i].url() }); \
     return mine; }"
);

/// The URL of the current page and of each frame in it, as Chromium
/// normalised them: a page of the same site may frame the hub page, and
/// Chromium sends its cookies there.
const FRAME_URLS: &str = "async page => page.frames().map(f => f.url())";

/// The hosts that serve the hub page (hubweb/cloudflare.config.ts), where a
/// tap on Yes is signed as Saiful's own.
const HUB_PAGE_HOSTS: [&str; 4] = [
    "hub.on-rexa.com",
    "mem.on-rexa.com",
    "next-hub.on-rexa.com",
    "next-mem.on-rexa.com",
];

/// The hub's own playwright-cli session, the one that attaches through the
/// extension. Every device browser session joins it by this name, so the
/// extension opens its connect tab once per Chromium, not once per session.
/// The dot keeps it out of the names a caller may give a session.
const CONNECTION: &str = "hub.connection";

/// How long a session may go without a call before the hub closes its tabs.
const DEFAULT_IDLE: Duration = Duration::from_secs(15 * 60);

/// The file in a session's folder that says when its last call ended, and
/// for which project.
const LAST_CALL: &str = "last-call.json";

/// What a session hears on its first call after its connection ended.
const ENDED: &str = "device: the browser connection ended (Chromium closed or restarted, or its \
     connect tab was closed), and this session's tabs with it; the next call attaches anew\n";

/// The file in a session's folder that says its last attach made a
/// connection of its own, with no token, rather than join the hub's. Every
/// attach writes or removes it.
const ALONE: &str = "alone";

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
    if call.verb == "with-secrets" {
        // The values go back beside the answer, never in it or the log.
        let (outcome, secrets) = crate::device_secrets::call(machine, &call.project);
        log(machine, &call, &outcome);
        return Response::new(
            200,
            "application/json",
            json!({
                "machine": machine,
                "stdout": outcome.stdout,
                "stderr": outcome.stderr,
                "exit": outcome.exit,
                "files": {},
                "secrets": secrets,
            })
            .to_string(),
        );
    }
    let mut outcome = match call.verb.as_str() {
        "whoami" => Outcome {
            stdout: format!("{machine}\n"),
            stderr: String::new(),
            exit: 0,
            refused: None,
            files: BTreeMap::new(),
        },
        // adb, podman, gh, cf and deploys go through tmux, where Saiful can
        // watch them and a destructive line waits for his yes.
        "run" => Outcome::refuse("run is gone; use device tmux".to_string()),
        "browser" => browser(call.session.as_deref(), &call.project, &call.args),
        "tmux" => {
            crate::device_tmux::call(machine, call.session.as_deref(), &call.project, &call.args)
        }
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
    // tab-new opens its URL as goto does.
    if matches!(args[0].as_str(), "goto" | "tab-new") && args[1..].iter().any(|a| on_hub_page(a)) {
        return Outcome::refuse(
            "browser may not open the hub page: a yes there is Saiful's own".to_string(),
        );
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
        on: Default::default(),
        stayed: Default::default(),
    };
    let held = session_lock(session);
    let _held = held.lock().unwrap_or_else(|e| e.into_inner());
    let called = cli.call(args);
    let record = cli.dir.join(LAST_CALL);
    // A detach that could not close the tabs keeps the session's record,
    // so the idle sweep tries again.
    if !cli.stayed.get() && cli.on.get() == Some(false)
        || args[0] == "detach" && matches!(&called, Ok(outcome) if outcome.exit == 0)
    {
        let _ = std::fs::remove_file(&record);
    } else {
        let last = LastCall {
            project: project.to_string(),
            at: unix_now(),
            stayed: cli.stayed.get(),
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

/// Where the hub's connection session runs, outside hub/device, which holds
/// only the sessions the sweep may end.
fn connection_dir() -> anyhow::Result<PathBuf> {
    let dir = crate::config::state_home()?.join("hub/connection");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// When a session's last call ended, and for which project.
#[derive(Serialize, Deserialize)]
struct LastCall {
    project: String,
    /// Unix milliseconds.
    at: u64,
    /// Whether a tab of the session stayed open through its detach, so the
    /// sweep closes it on its next round.
    #[serde(default)]
    stayed: bool,
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
                last.stayed
                    || Duration::from_millis(unix_now().saturating_sub(last.at)) >= limit
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
    cloud_run_of(project, mem, projects).is_some_and(|run| {
        run.id == runner_of(session) && run.state != crate::live::CloudState::Live
    })
}

/// run_over, or the project's cloud run is now another session's: a job or
/// agent of a run that a later one replaced before any sweep saw it end
/// would otherwise hold the device for good.
pub(crate) fn run_ended(
    session: &str,
    project: &str,
    mem: &MemCli,
    projects: &std::cell::OnceCell<Vec<serde_json::Value>>,
) -> bool {
    cloud_run_of(project, mem, projects).is_some_and(|run| {
        run.id != runner_of(session) || run.state != crate::live::CloudState::Live
    })
}

/// A thread sends CLAUDE_CODE_REMOTE_SESSION_ID, and the runner record names
/// the same session with `session_` for `cse_`, as fleet reads it.
fn runner_of(session: &str) -> String {
    match session.strip_prefix("cse_") {
        Some(rest) => format!("session_{rest}"),
        None => session.to_string(),
    }
}

/// `project`'s cloud run as the home page reads it, if it has one.
fn cloud_run_of(
    project: &str,
    mem: &MemCli,
    projects: &std::cell::OnceCell<Vec<serde_json::Value>>,
) -> Option<crate::live::CloudRun> {
    let rows = projects.get_or_init(|| mem.refresh(&["projects", "--json"]).rows("projects"));
    let row = rows
        .iter()
        .find(|row| row["name"].as_str() == Some(project))?;
    let summary = crate::page_home::summary_of(row);
    crate::live::cloud_run(
        &summary,
        || crate::doorbell::handoff_body(mem, project),
        jiff::Timestamp::now(),
    )
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
        on: Default::default(),
        stayed: Default::default(),
    };
    let attached = cli.attached().unwrap_or(false);
    let up = crate::config::config_home()
        .is_ok_and(|home| chromium_up(&home.join("chromium/SingletonLock")));
    // A session `list` no longer names may still have tabs open on the
    // hub's connection.
    let closing = up.then(|| {
        if attached {
            cli.close_own()
        } else {
            cli.close_own_left()
        }
    });
    if closing.is_some_and(|left| !matches!(left, Ok(Ok(left)) if left.is_empty())) {
        eprintln!("hub: device: could not close the tabs of {session}; trying again next round");
        return;
    }
    if attached {
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
    /// Whether the session was attached as the call went on, None when the
    /// call stopped before the hub could tell: the record of a session that
    /// is not, whose attach failed or whose connection ended, goes, so its
    /// next call attaches.
    on: std::cell::Cell<Option<bool>>,
    /// Whether a tab of the session stayed open through its detach.
    stayed: std::cell::Cell<bool>,
}

impl Cli<'_> {
    /// Closes the session's tabs: the URL of each that stayed open, or the
    /// run of playwright-cli that could not close them.
    fn close_own(&self) -> Result<Result<Vec<String>, crate::proc::Finished>, Stop> {
        if !self.dir.join(ALONE).exists() {
            self.keep_hub_tab()?;
        }
        self.close_own_in(self)
    }

    /// close_own through the hub's connection, for a session whose daemon
    /// is gone: by the guids the session's folder keeps, which no other
    /// session's tab has. Nothing to close when it keeps none, or when the
    /// connection is gone, which took the tabs' session with it.
    fn close_own_left(&self) -> Result<Result<Vec<String>, crate::proc::Finished>, Stop> {
        if own_ids(&self.dir).is_empty()
            || self.dir.join(ALONE).exists()
            || !self.listed(CONNECTION)?
        {
            let _ = std::fs::remove_file(self.dir.join(OWN_IDS));
            return Ok(Ok(Vec::new()));
        }
        self.keep_hub_tab()?;
        self.close_own_in(&self.connection()?)
    }

    /// Opens the hub's own tab on its connection again when Saiful closed
    /// it, so closing the last session's tab leaves the connection up and
    /// the next attach opens no other connect tab.
    fn keep_hub_tab(&self) -> Result<(), Stop> {
        let connection = self.connection()?;
        let file = connection.dir.join(HUB_TAB);
        // Held from the read of the guid to its write, so two calls at once
        // open one tab, not two.
        let _tabs = tabs_lock();
        let id = std::fs::read_to_string(&file)
            .ok()
            .map(|id| id.trim().to_string())
            .filter(|id| is_guid(id));
        let done = connection.run_code(&hub_tab_code(id.as_deref()))?;
        match result_of(&done).map(|r| serde_json::from_str::<Option<String>>(&r)) {
            Some(Ok(None)) => {}
            Some(Ok(Some(id))) if is_guid(&id) => {
                if let Err(e) = std::fs::write(&file, id) {
                    eprintln!("hub: device: cannot write {}: {e}", file.display());
                }
            }
            _ => eprintln!("hub: device: cannot keep the hub's own tab: {}", why(&done)),
        }
        Ok(())
    }

    /// Closes the session's tabs by `cli`'s daemon; forgets their guids
    /// once none is left.
    fn close_own_in(&self, cli: &Cli) -> Result<Result<Vec<String>, crate::proc::Finished>, Stop> {
        let done = {
            let _tabs = tabs_lock();
            cli.run_code(&close_own_code(&own_ids(&self.dir)))?
        };
        if !closed(&done) {
            return Ok(Err(done));
        }
        // Closing the connection's last tab ends the session before it
        // answers, and every tab of it is closed.
        let left: Vec<String> = result_of(&done)
            .and_then(|left| serde_json::from_str(&left).ok())
            .unwrap_or_default();
        if left.is_empty() {
            let _ = std::fs::remove_file(self.dir.join(OWN_IDS));
        }
        Ok(Ok(left))
    }

    /// The hub's own connection session.
    fn connection(&self) -> Result<Cli<'static>, Stop> {
        Ok(Cli {
            deadline: self.deadline,
            path: self.path.clone(),
            session: CONNECTION,
            dir: connection_dir().map_err(|e| Stop::Failed(format!("{e:#}")))?,
            on: Default::default(),
            stayed: Default::default(),
        })
    }

    fn call(&self, args: &[String]) -> Result<Outcome, Stop> {
        let command = args[0].as_str();
        let detach = command == "detach";
        let tab_command = matches!(command, "tab-list" | "tab-new" | "tab-select" | "tab-close");
        let mut fresh = false;
        let attached = self.attached()?;
        self.on.set(Some(attached));
        if detach {
            // A session `list` no longer names may still have tabs open on
            // the hub's connection.
            let closing = if attached {
                self.close_own()?
            } else {
                self.close_own_left()?
            };
            match closing {
                Err(done) => {
                    // The sweep tries again, attached or not.
                    self.stayed.set(!attached);
                    let stays = if attached {
                        "so it stays attached"
                    } else {
                        "so the hub tries again within 15 minutes"
                    };
                    return Ok(Outcome::fail(
                        done.code.unwrap_or(128),
                        format!(
                            "device: closing the session's tabs failed, {stays}: {}",
                            String::from_utf8_lossy(&done.stderr)
                        ),
                    ));
                }
                Ok(left) if !left.is_empty() => {
                    self.stayed.set(true);
                    return Ok(Outcome::fail(1, stayed_open(&left)));
                }
                Ok(_) => {}
            }
        } else if !attached {
            // A session with a record was attached when its last call ended.
            if self.dir.join(LAST_CALL).exists() {
                // The record goes either way, so the next call attaches; the
                // guids of tabs left open stay, and that session's detach or
                // idle close takes them.
                let mut ended = ENDED.to_string();
                if let Ok(left) = self.close_own_left()?
                    && !left.is_empty()
                {
                    ended.push_str(&stayed_open(&left));
                }
                return Ok(Outcome::fail(1, ended));
            }
            if let Some(failed) = self.attach()? {
                return Ok(failed);
            }
            fresh = true;
        }
        // A tab number counts every tab of the shared connection, so no
        // other session closes one between the check of a number and its use.
        let _tabs = matches!(command, "tab-select" | "tab-close").then(tabs_lock);
        if let Some(refused) = self.not_its_tab(args)? {
            return Ok(refused);
        }
        // Saiful closing the session's current tab by hand makes the tab at
        // its place current, which may be another session's. A session alone
        // on its connection works in the tab Saiful picked, which it never
        // marks.
        let shared = !self.dir.join(ALONE).exists();
        if !fresh && !detach && !tab_command && shared {
            let _tabs = tabs_lock();
            if let Err(why) = self.keep_own_current()? {
                return Ok(Outcome::fail(
                    1,
                    format!(
                        "device: cannot find the session's own tab, so the command did not run: {why}"
                    ),
                ));
            }
        }
        // A click on the hub page's Yes would release a held tmux line as
        // Saiful's word. Alone, the session works in the tab Saiful picked,
        // which may be that page, so every path reads the URL here. A goto
        // leaves the page, unless its javascript: URL runs in it.
        let leaves = command == "goto"
            && !args[1..].iter().any(|a| {
                a.get(..11)
                    .is_some_and(|s| s.eq_ignore_ascii_case("javascript:"))
            });
        if !detach && !tab_command && !leaves {
            let done = self.run_code(FRAME_URLS)?;
            let urls = result_of(&done).and_then(|r| serde_json::from_str::<Vec<String>>(&r).ok());
            match urls {
                Some(urls) if urls.iter().any(|url| on_hub_page(url)) => {
                    return Ok(Outcome::refuse(
                        "browser may not act on the hub page: a yes there is Saiful's own"
                            .to_string(),
                    ));
                }
                Some(_) => {}
                None => {
                    return Ok(Outcome::fail(
                        1,
                        format!(
                            "device: cannot read the session's tab URL, so the command did not run: {}",
                            why(&done)
                        ),
                    ));
                }
            }
        }
        let mut argv = vec![format!("-s={}", self.session)];
        argv.extend(args.iter().cloned());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let done = if command == "tab-new" {
            self.open_own_tab(&argv)?
        } else {
            self.run(&argv, None)?
        };
        if command == "tab-close" && done.code == Some(0) {
            let _ = self.keep_own_current()?;
        }
        let mut stdout = String::from_utf8_lossy(&done.stdout).into_owned();
        let mut stderr = String::from_utf8_lossy(&done.stderr).into_owned();
        // A page that closes itself, such as a popup on a click, makes the
        // connection's next tab current, and playwright-cli reports that
        // page, which may be another session's.
        if !detach && !tab_command && shared && !READ_ONLY.contains(&command) {
            let _tabs = tabs_lock();
            let current = self.run_code(OWN_CURRENT)?;
            if result_of(&current).as_deref() != Some("true") {
                stdout = without(
                    &stdout,
                    &["### Page", "### Snapshot", "### Modal state", "### Events"],
                );
                stderr.push_str("device: the session's tab closed, so it works in its own again\n");
                let _ = self.keep_own_current()?;
            }
        }
        // playwright-cli lists every tab of the connection: in a tab
        // command's result, and under "Open tabs" once there are two.
        if stdout.contains("### Open tabs") || (tab_command && stdout.contains("### Result")) {
            stdout = only_own(&stdout, tab_command, &self.own_tabs()?);
        }
        let files = linked_files(&stdout, &self.dir, &mut stderr);
        Ok(Outcome {
            stdout,
            stderr,
            exit: done.code.unwrap_or(128),
            refused: None,
            files,
        })
    }

    /// Joins the hub's connection, making it first when it is not up, and
    /// opens the session a tab of its own there. Without the extension's
    /// token no connection can be made unattended: the session attaches by
    /// itself, and Saiful picks one of his own tabs on the connect page,
    /// which stays his. The failure to answer with, if any.
    fn attach(&self) -> Result<Option<Outcome>, Stop> {
        let flag = format!("-s={}", self.session);
        let attach_failed = |done: &crate::proc::Finished| {
            Outcome::fail(
                done.code.unwrap_or(128),
                format!(
                    "device: attach failed: {}",
                    String::from_utf8_lossy(&done.stderr)
                ),
            )
        };
        {
            // One session at a time looks for the connection, so two at once
            // do not make two.
            static MAKING: Mutex<()> = Mutex::new(());
            let _making = MAKING.lock().unwrap_or_else(|e| e.into_inner());
            if !self.listed(CONNECTION)? {
                ensure_chromium(&self.path, self.deadline);
                let Some(token) = self.token() else {
                    let done = self.run_scoped(&[&flag, "attach", "--extension=chromium"], None)?;
                    if done.code != Some(0) {
                        return Ok(Some(attach_failed(&done)));
                    }
                    self.on.set(Some(true));
                    let _ = std::fs::write(self.dir.join(ALONE), "");
                    return Ok(None);
                };
                let connection = self.connection()?;
                let _ = std::fs::remove_file(connection.dir.join(HUB_TAB));
                let made = connection.run_scoped(
                    &[
                        &format!("-s={CONNECTION}"),
                        "attach",
                        "--extension=chromium",
                    ],
                    Some(&token),
                )?;
                if made.code != Some(0) {
                    return Ok(Some(attach_failed(&made)));
                }
            }
        }
        let _ = std::fs::remove_file(self.dir.join(ALONE));
        let joined = self.run_scoped(&[&flag, "attach", CONNECTION], None)?;
        if joined.code != Some(0) {
            return Ok(Some(attach_failed(&joined)));
        }
        // Joined, the session has a record from here on, so the sweep ends
        // it even when it gets no tab.
        self.on.set(Some(true));
        self.keep_hub_tab()?;
        // A session joins on whichever tab of the connection is current,
        // which is another's, so it works in a new one.
        let opened = self.open_own_tab(&[&flag, "tab-new"])?;
        if opened.code != Some(0) {
            return Ok(Some(attach_failed(&opened)));
        }
        Ok(None)
    }

    /// Runs `argv`, a tab-new, and marks the tab it opened as the session's
    /// own. tab-new opens and selects the tab before it loads the page, so
    /// a page that fails to load still leaves the session a new tab; one
    /// whose page closed leaves none, and no mark.
    fn open_own_tab(&self, argv: &[&str]) -> Result<crate::proc::Finished, Stop> {
        self.run_code(SEEN_ALL)?;
        let opened = self.run(argv, None)?;
        let tagged = self.run_code(TAG_NEW)?;
        match result_of(&tagged).map(|r| serde_json::from_str::<Option<String>>(&r)) {
            Some(Ok(None)) => {}
            Some(Ok(Some(id))) if is_guid(&id) => {
                let mut ids = own_ids(&self.dir);
                ids.push(id);
                let file = self.dir.join(OWN_IDS);
                if let Err(e) = std::fs::write(&file, serde_json::to_vec(&ids).unwrap_or_default())
                {
                    eprintln!("hub: device: cannot write {}: {e}", file.display());
                }
            }
            _ => eprintln!(
                "hub: device: the tab {} opened has no guid, so only its daemon's mark closes it: {}",
                self.session,
                why(&tagged)
            ),
        }
        Ok(opened)
    }

    /// After a tab-close playwright-core makes the tab at the closed one's
    /// place current, which may be another session's or one Saiful dragged
    /// into the group. This selects the session's own instead, or opens it
    /// a blank one. Why it could not, if it could not. The caller holds
    /// tabs_lock, so the number it selects is the one it read.
    fn keep_own_current(&self) -> Result<Result<(), String>, Stop> {
        let done = self.run_code(FIRST_OWN)?;
        let flag = format!("-s={}", self.session);
        let moved = match result_of(&done).as_deref() {
            Some("-1") => return Ok(Ok(())),
            Some("-2") => self.open_own_tab(&[&flag, "tab-new"])?,
            Some(i) if i.parse::<usize>().is_ok() => self.run(&[&flag, "tab-select", i], None)?,
            _ => return Ok(Err(why(&done))),
        };
        if moved.code != Some(0) {
            return Ok(Err(why(&moved)));
        }
        Ok(Ok(()))
    }

    /// The session's own tabs as playwright-cli lists tabs, none when the
    /// hub cannot read them.
    fn own_tabs(&self) -> Result<String, Stop> {
        #[derive(Deserialize)]
        struct OwnTab {
            i: usize,
            current: bool,
            title: String,
            url: String,
        }
        let done = self.run_code(OWN_TABS)?;
        let tabs: Vec<OwnTab> = result_of(&done)
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Ok(tabs
            .iter()
            .map(|t| {
                let current = if t.current { " (current)" } else { "" };
                format!("- {}:{current} [{}]({})\n", t.i, t.title, t.url)
            })
            .collect())
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
        self.listed(self.session)
    }

    /// Whether `playwright-cli list` names `session`.
    fn listed(&self, session: &str) -> Result<bool, Stop> {
        let done = self.run(&["list"], None)?;
        let name = format!("- {session}:");
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
        self.run_as(args, token, false)
    }

    /// `run` with the playwright-cli daemon it starts in a systemd scope of
    /// its own, when the user's PATH has systemd-run. The hub's unit stops
    /// every process of its own cgroup when it restarts, which would end
    /// the connection, and the next attach would open another connect tab.
    fn run_scoped(
        &self,
        args: &[&str],
        token: Option<&str>,
    ) -> Result<crate::proc::Finished, Stop> {
        self.run_as(args, token, true)
    }

    fn run_as(
        &self,
        args: &[&str],
        token: Option<&str>,
        scoped: bool,
    ) -> Result<crate::proc::Finished, Stop> {
        // The dotfiles' wrapper, found on the user manager's PATH since the
        // hub's own, pinned by its unit, does not have the dotfiles' bin
        // (~/Codes/dotfiles/bin in v2); v1's ~/.dotfiles/bin when it is not.
        let wrapper = find_in(&self.path, "playwright-cli").unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_default();
            Path::new(&home).join(".dotfiles/bin/playwright-cli")
        });
        let mut command = match find_in(&self.path, "systemd-run").filter(|_| scoped) {
            Some(systemd_run) => {
                let mut command = Command::new(systemd_run);
                command
                    .args(["--user", "--scope", "--quiet", "--collect", "--"])
                    .arg(wrapper);
                command
            }
            None => Command::new(wrapper),
        };
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

/// What a session hears when tabs it opened stayed open through their close.
fn stayed_open(left: &[String]) -> String {
    let (tabs, them) = if left.len() == 1 {
        ("tab", "it")
    } else {
        ("tabs", "them")
    };
    format!(
        "device: {} {tabs} stayed open: {}; the hub tries to close {them} again within 15 \
         minutes\n",
        left.len(),
        left.join(", "),
    )
}

/// The value a run-code returned: the line after `### Result`.
fn result_of(done: &crate::proc::Finished) -> Option<String> {
    let stdout = String::from_utf8_lossy(&done.stdout);
    let mut lines = stdout.lines().skip_while(|line| *line != "### Result");
    lines.nth(1).map(str::to_string)
}

/// Whether `url` is on a hub page host. With no scheme, playwright-cli's
/// goto puts https:// in front, so the text up to the path is the host.
fn on_hub_page(url: &str) -> bool {
    let rest = match url.split_once("://") {
        Some((scheme, rest))
            if scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) =>
        {
            rest
        }
        _ => url,
    };
    // Chromium drops the slashes a URL with no scheme starts with, and
    // decodes an escaped host.
    let rest = rest.trim_start_matches(['/', '\\']);
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    let host = unescaped(host).to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    HUB_PAGE_HOSTS.contains(&host)
}

/// `text` with each `%XX` escape decoded, as a URL's host is read.
fn unescaped(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(decoded) = u8::from_str_radix(hex, 16)
        {
            out.push(decoded);
            i += 3;
            continue;
        }
        out.push(byte);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What playwright-cli said when a run of it did not do what the hub asked.
fn why(done: &crate::proc::Finished) -> String {
    match String::from_utf8_lossy(&done.stderr) {
        stderr if !stderr.trim().is_empty() => stderr.into_owned(),
        _ => format!("playwright-cli exited {:?} with no answer\n", done.code),
    }
}

/// Commands that leave the current tab as it is.
const READ_ONLY: [&str; 3] = ["snapshot", "find", "console"];

/// `stdout` without the sections named in `names`.
fn without(stdout: &str, names: &[&str]) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in stdout.lines() {
        if line.starts_with("### ") {
            skipping = names.contains(&line);
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The lock over the numbers of the connection's tabs, held from a tab
/// number's check to its use and while a session's tabs close.
fn tabs_lock() -> std::sync::MutexGuard<'static, ()> {
    static TABS: Mutex<()> = Mutex::new(());
    TABS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `stdout` with each list of tabs replaced by `own`: the "Open tabs"
/// section, and the result of a tab command when `in_result`.
fn only_own(stdout: &str, in_result: bool, own: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in stdout.lines() {
        if line.starts_with("### ") {
            skipping = false;
        } else if skipping {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if line == "### Open tabs" || (in_result && line == "### Result") {
            out.push_str(own);
            skipping = true;
        }
    }
    out
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

/// The user manager's environment, what the user's own programs get, and
/// not the hub's, which holds its tokens. HOME and PATH fall back to the
/// hub's own when systemctl does not say.
pub(crate) fn user_env() -> Vec<(String, String)> {
    let mut command = Command::new("systemctl");
    command.args(["--user", "show-environment"]);
    let mut env: Vec<(String, String)> = match output_within(&mut command, HELPER_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => String::from_utf8_lossy(&done.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        _ => Vec::new(),
    };
    for key in ["HOME", "PATH"] {
        if !env.iter().any(|(k, _)| k == key)
            && let Ok(value) = std::env::var(key)
        {
            env.push((key.to_string(), value));
        }
    }
    env
}

/// `bin` with `env` and nothing of the hub's. `scoped` runs it in a systemd
/// scope of its own when the user manager can make one, so a server it
/// starts, such as tmux's, is not in the hub's cgroup and outlives a restart
/// of the hub.
pub(crate) fn user_command(bin: &Path, env: &[(String, String)], scoped: bool) -> Command {
    let var = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    let systemd_run = var("XDG_RUNTIME_DIR")
        .and(var("PATH"))
        .filter(|_| scoped)
        .and_then(|path| find_in(path, "systemd-run"));
    let mut command = match systemd_run {
        Some(systemd_run) => {
            let mut command = Command::new(systemd_run);
            command
                .args(["--user", "--scope", "--quiet", "--collect", "--"])
                .arg(bin);
            command
        }
        None => Command::new(bin),
    };
    command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    command
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
    // A relative folder (node_modules/.bin) is wherever the caller stands.
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
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
