//! `device tmux send|read|end`: one tmux session on this device for a
//! thread, named `cloud-<project>-<milestone>` so Saiful can attach to it and
//! watch, and working in its own folder under `~/.cache/workflow-cloud/`.

use std::path::PathBuf;
use std::process::Command;

use crate::device::{HELPER_TIMEOUT, Outcome, find_in, user_path};
use crate::memcli::MemCli;
use crate::proc::{Ended, Finished, output_within};

/// The keys a send passes as themselves, with no Enter after them, besides
/// C-<key> and M-<key>.
const KEYS: [&str; 10] = [
    "Enter", "Escape", "Tab", "BSpace", "Up", "Down", "Left", "Right", "PageUp", "PageDown",
];

/// How far above the visible pane a read reaches.
const HISTORY: &str = "-200";

pub(crate) fn call(project: &str, args: &[String]) -> Outcome {
    let Some((verb, rest)) = args.split_first() else {
        return Outcome::refuse("tmux needs send, read or end".to_string());
    };
    if !matches!(verb.as_str(), "send" | "read" | "end") {
        return Outcome::refuse(format!("tmux {verb} is not a verb (send, read, end)"));
    }
    let name = match job_name(project) {
        Ok(name) => name,
        Err(refused) => return refused,
    };
    let tmux = match Tmux::here() {
        Ok(tmux) => tmux,
        Err(failed) => return failed,
    };
    match verb.as_str() {
        "send" => send(&tmux, &name, rest),
        "read" => read(&tmux, &name),
        _ => end(&tmux, &name),
    }
}

/// `cloud-<project>-<milestone>`, from the project's open milestone in mem.
fn job_name(project: &str) -> Result<String, Outcome> {
    let rows = MemCli::new().read(&["projects", "--json"]).rows("projects");
    let milestone = rows
        .iter()
        .find(|row| row["name"].as_str() == Some(project))
        .and_then(|row| row["milestone"].as_str());
    let Some(milestone) = milestone else {
        return Err(Outcome::refuse(format!("{project} has no open milestone")));
    };
    let plain = |s: &str| {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if !plain(project) || !plain(milestone) {
        return Err(Outcome::refuse(format!(
            "{project} {milestone} cannot name a tmux session"
        )));
    }
    Ok(format!("cloud-{project}-{milestone}"))
}

fn send(tmux: &Tmux, name: &str, rest: &[String]) -> Outcome {
    if rest.is_empty() {
        return Outcome::refuse("tmux send needs a line".to_string());
    }
    let line = rest.join(" ");
    if !tmux.has(name) {
        let Some(home) = std::env::var_os("HOME") else {
            return Outcome::fail(126, "device: HOME is not set\n".to_string());
        };
        let folder = PathBuf::from(home).join(".cache/workflow-cloud").join(name);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            return Outcome::fail(
                126,
                format!("device: cannot make {}: {e}\n", folder.display()),
            );
        }
        let folder = folder.to_string_lossy().into_owned();
        let path = format!("PATH={}", tmux.path);
        let made = tmux.run(&[
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
        ]);
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
        Ok(()) => Outcome::done(String::new()),
        Err(failed) => failed,
    }
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
}

impl Tmux {
    pub(crate) fn here() -> Result<Tmux, Outcome> {
        let path = user_path();
        let Some(bin) = find_in(&path, "tmux") else {
            return Err(Outcome::fail(
                127,
                "device: tmux was not found\n".to_string(),
            ));
        };
        Ok(Tmux { bin, path })
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        matches!(
            self.run(&["has-session", "-t", &format!("={name}")]),
            Ok(done) if done.code == Some(0)
        )
    }

    pub(crate) fn run(&self, args: &[&str]) -> Result<Finished, Outcome> {
        let mut command = Command::new(&self.bin);
        if let Some(socket) = std::env::var_os("HUB_DEVICE_TMUX_SOCKET").filter(|s| !s.is_empty()) {
            command.arg("-S").arg(socket);
        }
        command
            .args(args)
            .env("PATH", &self.path)
            .env_remove("TMUX");
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
