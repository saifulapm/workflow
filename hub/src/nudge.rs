//! A cloud thread that parked on its questions has no session here to
//! resume: it wakes only on a new message. So once Saiful has answered
//! every question a parked cloud run asked, from any place, the mini's hub
//! sends that thread one message with `claude -p --cloud <session>`, and he
//! types nothing on claude.ai. Only the mini nudges, so two hubs never
//! nudge one park twice. The park is the one `workflow go --cloud` reads
//! (workflow/src/cloud.rs `parked_questions`): a `parked:` handoff written
//! after the run's latest record, its milestone still the open one, and
//! the questions of its project asked since the run first started. Only a
//! `parked: question` or `parked: blocked:` handoff counts, and the nudge
//! waits a minute after the newest answer.
//!
//! A live run gets each answer as he gives it: a thread's safety check reads
//! what is typed in the thread, never what `workflow wait-answer` prints, so
//! the mini types his answer in as a message of its own. Only an answer a
//! device or the hub page wrote is his word; one a cloud machine wrote, the
//! thread's own `mem answer` among them, is never typed in.
//!
//! Both go out with the claude.ai login of the run's own account, which
//! dots' registry places in a folder of its own; with no login for that
//! account here, the mini rings Saiful once on Slack instead.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::proc::{self, Ended};
use crate::slack::Slack;

/// The one machine that nudges.
const MACHINE: &str = "mac-mini-m2";
const MEM_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_TIMEOUT: Duration = Duration::from_secs(60);
/// How long after the newest answer the nudge waits (`HUB_NUDGE_SETTLE_MS`).
const SETTLE: Duration = Duration::from_secs(60);
/// How many times one park's nudge is sent before the hub gives it up.
const TRIES: usize = 3;

fn mem_out(args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("mem");
    command.args(args);
    match proc::output_within(&mut command, MEM_TIMEOUT) {
        Ended::Exited(done) if done.code == Some(0) => Some(done.stdout),
        _ => None,
    }
}

fn mem_json(args: &[&str]) -> Option<Value> {
    serde_json::from_slice(&mem_out(args)?).ok()
}

fn nudged_file() -> Option<PathBuf> {
    Some(crate::config::state_home().ok()?.join("hub/nudged"))
}

/// What was sent already: the parks nudged, as `<session> <handoff time>`
/// lines, the answers typed in, as `answer <id>`, those not typed in (a
/// failed send, or not his word), as `tried <id>`, and the runs whose
/// account had no login, as `<session> no login`.
fn read_nudged() -> HashSet<String> {
    nudged_file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn write_nudged(nudged: &HashSet<String>) {
    let Some(path) = nudged_file() else {
        return;
    };
    let mut lines: Vec<&str> = nudged.iter().map(String::as_str).collect();
    lines.sort();
    let mut body = lines.join("\n");
    body.push('\n');
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let temp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, &path)) {
        eprintln!("hub: nudge: keeping what was nudged: {e}");
    }
}

fn time(value: &Value) -> Option<jiff::Timestamp> {
    value.as_str()?.parse().ok()
}

/// A parked cloud run of `project`: its session, the park's key and
/// whether it parked on a blocked step, or None when the run is live,
/// another machine's, or its milestone landed.
fn park(project: &Value) -> Option<(String, String, bool)> {
    let name = project["name"].as_str()?;
    let runner = &project["runner"];
    if runner["machine"] != "cloud" || runner["milestone"].is_null() {
        return None;
    }
    if runner["milestone"] != project["milestone"] {
        return None;
    }
    let session = runner["id"].as_str().filter(|id| !id.is_empty())?;
    let since = time(&runner["since"])?;
    let at = project["handoff_at"].as_str()?;
    if at.parse::<jiff::Timestamp>().ok()? <= since {
        return None;
    }
    let handoff = mem_out(&["handoff", "--project", name])?;
    let handoff = String::from_utf8_lossy(&handoff);
    let handoff = handoff.trim_start();
    // Only a park on questions waits for answers: a hold, a 6-hour stop or
    // a local-only park (a device takes it over) does not, however its
    // questions stand. One question or several: `parked: question #<id>`,
    // `parked: questions #<id> and #<id>`; or a step the safety check
    // blocked, asked about: `parked: blocked: <step> (#<id>)`.
    let blocked = handoff.starts_with("parked: blocked:");
    if !handoff.starts_with("parked: question") && !blocked {
        return None;
    }
    Some((session.to_string(), format!("{session} {at}"), blocked))
}

/// The newest answer is `settle` old or more. The wait lets a message he
/// types in the thread himself re-record the run first, so it gets no
/// second turn from the nudge.
fn settled(asked: &[Value], settle: Duration) -> bool {
    let settled = jiff::Timestamp::now().as_millisecond() - settle.as_millis() as i64;
    asked
        .iter()
        .filter_map(|q| hubpages::model::ulid_millis(q["answer_id"].as_str()?))
        .max()
        .is_some_and(|ms| ms <= settled)
}

/// The questions the run asked since it first started, oldest first; a
/// thread has no amx session and asks Saiful, never an orchestrator.
fn asked(project: &Value) -> Option<Vec<Value>> {
    let name = project["name"].as_str()?;
    let runner = &project["runner"];
    let first = time(&runner["first"]).or_else(|| time(&runner["since"]))?;
    let questions = mem_json(&["questions", "--json", "--project", name])?;
    let mut asked: Vec<Value> = questions["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|q| {
            q["id"]
                .as_str()
                .and_then(hubpages::model::ulid_millis)
                .is_some_and(|ms| ms > first.as_millisecond())
        })
        .filter(|q| q["session"].as_str().is_none_or(str::is_empty))
        .filter(|q| q["audience"] != "orchestrator")
        .cloned()
        .collect();
    asked.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Some(asked)
}

/// A live cloud run of `project`, its milestone the open one and no park
/// written since its latest record: its session and that record's time.
fn live(project: &Value) -> Option<(String, jiff::Timestamp)> {
    let runner = &project["runner"];
    if runner["machine"] != "cloud" || runner["milestone"].is_null() {
        return None;
    }
    if runner["milestone"] != project["milestone"] {
        return None;
    }
    let session = runner["id"].as_str().filter(|id| !id.is_empty())?;
    let since = time(&runner["since"])?;
    if time(&project["handoff_at"]).is_some_and(|at| at > since) {
        let name = project["name"].as_str()?;
        let handoff = mem_out(&["handoff", "--project", name])?;
        if String::from_utf8_lossy(&handoff)
            .trim_start()
            .starts_with("parked:")
        {
            return None;
        }
    }
    Some((session.to_string(), since))
}

/// The machine that wrote the answer item `id`.
fn writer(id: &str) -> Option<String> {
    let shown = mem_json(&["show", id, "--json"])?;
    Some(
        shown["items"].as_array()?.first()?["machine"]
            .as_str()?
            .to_string(),
    )
}

/// The relay of one answer, as the thread reads it.
fn relay(row: &Value) -> String {
    format!(
        "Saiful answered your question #{} (\"{}\"): \"{}\". His hub typed this for him.",
        row["short_id"].as_str().unwrap_or(""),
        row["body"].as_str().unwrap_or(""),
        row["answer"].as_str().unwrap_or(""),
    )
}

/// What the woken thread reads: each answer of his not typed into it yet,
/// in the relay's words, so its safety check reads them as his. Answers
/// the thread wrote itself, or that went in already, are in its history.
fn message(project: &str, fresh: &[&Value]) -> String {
    let mut text: String = fresh.iter().map(|row| relay(row) + "\n").collect();
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&format!(
        "Every question this thread parked on is answered; the mini's hub sends \
         this for Saiful. Carry on with {project}'s milestone as your \
         instructions say for a new turn after a park. `mem questions` shows \
         the answers too."
    ));
    text
}

/// The claude login of the claude.ai `account`, from dots' registry
/// (`$XDG_STATE_HOME/dots/claude-accounts.json`, an id to its home): the
/// folder for CLAUDE_CONFIG_DIR, or None for the default ~/.claude, which
/// is main's when the registry is missing. `Err` when the account has no
/// login on this machine: no home, or one without credentials. The hub
/// runs the vendor claude, which reads only CLAUDE_CONFIG_DIR.
fn login(account: &str) -> Result<Option<PathBuf>, ()> {
    let default = PathBuf::from(std::env::var_os("HOME").ok_or(())?).join(".claude");
    let registry: Value = crate::config::state_home()
        .ok()
        .and_then(|state| std::fs::read(state.join("dots/claude-accounts.json")).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let home = registry["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["id"] == account)
        .and_then(|row| row["home"].as_str())
        .map(PathBuf::from)
        .or_else(|| (account == "main").then(|| default.clone()))
        .ok_or(())?;
    if !home.join(".credentials.json").is_file() {
        return Err(());
    }
    Ok((home != default).then_some(home))
}

/// One `claude -p --cloud` send of `text` to `session`, logged in as
/// `login` says.
fn send(session: &str, text: &str, login: &Option<PathBuf>) -> Result<(), String> {
    let mut command = Command::new("claude");
    command.args(["-p", "--cloud", session, "--output-format", "json"]);
    match login {
        Some(home) => command.env("CLAUDE_CONFIG_DIR", home),
        None => command.env_remove("CLAUDE_CONFIG_DIR"),
    };
    if let Ok(dir) = crate::config::state_home() {
        command.current_dir(dir);
    }
    match proc::output_fed(&mut command, Some(text.as_bytes()), SEND_TIMEOUT) {
        Ended::Exited(done) => {
            let reply: Value = serde_json::from_slice(&done.stdout).unwrap_or_default();
            if done.code == Some(0) && reply["ok"] == true {
                Ok(())
            } else {
                let error = reply["error"].as_str().map_or_else(
                    || String::from_utf8_lossy(&done.stderr).trim().to_string(),
                    str::to_string,
                );
                Err(format!("exit {:?}: {error}", done.code))
            }
        }
        Ended::TimedOut => Err(format!("no answer in {} s", SEND_TIMEOUT.as_secs())),
        Ended::Failed(e) => Err(format!("claude did not start: {e}")),
    }
}

/// `pause` is the wait between tries of one nudge, `settle` the wait
/// after the newest answer.
fn round(nudged: &mut HashSet<String>, pause: Duration, settle: Duration, slack: Option<&Slack>) {
    let Some(projects) = mem_json(&["projects", "--json"]) else {
        return;
    };
    for project in projects["projects"].as_array().into_iter().flatten() {
        let Some((session, key, blocked)) = park(project) else {
            continue;
        };
        if nudged.contains(&key) {
            continue;
        }
        let Some(asked) = asked(project) else {
            continue;
        };
        if asked.is_empty() || asked.iter().any(|q| q["answered"] != true) {
            continue;
        }
        if !settled(&asked, settle) {
            continue;
        }
        // His answers not typed in yet, by the machine that wrote each; a
        // writer mem cannot name now is asked again next round.
        let mut fresh = Vec::new();
        let mut unread = false;
        for row in &asked {
            let Some(id) = row["answer_id"].as_str() else {
                continue;
            };
            match writer(id) {
                Some(by) if crate::device::his_word(&by) => {
                    if !nudged.contains(&format!("answer {id}")) {
                        fresh.push(row);
                    }
                }
                Some(_) => {}
                None => unread = true,
            }
        }
        if unread {
            continue;
        }
        // A park on a step still blocked after his answers went in gets no
        // second message with the same ones: it would only park again.
        if blocked && fresh.is_empty() {
            nudged.insert(key);
            write_nudged(nudged);
            continue;
        }
        let name = project["name"].as_str().unwrap_or_default();
        let text = message(name, &fresh);
        match deliver(project, &session, &text, pause, nudged, slack) {
            Some(Ok(())) => {
                eprintln!("hub: nudge: {name}: woke {session} with its answers");
                nudged.extend(
                    fresh
                        .iter()
                        .filter_map(|row| Some(format!("answer {}", row["answer_id"].as_str()?))),
                );
            }
            // Tried no more: a claude.ai change that breaks the send leaves a
            // typed message as the way to wake the thread.
            Some(Err(e)) => eprintln!(
                "hub: nudge: {name}: {session} did not take the nudge in {TRIES} tries, \
                 so it waits for a typed message: {e}"
            ),
            None => {}
        }
        nudged.insert(key);
        write_nudged(nudged);
    }
}

/// Types each answer a device or the hub page gave since a live cloud
/// run's latest record into its thread, one message each, in the order
/// the questions were asked.
fn relay_round(nudged: &mut HashSet<String>, pause: Duration, slack: Option<&Slack>) {
    let Some(projects) = mem_json(&["projects", "--json"]) else {
        return;
    };
    for project in projects["projects"].as_array().into_iter().flatten() {
        let Some((session, since)) = live(project) else {
            continue;
        };
        let Some(asked) = asked(project) else {
            continue;
        };
        let name = project["name"].as_str().unwrap_or_default();
        for row in asked {
            let Some(answer) = row["answer_id"]
                .as_str()
                .filter(|_| row["answered"] == true)
            else {
                continue;
            };
            let typed = format!("answer {answer}");
            let tried = format!("tried {answer}");
            // An answer from before this record reached the thread in its
            // prompt or its nudge.
            if nudged.contains(&typed)
                || nudged.contains(&tried)
                || hubpages::model::ulid_millis(answer)
                    .is_none_or(|ms| ms <= since.as_millisecond())
            {
                continue;
            }
            // A question a device asked is its agent's, not the thread's.
            if row["machine"].as_str().is_none_or(crate::device::his_word) {
                continue;
            }
            let Some(by) = writer(answer) else {
                continue;
            };
            // Only an answer that went in counts as typed: a park on a step
            // still blocked may be woken with one that did not.
            let mut went_in = false;
            if crate::device::his_word(&by) {
                let short = row["short_id"].as_str().unwrap_or_default();
                match deliver(project, &session, &relay(&row), pause, nudged, slack) {
                    Some(Ok(())) => {
                        went_in = true;
                        eprintln!(
                            "hub: nudge: {name}: typed the answer to #{short} into {session}"
                        );
                    }
                    Some(Err(e)) => eprintln!(
                        "hub: nudge: {name}: {session} did not take the answer to #{short} \
                         in {TRIES} tries: {e}"
                    ),
                    None => {}
                }
            }
            nudged.insert(if went_in { typed } else { tried });
            write_nudged(nudged);
        }
    }
}

/// Sends `text` to the run's thread with its account's login, tried
/// `TRIES` times; None when that account has no login here, after ringing
/// Saiful once for the run.
fn deliver(
    project: &Value,
    session: &str,
    text: &str,
    pause: Duration,
    nudged: &mut HashSet<String>,
    slack: Option<&Slack>,
) -> Option<Result<(), String>> {
    let name = project["name"].as_str().unwrap_or_default();
    // A run recorded before accounts were is main's.
    let account = project["runner"]["account"].as_str().unwrap_or("main");
    let Ok(login) = login(account) else {
        let rung = format!("{session} no login");
        if nudged.insert(rung) {
            write_nudged(nudged);
            let ring = format!("no claude login for the {account} account on {MACHINE}");
            eprintln!("hub: nudge: {name}: {ring}");
            if let Some(Err(e)) = slack.map(|slack| slack.post_text(&ring)) {
                eprintln!("hub: nudge: {name}: ringing Saiful: {e:#}");
            }
        }
        return None;
    };
    let mut result = send(session, text, &login);
    for _ in 1..TRIES {
        if result.is_ok() {
            break;
        }
        std::thread::sleep(pause);
        result = send(session, text, &login);
    }
    Some(result)
}

/// On the mini, checks every 10 s (`HUB_NUDGE_CHECK_MS`) for a parked
/// cloud run with all its answers, the newest a minute old, and for an
/// answer a live cloud run has not had typed in yet; on any other
/// machine, nothing. `slack` rings Saiful when a run's account has no
/// login here.
pub fn run(machine: &str, slack: Option<Arc<Slack>>) {
    if machine != MACHINE {
        return;
    }
    let every = std::env::var("HUB_NUDGE_CHECK_MS")
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(Duration::from_secs(10), Duration::from_millis);
    let settle = std::env::var("HUB_NUDGE_SETTLE_MS")
        .ok()
        .and_then(|ms| ms.trim().parse().ok())
        .map_or(SETTLE, Duration::from_millis);
    let mut nudged = read_nudged();
    loop {
        std::thread::sleep(every);
        round(&mut nudged, every, settle, slack.as_deref());
        relay_round(&mut nudged, every, slack.as_deref());
    }
}
