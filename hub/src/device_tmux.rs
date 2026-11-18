//! `device tmux send|read|end`: one tmux session on this device for a
//! thread, named `cloud-<project>-<milestone>` so Saiful can attach to it and
//! watch, and working in a worktree of the project's repo under
//! `~/.cache/workflow-cloud/`.
//! A device runs one such job at a time; it ends on `end`, after the idle
//! limit, at its 20-minute cap, and when its cloud run lands or parks.

use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::device::{HELPER_TIMEOUT, Outcome, find_in, unix_now, user_command, user_env};
use crate::device_worktree::Submodules;
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

pub(crate) fn call(
    machine: &str,
    session: Option<&str>,
    project: &str,
    args: &[String],
) -> Outcome {
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
        "send" => send(&tmux, machine, &name, project, rest),
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
        crate::device_worktree::sweep();
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
        unwall(tmux, &job.name);
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

fn send(tmux: &Tmux, machine: &str, name: &str, project: &str, rest: &[String]) -> Outcome {
    let (yes, rest) = match rest {
        [flag, id, rest @ ..] if flag == "--yes" => (Some(id.trim_start_matches('#')), rest),
        _ => (None, rest),
    };
    let (outside, rest) = match rest {
        [flag, rest @ ..] if flag == "--no-wall" => (true, rest),
        _ => (false, rest),
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
    let mut spent = Vec::new();
    let asked = if outside {
        Some(Asked::Outside)
    } else {
        crate::held::held(&line).then_some(Asked::Line)
    };
    if let Some(kind) = asked {
        let Some(id) = yes else {
            return ask(project, machine, kind, &line);
        };
        if let Err(refused) = released(project, id, &line, kind) {
            return refused;
        }
        spent.push(id.to_string());
    }
    if !tmux.has(name) {
        // A yes that released no line may be the one for the whole job.
        let job = yes.filter(|_| spent.is_empty());
        match start(tmux, machine, name, project, job) {
            Ok(Some(id)) => spent.push(id),
            Ok(None) => {}
            Err(failed) => return failed,
        }
    }
    let sent = if outside {
        run_outside(tmux, name, &line)
    } else if is_key(&line) {
        // To the window in front, so C-c reaches a job outside the wall too.
        ok(
            tmux.run(&["send-keys", "-t", &format!("={name}:"), &line]),
            "send-keys",
        )
    } else {
        close_outside(tmux, name);
        type_line(tmux, &format!("={name}:={WALL}"), &line)
    };
    match sent {
        Ok(()) => {
            for id in &spent {
                spend(id);
            }
            Outcome::done(String::new())
        }
        Err(failed) => failed,
    }
}

/// The window of a job's shell, behind the wall unless Saiful let the job
/// run outside it, and the one a yes to a `--no-wall` line runs that line
/// in, outside the wall.
const WALL: &str = "wall";
const OUTSIDE: &str = "outside";

/// The answer that lets a job whose wall cannot start run outside it.
const RUN_OUTSIDE: &str = "run this job outside the wall";

/// Starts the job's session in its worktree, behind the wall once the
/// wall starts and the project's tools match outside it; else outside it
/// on a yes `job` to the device's question, which it returns to be spent,
/// and otherwise not at all, asking that question.
fn start(
    tmux: &Tmux,
    machine: &str,
    name: &str,
    project: &str,
    job: Option<&str>,
) -> Result<Option<String>, Outcome> {
    let work = crate::device_worktree::prepare(machine, project, name)?;
    let folder = work.folder.to_string_lossy().into_owned();
    let path = format!("PATH={}", crate::wall::shell_path(&tmux.env));
    let mut new = vec![
        "new-session",
        "-d",
        "-s",
        name,
        "-n",
        WALL,
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
    // A thread writes bash, and the user's own shell may be fish. No rc
    // file, which may turn history back on, and no `!` expansion.
    let Some(bash) = find_in(&tmux.path, "bash") else {
        return Err(Outcome::fail(
            127,
            "device: bash was not found\n".to_string(),
        ));
    };
    let bash = bash.to_string_lossy().into_owned();
    let shell = [&bash, "--noprofile", "--norc", "+H", "-i"].map(str::to_string);
    let Some(state) = state("hub/wall") else {
        return Err(Outcome::fail(
            126,
            "device: cannot find the state folder\n".to_string(),
        ));
    };
    let history = [("HISTFILE", "/dev/null"), ("HISTSIZE", "0")];
    let subdir = MemCli::new()
        .read(&["projects", "--json"])
        .rows("projects")
        .iter()
        .find(|row| row["name"].as_str() == Some(project))
        .and_then(|row| row["subdir"].as_str().map(str::to_string))
        .unwrap_or_default();
    let place = crate::wall::Place {
        root: &work.folder,
        cwd: &work.folder,
        writable: true,
    };
    // Both sides of the check run in Saiful's checkout, so they differ by
    // the wall alone: in the worktree, a tool the branch installed for
    // itself (cf hands off to a project's own cf) would read as the wall's.
    let checked = crate::wall::Place {
        root: &work.checkout,
        cwd: &work.checkout.join(&subdir),
        writable: false,
    };
    let walled =
        crate::wall::command(&tmux.env, &place, &history, &shell, &state).and_then(|walled| {
            match crate::wall::compare(&tmux.env, &checked, &work.checkout.join(&subdir), &state)? {
                seen if seen
                    .iter()
                    .all(|(_, s)| matches!(s, crate::wall::Seen::Same(_))) =>
                {
                    Ok(walled)
                }
                seen => Err(differences(&seen)),
            }
        });
    // A job's yes is spent once a job starts on it, walled or not, so no
    // later wall that cannot start finds it still good.
    let (mut command, spent) = match (walled, job) {
        (Ok(walled), job) => (walled, job.map(str::to_string)),
        (Err(why), Some(id)) => {
            released(project, id, &why, Asked::Job)?;
            (shell.to_vec(), Some(id.to_string()))
        }
        (Err(why), None) => {
            return Err(ask(project, machine, Asked::Job, &why));
        }
    };
    // The submodules come with the GitHub login the wall hides, so the hub
    // updates them before the wall starts, in the pane, where a first clone
    // may take longer than a call; not their own submodules, whose URLs
    // the commit a thread chose names.
    let first = match work.submodules {
        Submodules::None => None,
        Submodules::Update => Some("GIT_TERMINAL_PROMPT=0 git submodule update --init"),
        Submodules::Differ => {
            Some("echo hub: submodules not updated: the branch .gitmodules is not the checkout one")
        }
    };
    if let Some(first) = first {
        let before = [
            bash.as_str(),
            "--noprofile",
            "--norc",
            "-c",
            &format!("{first}; exec \"$@\""),
            "bash",
        ];
        command.splice(0..0, before.map(str::to_string));
    }
    new.extend(command.iter().map(String::as_str));
    ok(tmux.run(&new), "new-session")?;
    Ok(spent)
}

/// What the wall check found that is not the same on both sides.
fn differences(seen: &[(&str, crate::wall::Seen)]) -> String {
    seen.iter()
        .filter_map(|(tool, seen)| match seen {
            crate::wall::Seen::Same(_) => None,
            crate::wall::Seen::Differs { inside, outside } => {
                Some(format!("{tool} {inside} inside, {outside} outside"))
            }
            crate::wall::Seen::Missing => Some(format!("{tool} missing inside")),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Runs `line` outside the wall, in the job's worktree, in a window of its
/// own that stays to be read until the next line or the job's end.
fn run_outside(tmux: &Tmux, name: &str, line: &str) -> Result<(), Outcome> {
    let folder = tmux.run(&[
        "display-message",
        "-p",
        "-t",
        &format!("={name}:"),
        "#{session_path}",
    ])?;
    let folder = String::from_utf8_lossy(&folder.stdout).trim().to_string();
    let Some(bash) = find_in(&tmux.path, "bash") else {
        return Err(Outcome::fail(
            127,
            "device: bash was not found\n".to_string(),
        ));
    };
    close_outside(tmux, name);
    let script = r#"eval "$1"; echo "exit: $?"; exec sleep 3600"#;
    ok(
        tmux.run(&[
            "new-window",
            "-t",
            &format!("={name}:"),
            "-n",
            OUTSIDE,
            "-c",
            &folder,
            &bash.to_string_lossy(),
            "--noprofile",
            "--norc",
            "-c",
            script,
            "bash",
            line,
        ]),
        "new-window",
    )
}

/// Closes the window a `--no-wall` line ran in, if there is one.
fn close_outside(tmux: &Tmux, name: &str) {
    let _ = tmux.run(&["kill-window", "-t", &format!("={name}:={OUTSIDE}")]);
}

/// What a question the device asks is for, read from its first line, which
/// the device writes and no thread chooses.
#[derive(Clone, Copy, PartialEq)]
enum Asked {
    /// A held line, sent in the wall.
    Line,
    /// A line sent with `--no-wall`, run outside the wall.
    Outside,
    /// A job whose wall cannot start, run outside it.
    Job,
}

/// The first line a no-wall question starts with: the project and the
/// machine are plain names, so no line a thread sends can make it.
fn outside_head(project: &str, machine: &str) -> String {
    format!("A {project} thread asks to run this line outside the wall on {machine}:")
}

fn job_head(project: &str, machine: &str) -> String {
    format!("The wall cannot start on {machine} for {project}: ")
}

/// The first line a held line's question starts with.
fn line_head(project: &str, machine: &str) -> String {
    format!("A {project} thread asks to run this line in its tmux session on {machine}:")
}

/// The kind of a question the hub asked, None for any other question,
/// such as one Saiful's own agent asked on the device.
fn kind_of(question: &serde_json::Value, project: &str) -> Option<Asked> {
    let body = question["body"].as_str().unwrap_or_default();
    let head = body.lines().next().unwrap_or_default();
    let machine = question["machine"].as_str().unwrap_or_default();
    let offers = question["options"]
        .as_array()
        .is_some_and(|o| o.iter().any(|o| o.as_str() == Some(RUN_OUTSIDE)));
    if offers && head.starts_with(&job_head(project, machine)) {
        Some(Asked::Job)
    } else if head == outside_head(project, machine) {
        Some(Asked::Outside)
    } else if head == line_head(project, machine) {
        Some(Asked::Line)
    } else {
        None
    }
}

/// Asks Saiful on this device about `what`, a line or why the wall cannot
/// start, or finds the open question a device already asks it in, and
/// names it: a thread never asks it, so no thread writes the text his yes
/// releases.
fn ask(project: &str, machine: &str, kind: Asked, what: &str) -> Outcome {
    let mem = MemCli::new();
    let rows = mem
        .refresh(&["--project", project, "questions", "--json"])
        .rows("questions");
    let open = rows.iter().find(|q| {
        q["project"].as_str() == Some(project)
            && q["answered"].as_bool() != Some(true)
            && q["machine"]
                .as_str()
                .is_some_and(|m| crate::device::DEVICES.contains(&m))
            && kind_of(q, project) == Some(kind)
            && match kind {
                Asked::Job => q["body"].as_str().unwrap_or_default().contains(what),
                _ => asks(q["body"].as_str().unwrap_or_default(), what),
            }
    });
    let (text, options, shown) = match kind {
        Asked::Line => (
            format!(
                "{}\n\n    {what}\n\nYes sends it there once.",
                line_head(project, machine)
            ),
            ["yes", "no"],
            what.to_string(),
        ),
        Asked::Outside => (
            format!(
                "{}\n\n    {what}\n\nIt runs with your logins, in the thread's worktree. Yes runs it there once; the next line is walled again.",
                outside_head(project, machine)
            ),
            ["yes", "no"],
            format!("{what} (outside the wall)"),
        ),
        Asked::Job => (
            format!(
                "{}{what}\n\nRun this job outside the wall, with your logins, or hold it?",
                job_head(project, machine)
            ),
            [RUN_OUTSIDE, "hold"],
            format!("the wall cannot start on {machine}: {what}"),
        ),
    };
    let id = match open.and_then(|q| q["short_id"].as_str()) {
        Some(id) => id.to_string(),
        None => {
            let run = mem.write_through(&[
                "--project",
                project,
                "ask",
                "--for",
                "saiful",
                &text,
                "--option",
                options[0],
                "--option",
                options[1],
            ]);
            let out = String::from_utf8_lossy(&run.stdout);
            match out.trim().strip_prefix('#') {
                Some(id) if run.ok() && plain(id) => id.to_string(),
                _ => {
                    return Outcome::fail(
                        1,
                        format!("device: cannot ask about {shown}: {}\n", run.stderr),
                    );
                }
            }
        }
    };
    Outcome {
        stdout: format!("ask: {shown}\nasked: #{id}\n"),
        ..Outcome::fail(6, String::new())
    }
}

/// Whether a question's `body` asks for `line`: on a line of its own, or
/// in backticks.
fn asks(body: &str, line: &str) -> bool {
    body.lines().any(|l| l.trim() == line) || body.contains(&format!("`{line}`"))
}

/// Ok when question `id` of `project` was asked by a device, is of `kind`
/// and asks for `line` (a job's question, for any), and Saiful answered it
/// yes (a job's, to run outside the wall) on one of the devices or the hub
/// page, never a cloud thread's own `mem answer`, and no earlier line went
/// out on it.
fn released(project: &str, id: &str, line: &str, kind: Asked) -> Result<(), Outcome> {
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
    let asker = question["machine"].as_str().unwrap_or("no machine");
    if !crate::device::DEVICES.contains(&asker) {
        return Err(Outcome::refuse(format!(
            "#{id} was asked by {asker}, not by a device"
        )));
    }
    if kind_of(question, project) != Some(kind) {
        let what = match kind {
            Asked::Line => "a line in the wall",
            Asked::Outside => "a line outside the wall",
            Asked::Job => "a job outside the wall",
        };
        return Err(Outcome::refuse(format!("#{id} does not ask for {what}")));
    }
    let body = question["body"].as_str().unwrap_or_default();
    let named = match kind {
        // The reason the wall cannot start now, not one it had before.
        Asked::Job => body.lines().next() == Some(&format!("{}{line}", job_head(project, asker))),
        _ => asks(body, line),
    };
    if !named {
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
    let agreed = match kind {
        Asked::Job => answer.trim().eq_ignore_ascii_case(RUN_OUTSIDE),
        _ => first.eq_ignore_ascii_case("yes"),
    };
    if !agreed {
        return Err(Outcome::refuse(format!(
            "the answer to #{id} is {answer}, not yes"
        )));
    }
    let machine = question["answer_id"]
        .as_str()
        .map(|answer| mem.refresh(&["show", answer, "--json"]).rows("items"))
        .and_then(|items| items.first()?["machine"].as_str().map(str::to_string));
    let machine = machine.as_deref().unwrap_or("no machine");
    if !crate::device::his_word(machine) {
        return Err(Outcome::refuse(format!(
            "the yes for #{id} came from {machine}, not from a device or the hub page"
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
            let mut pane = String::from_utf8_lossy(&done.stdout).into_owned();
            if let Some(status) = silence(tmux, name) {
                if !pane.is_empty() && !pane.ends_with('\n') {
                    pane.push('\n');
                }
                pane.push_str(&status);
            }
            Outcome::done(pane)
        }
        other => ok(other, "capture-pane")
            .err()
            .unwrap_or_else(|| Outcome::fail(1, "device: tmux capture-pane failed\n".to_string())),
    }
}

/// How long a line may print nothing before a read says what it still
/// waits on: `HUB_DEVICE_SILENT_MS`, else a minute.
fn silent_limit() -> Duration {
    std::env::var("HUB_DEVICE_SILENT_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(Duration::from_secs(60), Duration::from_millis)
}

/// The line a read adds when the line in front has printed nothing for the
/// silent limit and has not ended: the pane never holds it.
fn silence(tmux: &Tmux, name: &str) -> Option<String> {
    let done = tmux
        .run(&[
            "display-message",
            "-p",
            "-t",
            &format!("={name}:"),
            "#{window_activity} #{pane_pid}",
        ])
        .ok()
        .filter(|done| done.code == Some(0))?;
    let out = String::from_utf8_lossy(&done.stdout);
    let mut words = out.split_whitespace();
    let activity: u64 = words.next()?.parse().ok()?;
    let pane: u32 = words.next()?.parse().ok()?;
    // tmux keeps the time to the second: count the output as at its end, so
    // the line comes late rather than early.
    let silent = unix_now().saturating_sub((activity + 1) * 1000);
    if Duration::from_millis(silent) < silent_limit() {
        return None;
    }
    let names = waits_on(pane);
    (!names.is_empty()).then(|| status_line(silent, &names))
}

fn status_line(silent_ms: u64, names: &[String]) -> String {
    let named = match names.len() {
        0..=5 => names.join(", "),
        n => format!("{} and {} more", names[..5].join(", "), n - 5),
    };
    format!(
        "device: no output for {} min; the line still waits on: {named}\n",
        silent_ms / 60_000
    )
}

/// What a process's stat says of it.
struct Stat {
    pid: u32,
    ppid: u32,
    pgrp: i64,
    tpgid: i64,
    zombie: bool,
    started: u64,
    comm: String,
}

fn stat(pid: u32) -> Option<Stat> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command's name may hold spaces and parentheses.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    // From the state, field 3: ppid 4, pgrp 5, tpgid 8, starttime 22.
    Some(Stat {
        pid,
        ppid: rest.get(1)?.parse().ok()?,
        pgrp: rest.get(2)?.parse().ok()?,
        tpgid: rest.get(5)?.parse().ok()?,
        zombie: *rest.first()? == "Z",
        started: rest.get(19)?.parse().ok()?,
        comm,
    })
}

/// The names of the commands the pane's line still runs, in the order they
/// started: the live children, in the terminal's foreground group, of the
/// line's shell: the pane's bash, or the bash the wall's pasta and bwraps
/// run. A shell back at its prompt
/// has a foreground of its own, and the window outside the wall, whose bash
/// ends in `sleep`, has no bash once its line ended.
fn waits_on(pane: u32) -> Vec<String> {
    let all: Vec<Stat> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter_map(stat)
        .collect();
    let children = |of: u32| all.iter().filter(move |s| s.ppid == of);
    let Some(top) = all.iter().find(|s| s.pid == pane) else {
        return Vec::new();
    };
    // The wall's pasta and two bwraps each start one child, down to its
    // bash; the inner bwrap, the wall's init, also takes in the orphans of
    // the line, which start later.
    let mut shell = top;
    while wall_layer(&shell.comm) {
        match children(shell.pid).min_by_key(|s| (s.started, s.pid)) {
            Some(first) => shell = first,
            None => return Vec::new(),
        }
    }
    if shell.comm != "bash" {
        return Vec::new();
    }
    let mut line: Vec<&Stat> = children(shell.pid)
        .filter(|s| !s.zombie && s.pgrp == shell.tpgid)
        .collect();
    line.sort_by_key(|s| (s.started, s.pid));
    line.iter().map(|s| s.comm.clone()).collect()
}

/// Whether a process named `comm` is the wall's pasta or one of its
/// bwraps. On an x86_64 device with AVX2, pasta runs as `pasta.avx2`.
fn wall_layer(comm: &str) -> bool {
    comm == "bwrap" || comm == "pasta" || comm.starts_with("pasta.")
}

fn end(tmux: &Tmux, name: &str) -> Outcome {
    if !tmux.has(name) {
        return Outcome::done(String::new());
    }
    unwall(tmux, name);
    match ok(
        tmux.run(&["kill-session", "-t", &format!("={name}")]),
        "kill-session",
    ) {
        Ok(()) => Outcome::done(String::new()),
        Err(failed) => failed,
    }
}

/// Types `line` into the pane as it is, then Enter.
fn type_line(tmux: &Tmux, target: &str, line: &str) -> Result<(), Outcome> {
    ok(
        tmux.run(&["send-keys", "-t", target, "-l", "--", line]),
        "send-keys",
    )?;
    ok(tmux.run(&["send-keys", "-t", target, "Enter"]), "send-keys")
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

/// Kills the init of each wall in `name`'s panes: the kernel then ends every
/// process in its pid namespace at once, one that ignores a hangup too,
/// before tmux ends the rest.
fn unwall(tmux: &Tmux, name: &str) {
    let Ok(panes) = tmux.run(&[
        "list-panes",
        "-s",
        "-t",
        &format!("={name}"),
        "-F",
        "#{pane_pid}",
    ]) else {
        return;
    };
    let mut parents: Vec<(u32, u32)> = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|p| p.parse().ok()) else {
            continue;
        };
        let status = std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
        let ppid = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|p| p.trim().parse().ok());
        if let Some(ppid) = ppid {
            parents.push((pid, ppid));
        }
    }
    let mut todo: Vec<u32> = String::from_utf8_lossy(&panes.stdout)
        .lines()
        .filter_map(|p| p.trim().parse().ok())
        .collect();
    while let Some(pid) = todo.pop() {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let init = status
            .lines()
            .find_map(|l| l.strip_prefix("NSpid:"))
            .is_some_and(|ids| {
                let ids: Vec<&str> = ids.split_whitespace().collect();
                ids.len() > 1 && ids.last() == Some(&"1")
            });
        if init {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            continue;
        }
        todo.extend(parents.iter().filter(|(_, p)| *p == pid).map(|(c, _)| *c));
    }
}

#[cfg(test)]
mod tests {
    use super::{status_line, wall_layer};

    #[test]
    fn the_walk_to_the_wall_s_bash_passes_pasta_s_own_builds_too() {
        for comm in ["pasta", "pasta.avx2", "bwrap"] {
            assert!(wall_layer(comm), "{comm}");
        }
        for comm in ["bash", "sleep", "cargo"] {
            assert!(!wall_layer(comm), "{comm}");
        }
    }

    #[test]
    fn the_status_line_counts_whole_minutes() {
        assert_eq!(
            status_line(185_000, &["sleep".to_string()]),
            "device: no output for 3 min; the line still waits on: sleep\n"
        );
    }
}
