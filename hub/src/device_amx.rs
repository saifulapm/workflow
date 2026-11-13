//! `device amx new|ls|result|status|send|answer|logs|stop`: the one amx agent
//! a cloud thread starts on this device. It works in the thread's folder under
//! `~/.cache/workflow-cloud/`, the thread drives that agent and no other, and
//! the sweep stops it once the thread's run lands or parks.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::device::{Outcome, find_in, timeout, user_command, user_env};
use crate::memcli::MemCli;
use crate::proc::{Ended, Finished, output_within};

/// The seconds `amx result` waits, under the device's 90 s for a call.
const RESULT_WAIT: &str = "80";

/// The agent this device started for a thread, at most one at a time.
#[derive(Serialize, Deserialize)]
struct Agent {
    project: String,
    session: String,
    name: String,
}

pub(crate) fn call(session: Option<&str>, project: &str, args: &[String]) -> Outcome {
    let Some(session) = session.filter(|s| {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }) else {
        return Outcome::refuse("amx needs a session".to_string());
    };
    let Some((verb, rest)) = args.split_first() else {
        return Outcome::refuse("amx needs a verb".to_string());
    };
    let verb = verb.as_str();
    if !matches!(
        verb,
        "new" | "ls" | "result" | "status" | "send" | "answer" | "logs" | "stop"
    ) {
        return Outcome::refuse(format!("amx {verb} is not allowed here"));
    }
    // The hub owns every flag. An answer's words are keys, not flags.
    if verb != "answer"
        && let Some(flag) = rest.iter().find(|w| w.starts_with('-'))
    {
        return Outcome::refuse(format!("amx {flag} is not allowed here"));
    }
    // The agent is a shell on the device as Saiful too: a line tmux holds
    // for his yes would let it write its own release. An answer's free words
    // reach it as a message does, `--text=<words>` among them.
    if matches!(verb, "new" | "send" | "answer")
        && crate::held::held(&rest.join(" ").replace('=', " "))
    {
        return Outcome::refuse(format!(
            "amx {verb} holds a line only Saiful releases: send it with device tmux, which asks him"
        ));
    }
    let amx = match Amx::here() {
        Ok(amx) => amx,
        Err(failed) => return failed,
    };
    if verb == "new" {
        return new(&amx, session, project, rest);
    }
    if verb == "ls" {
        return ls(&amx, session);
    }
    let Some((name, words)) = rest.split_first() else {
        return Outcome::refuse(format!("amx {verb} needs the agent's name"));
    };
    let held = lock();
    if !load().is_some_and(|a| a.session == session && &a.name == name) {
        return Outcome::refuse(format!("{name} is not this thread's agent"));
    }
    if verb == "stop" {
        let mut stop = vec!["stop", "--force", name];
        stop.extend(words.iter().map(String::as_str));
        let stopped = amx.run(&stop);
        if stopped.as_ref().is_ok_and(gone) {
            forget();
        }
        return passed(stopped);
    }
    // Only new, stop and the sweep change the record, and a result can wait
    // 80 s, so the rest run without the lock.
    drop(held);
    if verb == "result" {
        if !words.is_empty() {
            return Outcome::refuse("amx result takes only the agent's name".to_string());
        }
        return match amx.run(&["result", name, "--timeout", RESULT_WAIT]) {
            Ok(done) if done.code == Some(3) => Outcome {
                stdout: format!("again: device amx result {name}\n"),
                ..Outcome::fail(9, String::new())
            },
            other => passed(other),
        };
    }
    let mut pass = vec![verb, name];
    pass.extend(words.iter().map(String::as_str));
    passed(amx.run(&pass))
}

fn new(amx: &Amx, session: &str, project: &str, task: &[String]) -> Outcome {
    if task.is_empty() {
        return Outcome::refuse("amx new needs a task".to_string());
    }
    let _held = lock();
    if let Some(agent) = load() {
        return Outcome {
            stdout: format!("busy: {}\n", agent.project),
            ..Outcome::fail(5, String::new())
        };
    }
    let folder =
        match crate::device_tmux::job_name(project).and_then(|n| crate::device_tmux::folder(&n)) {
            Ok(folder) => folder.to_string_lossy().into_owned(),
            Err(refused) => return refused,
        };
    let done = match amx.run(&["new", "--dir", &folder, &task.join(" ")]) {
        Ok(done) if done.code == Some(0) => done,
        other => return passed(other),
    };
    let stdout = String::from_utf8_lossy(&done.stdout);
    let Some(name) = stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    else {
        return passed(Ok(done));
    };
    let agent = Agent {
        project: project.to_string(),
        session: session.to_string(),
        name: name.to_string(),
    };
    if let Err(failed) = save(&agent) {
        return failed;
    }
    Outcome::done(format!("{name}\n"))
}

fn ls(amx: &Amx, session: &str) -> Outcome {
    let agent = {
        let _held = lock();
        load()
    };
    let Some(agent) = agent.filter(|a| a.session == session) else {
        return Outcome::done("[]\n".to_string());
    };
    let done = match amx.run(&["ls", "--json"]) {
        Ok(done) if done.code == Some(0) => done,
        other => return passed(other),
    };
    let Ok(all) = serde_json::from_slice::<Vec<Value>>(&done.stdout) else {
        return Outcome::fail(1, "device: amx ls --json printed no list\n".to_string());
    };
    let own: Vec<Value> = all
        .into_iter()
        .filter(|a| a["id"].as_str() == Some(agent.name.as_str()))
        .collect();
    Outcome::done(format!(
        "{}\n",
        serde_json::to_string_pretty(&own).unwrap_or_default()
    ))
}

/// Stops the agent the record names once its thread's run has landed or
/// parked, and forgets it. Every other agent on the device is Saiful's.
pub(crate) fn sweep(mem: &MemCli, projects: &OnceCell<Vec<Value>>) {
    let _held = lock();
    let Some(agent) = load() else {
        return;
    };
    if !crate::device::run_ended(&agent.session, &agent.project, mem, projects) {
        return;
    }
    // A stop that failed keeps the record, so the next round tries again;
    // an agent amx no longer knows is gone already.
    let stopped = Amx::here().and_then(|amx| amx.run(&["stop", "--force", &agent.name]));
    if !stopped.as_ref().is_ok_and(gone) {
        eprintln!(
            "hub: device: could not stop amx agent {} of {}; trying again next round",
            agent.name, agent.project
        );
        return;
    }
    forget();
    eprintln!(
        "hub: device: stopped amx agent {} of {}",
        agent.name, agent.project
    );
}

/// Whether a stop left no agent: it stopped it, or amx no longer knows it,
/// as after Saiful's own `amx clear`.
fn gone(done: &Finished) -> bool {
    done.code == Some(0) || String::from_utf8_lossy(&done.stderr).contains("no agent")
}

/// Held across each read, change and write of the record.
fn lock() -> MutexGuard<'static, ()> {
    static RECORD: Mutex<()> = Mutex::new(());
    RECORD.lock().unwrap_or_else(|e| e.into_inner())
}

/// Not under hub/device/, whose every folder the browser sweep reads as a
/// session.
fn record() -> anyhow::Result<PathBuf> {
    Ok(crate::config::state_home()?.join("hub/device-amx.json"))
}

fn load() -> Option<Agent> {
    let bytes = std::fs::read(record().ok()?).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save(agent: &Agent) -> Result<(), Outcome> {
    let written = record().and_then(|path| {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_vec(agent)?)?;
        Ok(())
    });
    written.map_err(|e| Outcome::fail(126, format!("device: cannot keep the amx agent: {e:#}\n")))
}

fn forget() {
    if let Ok(path) = record() {
        let _ = std::fs::remove_file(path);
    }
}

/// amx's own answer, stdout, stderr and exit as it gave them.
fn passed(done: Result<Finished, Outcome>) -> Outcome {
    match done {
        Ok(done) => Outcome {
            stdout: String::from_utf8_lossy(&done.stdout).into_owned(),
            ..Outcome::fail(
                // A child killed by a signal has no code; see device::run.
                done.code.unwrap_or(128),
                String::from_utf8_lossy(&done.stderr).into_owned(),
            )
        },
        Err(failed) => failed,
    }
}

/// amx on the user manager's PATH, with the user manager's environment.
struct Amx {
    bin: PathBuf,
    env: Vec<(String, String)>,
}

impl Amx {
    fn here() -> Result<Amx, Outcome> {
        let env = user_env();
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let Some(bin) = find_in(&path, "amx") else {
            return Err(Outcome::fail(
                127,
                "device: amx was not found\n".to_string(),
            ));
        };
        Ok(Amx { bin, env })
    }

    fn run(&self, args: &[&str]) -> Result<Finished, Outcome> {
        let timeout = timeout();
        // The user's environment, not the hub's tokens, and `new`, which may
        // start the tmux server amx runs its agents in, in a scope of its own.
        let mut command = user_command(&self.bin, &self.env, args[0] == "new");
        command.args(args);
        match output_within(&mut command, timeout) {
            Ended::Exited(done) => Ok(done),
            Ended::TimedOut => Err(Outcome::fail(
                124,
                format!(
                    "device: amx ran past {} s and was stopped\n",
                    timeout.as_secs_f64()
                ),
            )),
            Ended::Failed(e) => Err(Outcome::fail(
                126,
                format!("device: amx did not start: {e}\n"),
            )),
        }
    }
}
