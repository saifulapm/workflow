//! The commands the hub web leaves for this machine, as the store hands
//! them over the sharing socket: Start, Resume and the rest that workflow
//! agent ran. Each runs once, here, in a scope of its own, and is answered
//! `done` so the store forgets it. The store sends a command again with each
//! report until it hears done, so the ids already run are kept on disk and
//! only answered again.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use hubpages::host::Command as Job;
use serde_json::{Value, json};

use crate::fleet;
use crate::proc::{self, Ended};

/// How many run ids are remembered: far more than ever wait at once.
const KEEP_DONE: usize = 500;

/// How long `amx ls` or `amx send` may take.
const AMX_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn bin(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

/// The done frames waiting for the socket.
static DONE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The worker's queue, once `start` has run.
static QUEUE: OnceLock<Sender<(String, Job)>> = OnceLock::new();

/// The done frames to send, taken.
pub fn dones() -> Vec<String> {
    std::mem::take(&mut *DONE.lock().unwrap_or_else(|e| e.into_inner()))
}

fn say_done(id: &str, machine: &str) {
    let frame = json!({"t":"done","id":id,"machine":machine}).to_string();
    DONE.lock().unwrap_or_else(|e| e.into_inner()).push(frame);
}

fn done_file() -> Option<PathBuf> {
    Some(crate::config::state_home().ok()?.join("hub/commands-done"))
}

fn ran_before(id: &str) -> bool {
    done_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| text.lines().any(|line| line == id))
}

/// Notes `id` as taken before it runs: a hub killed mid-run must not run it
/// twice; the store's resend then only hears done.
fn mark_ran(id: &str) {
    let Some(path) = done_file() else {
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut ids: VecDeque<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    ids.push_back(id);
    while ids.len() > KEEP_DONE {
        ids.pop_front();
    }
    let mut body = ids.into_iter().collect::<Vec<_>>().join("\n");
    body.push('\n');
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("tmp");
    let kept = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, &path));
    if let Err(e) = kept {
        eprintln!("hub: commands: keeping {id} as run: {e}");
    }
}

/// Starts the worker that runs commands one at a time, so a slow one never
/// holds up the socket.
pub fn start(machine: String) {
    let (tx, rx) = channel::<(String, Job)>();
    if QUEUE.set(tx).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("hub-commands".to_string())
        .spawn(move || {
            for (id, job) in rx {
                if !ran_before(&id) {
                    mark_ran(&id);
                    run(&job);
                }
                say_done(&id, &machine);
            }
        });
    if let Err(e) = spawned {
        eprintln!("hub: commands: starting the worker: {e}");
    }
}

/// Takes one cmd frame from the store: queued when it names this machine,
/// left alone otherwise (another machine's hub runs it).
pub fn take(frame: &str) {
    let Some(queue) = QUEUE.get() else {
        return;
    };
    let Ok(message) = serde_json::from_str::<Value>(frame) else {
        return;
    };
    let (Some(id), Some(machine)) = (message["id"].as_str(), message["machine"].as_str()) else {
        return;
    };
    if fleet::runner_name().as_deref() != Some(machine) {
        return;
    }
    match serde_json::from_value::<Job>(message["command"].clone()) {
        Ok(job) => {
            let _ = queue.send((id.to_string(), job));
        }
        Err(e) => {
            // One this hub cannot read is answered, so it stops coming back.
            eprintln!("hub: commands: skipping {id}: {e}");
            say_done(id, machine);
        }
    }
}

/// Runs `program args` and says whether it exited 0.
fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `workflow go` in a scope of its own, so stopping the hub's unit never
/// stops the orchestrator it started.
pub fn start_project(project: &str, cloud: bool) -> bool {
    let mut args = vec!["--user", "--scope", "--quiet", "workflow", "go", project];
    if cloud {
        args.push("--cloud");
    }
    let ok = succeeds(&bin("WORKFLOW_SYSTEMD_RUN", "systemd-run"), &args);
    eprintln!(
        "hub: commands: start {project}: {}",
        if ok { "started" } else { "failed" }
    );
    ok
}

/// What amx says of session `id`: `Some(true)` live, `Some(false)` stopped,
/// `None` when amx does not list it or cannot say.
pub fn session_live(id: &str) -> Option<bool> {
    let agents = fleet::agents()?;
    let row = agents.iter().find(|a| a["id"] == id)?;
    let ended = row["ended"].as_i64().unwrap_or(0);
    let state = row["state"].as_str().unwrap_or("");
    Some(ended == 0 && state != "stopped" && state != "failed")
}

/// Resumes session `id` with `message` once it has stopped; starts the
/// project afresh when amx has lost it. A live session reads its answers
/// itself.
pub fn resume(project: &str, id: &str, message: &str) {
    match session_live(id) {
        Some(true) => eprintln!("hub: commands: resume {id}: live, it reads its answers itself"),
        Some(false) => {
            let amx = fleet::amx_bin();
            let ok = succeeds(
                &bin("WORKFLOW_SYSTEMD_RUN", "systemd-run"),
                &["--user", "--scope", "--quiet", &amx, "resume", id, message],
            );
            if ok {
                eprintln!("hub: commands: resume {id}: resumed");
            } else {
                eprintln!("hub: commands: resume {id}: amx failed, start {project}");
                start_project(project, false);
            }
        }
        None => {
            eprintln!("hub: commands: resume {id}: session gone, start {project}");
            start_project(project, false);
        }
    }
}

fn run(job: &Job) {
    match job {
        // The replica is the store's own copy now: nothing to sync.
        Job::Sync => {}
        Job::Start { project, cloud } => {
            start_project(project, *cloud);
        }
        Job::Resume {
            project,
            id,
            message,
        } => resume(project, id, message),
        Job::FollowUp {
            project,
            id,
            message,
        } => {
            if session_live(id) != Some(true) {
                resume(project, id, message);
                return;
            }
            let mut command = Command::new(fleet::amx_bin());
            command.args(["send", id, message]);
            let sent = matches!(
                proc::output_within(&mut command, AMX_TIMEOUT),
                Ended::Exited(done) if done.code == Some(0)
            );
            eprintln!(
                "hub: commands: follow-up {id}: {}",
                if sent { "sent" } else { "failed" }
            );
        }
        Job::Notify { title, body } => {
            if !succeeds(
                &bin("WORKFLOW_NOTIFY_SEND", "notify-send"),
                &["-a", "workflow", title, body],
            ) {
                eprintln!("hub: commands: notify-send could not show {title:?}");
            }
        }
        // A cloud session takes its answers over Tailscale now.
        Job::Cloud { id, .. } => eprintln!("hub: commands: cloud {id}: no longer forwarded"),
    }
}

/// When each project was last resumed from a Resume tap, so a second tap
/// on the same button starts no second orchestrator.
static RESUMED: Mutex<Vec<(String, std::time::Instant)>> = Mutex::new(Vec::new());

/// How long after a resume another for the same project is refused.
const RESUME_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// A Resume tap on a stall: `workflow go <project>` here, when this machine
/// is its runner and its session is not live. The HTTP status says how it
/// went: 200 started, 404 not a run of this machine, 409 already running
/// (or resumed a moment ago), 502 the start failed.
pub fn resume_stalled(project: &str) -> u16 {
    let Some(machine) = fleet::runner_name() else {
        return 404;
    };
    let mut command = Command::new("mem");
    command.args(["projects", "--json"]);
    let projects: Value = match proc::output_within(&mut command, AMX_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => {
            serde_json::from_slice(&done.stdout).unwrap_or_default()
        }
        _ => return 502,
    };
    let runner = projects["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == project)
        .map(|p| p["runner"].clone())
        .filter(|r| r["machine"] == machine.as_str());
    let Some(runner) = runner else {
        return 404;
    };
    if runner["id"]
        .as_str()
        .is_some_and(|id| session_live(id) == Some(true))
    {
        return 409;
    }
    let mut resumed = RESUMED.lock().unwrap_or_else(|e| e.into_inner());
    resumed.retain(|(_, at)| at.elapsed() < RESUME_AGAIN_AFTER);
    if resumed.iter().any(|(p, _)| p == project) {
        return 409;
    }
    if !start_project(project, false) {
        return 502;
    }
    resumed.push((project.to_string(), std::time::Instant::now()));
    200
}
