//! `device tmux send|read|end`: one tmux session on this device for a
//! thread, named `cloud-<project>-<milestone>` so Saiful can attach to it and
//! watch, and working in its own folder under `~/.cache/workflow-cloud/`.
//! A device runs one such job at a time; it ends on `end`, after the idle
//! limit, at its 20-minute cap, and when its cloud run lands or parks.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::device::{HELPER_TIMEOUT, Outcome, find_in, unix_now, user_command, user_env};
use crate::memcli::MemCli;
use crate::proc::{Ended, Finished, output_within};

/// The keys a send passes as themselves, with no Enter after them, besides
/// C-<key> and M-<key>.
const KEYS: [&str; 10] = [
    "Enter", "Escape", "Tab", "BSpace", "Up", "Down", "Left", "Right", "PageUp", "PageDown",
];

/// How far above the visible pane a read reaches.
const HISTORY: &str = "-200";

/// How long a job may last, in use or not.
const DEFAULT_CAP: Duration = Duration::from_secs(20 * 60);

/// The device's one job, under the state folder: whose it is, and when it
/// started and was last called. Not under hub/device/, whose folders are
/// browser sessions.
const JOB: &str = "hub/device-tmux.json";

/// The session whose job the cap ended, so its next call can say so.
const CAPPED: &str = "hub/device-tmux-capped";

/// The ids of the questions whose yes already sent its line, one a line.
const SPENT: &str = "hub/device-tmux-yes";

/// Held across a job's read, change and write.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
struct Job {
    project: String,
    session: String,
    name: String,
    /// Unix milliseconds.
    started: u64,
    last: u64,
}

/// Why a job ends without its thread's `end`.
#[derive(PartialEq)]
enum Over {
    Cap,
    Idle,
    Run,
}

pub(crate) fn call(session: Option<&str>, project: &str, args: &[String]) -> Outcome {
    let Some((verb, rest)) = args.split_first() else {
        return Outcome::refuse("tmux needs send, read or end".to_string());
    };
    if !matches!(verb.as_str(), "send" | "read" | "end") {
        return Outcome::refuse(format!("tmux {verb} is not a verb (send, read, end)"));
    }
    let Some(session) = session.filter(|s| plain(s)) else {
        return Outcome::refuse("tmux needs a session".to_string());
    };
    let tmux = match Tmux::here() {
        Ok(tmux) => tmux,
        Err(failed) => return failed,
    };
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = unix_now();
    let mut job = read_job();
    if let Some(over) = job.as_ref().and_then(|job| due(job, now)) {
        end_job(&tmux, job.take().as_ref(), over);
    }
    if take_capped(session) {
        return Outcome::refuse("the job ended at the 20-minute cap".to_string());
    }
    let (name, started) = match &job {
        Some(job) if job.session != session => {
            return Outcome {
                stdout: format!("busy: {}\n", job.project),
                ..Outcome::fail(5, String::new())
            };
        }
        Some(job) => (job.name.clone(), job.started),
        None => match job_name(project) {
            Ok(name) => (name, now),
            Err(refused) => return refused,
        },
    };
    let outcome = match verb.as_str() {
        "send" => send(&tmux, &name, project, rest),
        "read" => read(&tmux, &name),
        _ => end(&tmux, &name),
    };
    if verb == "end" {
        end_job(&tmux, None, Over::Run);
    } else if job.is_some() || (verb == "send" && outcome.exit == 0) {
        write_job(&Job {
            project: project.to_string(),
            session: session.to_string(),
            name,
            started,
            last: now,
        });
    }
    outcome
}

/// Ends the job when it is past its cap or idle limit, or its cloud run has
/// landed or parked. Called by the device's sweep.
pub(crate) fn sweep(mem: &MemCli, projects: &OnceCell<Vec<serde_json::Value>>) {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(job) = read_job() else {
        return;
    };
    let over = due(&job, unix_now()).or_else(|| {
        crate::device::run_ended(&job.session, &job.project, mem, projects).then_some(Over::Run)
    });
    if let Some(over) = over
        && let Ok(tmux) = Tmux::here()
    {
        end_job(&tmux, Some(&job), over);
        eprintln!(
            "hub: device: ended tmux job {} of {}",
            job.name, job.project
        );
    }
}

fn due(job: &Job, now: u64) -> Option<Over> {
    let cap = std::env::var("HUB_DEVICE_JOB_CAP_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(DEFAULT_CAP, Duration::from_millis);
    let idle = crate::device::idle_limit(std::env::var("HUB_DEVICE_IDLE_MS").ok().as_deref());
    let since = |at: u64| Duration::from_millis(now.saturating_sub(at));
    if since(job.started) >= cap {
        Some(Over::Cap)
    } else if since(job.last) >= idle {
        Some(Over::Idle)
    } else {
        None
    }
}

/// Kills `job`'s session and forgets the job, and keeps the session that the
/// cap ended. With no `job`, only the record goes.
fn end_job(tmux: &Tmux, job: Option<&Job>, over: Over) {
    if let Some(job) = job {
        let _ = tmux.run(&["kill-session", "-t", &format!("={}", job.name)]);
        if over == Over::Cap
            && let Some(capped) = state(CAPPED)
        {
            let _ = std::fs::write(capped, &job.session);
        }
    }
    if let Some(file) = state(JOB) {
        let _ = std::fs::remove_file(file);
    }
}

/// Whether the cap ended `session`'s job since its last call; forgets it.
fn take_capped(session: &str) -> bool {
    let Some(file) = state(CAPPED) else {
        return false;
    };
    let capped = std::fs::read_to_string(&file).is_ok_and(|s| s == session);
    if capped {
        let _ = std::fs::remove_file(file);
    }
    capped
}

fn read_job() -> Option<Job> {
    let bytes = std::fs::read(state(JOB)?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_job(job: &Job) {
    let Some(file) = state(JOB) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&file, serde_json::to_vec(job).unwrap_or_default()) {
        eprintln!("hub: device: cannot write {}: {e}", file.display());
    }
}

fn state(rel: &str) -> Option<PathBuf> {
    crate::config::state_home().ok().map(|home| home.join(rel))
}

/// 1 to 64 of [A-Za-z0-9_-], fit for a session name and a path.
fn plain(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `cloud-<project>-<milestone>`, from the project's open milestone in mem.
pub(crate) fn job_name(project: &str) -> Result<String, Outcome> {
    let rows = MemCli::new().read(&["projects", "--json"]).rows("projects");
    let milestone = rows
        .iter()
        .find(|row| row["name"].as_str() == Some(project))
        .and_then(|row| row["milestone"].as_str());
    let Some(milestone) = milestone else {
        return Err(Outcome::refuse(format!("{project} has no open milestone")));
    };
    if !plain(project) || !plain(milestone) {
        return Err(Outcome::refuse(format!(
            "{project} {milestone} cannot name a tmux session"
        )));
    }
    Ok(format!("cloud-{project}-{milestone}"))
}

fn send(tmux: &Tmux, name: &str, project: &str, rest: &[String]) -> Outcome {
    let (yes, rest) = match rest {
        [flag, id, rest @ ..] if flag == "--yes" => (Some(id.trim_start_matches('#')), rest),
        _ => (None, rest),
    };
    if rest.is_empty() {
        return Outcome::refuse("tmux send needs a line".to_string());
    }
    let line = rest.join(" ");
    if line.ends_with('\\') {
        return Outcome::refuse(
            "a line may not end with \\: send the whole command as one line".to_string(),
        );
    }
    let mut spend_id = None;
    if crate::held::held(&line) {
        let Some(id) = yes else {
            return Outcome {
                stdout: format!("ask: {line}\n"),
                ..Outcome::fail(6, String::new())
            };
        };
        if let Err(refused) = released(project, id, &line) {
            return refused;
        }
        spend_id = Some(id);
    }
    if !tmux.has(name) {
        let folder = match folder(name) {
            Ok(folder) => folder.to_string_lossy().into_owned(),
            Err(failed) => return failed,
        };
        let path = format!("PATH={}", tmux.path);
        let mut new = vec![
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            &folder,
            "-x",
            "200",
            "-y",
            "50",
            "-e",
            &path,
            // No history, so no key brings a released line back.
            "-e",
            "HISTFILE=/dev/null",
            "-e",
            "HISTSIZE=0",
        ];
        // A thread writes bash, and the user's own shell may be fish. Several
        // words, so tmux runs bash itself rather than through that shell;
        // no rc file, which may turn history back on, and no `!` expansion.
        let bash = find_in(&tmux.path, "bash").map(|b| b.to_string_lossy().into_owned());
        if let Some(bash) = &bash {
            new.extend([bash.as_str(), "--noprofile", "--norc", "+H", "-i"]);
        }
        let made = tmux.run(&new);
        if let Err(failed) = ok(made, "new-session") {
            return failed;
        }
    }
    let target = format!("={name}:");
    let sent = if is_key(&line) {
        tmux.run(&["send-keys", "-t", &target, &line])
    } else {
        match ok(
            tmux.run(&["send-keys", "-t", &target, "-l", "--", &line]),
            "send-keys",
        ) {
            Ok(()) => tmux.run(&["send-keys", "-t", &target, "Enter"]),
            Err(failed) => return failed,
        }
    };
    match ok(sent, "send-keys") {
        Ok(()) => {
            if let Some(id) = spend_id {
                spend(id);
            }
            Outcome::done(String::new())
        }
        Err(failed) => failed,
    }
}

/// Ok when question `id` of `project` asks for `line` and Saiful answered
/// yes on one of the devices, never a cloud thread's own `mem answer`, and
/// no earlier line went out on it.
fn released(project: &str, id: &str, line: &str) -> Result<(), Outcome> {
    let spent = state(SPENT)
        .ok_or_else(|| Outcome::fail(126, "device: cannot find the state folder\n".to_string()))?;
    if std::fs::read_to_string(&spent).is_ok_and(|ids| ids.lines().any(|used| used == id)) {
        return Err(Outcome::refuse(format!(
            "#{id} already sent its line; ask again"
        )));
    }
    let mem = MemCli::new();
    let rows = mem
        .refresh(&["--project", project, "questions", "--json"])
        .rows("questions");
    let Some(question) = rows
        .iter()
        .find(|q| q["short_id"].as_str() == Some(id) && q["project"].as_str() == Some(project))
    else {
        return Err(Outcome::refuse(format!(
            "#{id} is not a question of {project}"
        )));
    };
    let body = question["body"].as_str().unwrap_or_default();
    let asks = body.lines().any(|l| l.trim() == line) || body.contains(&format!("`{line}`"));
    if !asks {
        return Err(Outcome::refuse(format!("#{id} does not ask for {line}")));
    }
    let answer = question["answer"].as_str().unwrap_or_default();
    if question["answered"].as_bool() != Some(true) {
        return Err(Outcome::refuse(format!("#{id} has no answer yet")));
    }
    let first = answer
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .unwrap_or_default();
    if !first.eq_ignore_ascii_case("yes") {
        return Err(Outcome::refuse(format!(
            "the answer to #{id} is {answer}, not yes"
        )));
    }
    let machine = question["answer_id"]
        .as_str()
        .map(|answer| mem.refresh(&["show", answer, "--json"]).rows("items"))
        .and_then(|items| items.first()?["machine"].as_str().map(str::to_string));
    if !machine.is_some_and(|m| crate::device::DEVICES.contains(&m.as_str())) {
        return Err(Outcome::refuse(format!(
            "the yes for #{id} came from a cloud machine"
        )));
    }
    Ok(())
}

/// Keeps `id` as spent once its line went out, so its yes sends no other.
fn spend(id: &str) {
    let Some(spent) = state(SPENT) else {
        return;
    };
    let mut ids = std::fs::read_to_string(&spent).unwrap_or_default();
    ids.push_str(id);
    ids.push('\n');
    if let Err(e) = std::fs::write(&spent, ids) {
        eprintln!("hub: device: cannot keep #{id} as spent: {e}");
    }
}

/// The job's folder, `~/.cache/workflow-cloud/<name>`, made when it is not
/// there yet.
pub(crate) fn folder(name: &str) -> Result<PathBuf, Outcome> {
    let Some(home) = std::env::var_os("HOME") else {
        return Err(Outcome::fail(126, "device: HOME is not set\n".to_string()));
    };
    let folder = PathBuf::from(home).join(".cache/workflow-cloud").join(name);
    std::fs::create_dir_all(&folder).map_err(|e| {
        Outcome::fail(
            126,
            format!("device: cannot make {}: {e}\n", folder.display()),
        )
    })?;
    Ok(folder)
}

fn read(tmux: &Tmux, name: &str) -> Outcome {
    if !tmux.has(name) {
        return Outcome::refuse(format!("no tmux session {name}; send a line first"));
    }
    let target = format!("={name}:");
    match tmux.run(&["capture-pane", "-p", "-J", "-S", HISTORY, "-t", &target]) {
        Ok(done) if done.code == Some(0) => {
            Outcome::done(String::from_utf8_lossy(&done.stdout).into_owned())
        }
        other => ok(other, "capture-pane")
            .err()
            .unwrap_or_else(|| Outcome::fail(1, "device: tmux capture-pane failed\n".to_string())),
    }
}

fn end(tmux: &Tmux, name: &str) -> Outcome {
    if !tmux.has(name) {
        return Outcome::done(String::new());
    }
    match ok(
        tmux.run(&["kill-session", "-t", &format!("={name}")]),
        "kill-session",
    ) {
        Ok(()) => Outcome::done(String::new()),
        Err(failed) => failed,
    }
}

/// A line that is one tmux key name: C-<key>, M-<key>, or one of KEYS.
fn is_key(line: &str) -> bool {
    KEYS.contains(&line)
        || ["C-", "M-"]
            .iter()
            .any(|p| line.strip_prefix(p).is_some_and(|k| k.chars().count() == 1))
}

/// Ok when tmux exited 0, else the answer that says how it failed.
fn ok(done: Result<Finished, Outcome>, what: &str) -> Result<(), Outcome> {
    let done = done?;
    if done.code == Some(0) {
        return Ok(());
    }
    Err(Outcome::fail(
        done.code.unwrap_or(128),
        format!(
            "device: tmux {what} failed: {}",
            String::from_utf8_lossy(&done.stderr)
        ),
    ))
}

/// tmux on the user manager's PATH, on `HUB_DEVICE_TMUX_SOCKET` when it is
/// set, else on the user's default server, the one `tmux attach` reaches.
pub(crate) struct Tmux {
    bin: PathBuf,
    path: String,
    env: Vec<(String, String)>,
}

impl Tmux {
    pub(crate) fn here() -> Result<Tmux, Outcome> {
        let env = user_env();
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let Some(bin) = find_in(&path, "tmux") else {
            return Err(Outcome::fail(
                127,
                "device: tmux was not found\n".to_string(),
            ));
        };
        Ok(Tmux { bin, path, env })
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        matches!(
            self.run(&["has-session", "-t", &format!("={name}")]),
            Ok(done) if done.code == Some(0)
        )
    }

    pub(crate) fn run(&self, args: &[&str]) -> Result<Finished, Outcome> {
        // The command that may start the server runs in a scope of its own.
        let mut command = user_command(&self.bin, &self.env, args[0] == "new-session");
        if let Some(socket) = std::env::var_os("HUB_DEVICE_TMUX_SOCKET").filter(|s| !s.is_empty()) {
            command.arg("-S").arg(socket);
        }
        command.args(args);
        match output_within(&mut command, HELPER_TIMEOUT) {
            Ended::Exited(done) => Ok(done),
            Ended::TimedOut => Err(Outcome::fail(
                124,
                "device: tmux took too long\n".to_string(),
            )),
            Ended::Failed(e) => Err(Outcome::fail(
                126,
                format!("device: tmux did not start: {e}\n"),
            )),
        }
    }
}
