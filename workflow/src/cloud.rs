//! Cloud runs: `workflow go <project> --cloud`, the prompt a cloud session
//! starts on, and the setup script of the claude.ai cloud environment
//! `workflow`.

use std::process::Command;

use crate::gitcmd::Git;
use crate::go::{self, Decision, Options};
use crate::memcli::{self, ProjectRow};
use crate::{embedded, exit, warn};

/// The prompt a cloud run starts on. The hub's Start in cloud card renders
/// the same file, so the two stay one text.
const PROMPT: &str = include_str!("cloud-prompt.txt");

/// claude.ai's Code page, where a cloud session lives. Built with `concat!`,
/// as the hygiene rule's own pattern is, so the rule keeps catching a pasted
/// session link while the code that makes one can still name the page.
pub const CODE_PAGE: &str = concat!("https://claude.ai", "/code");

/// A cloud run counts as live this long without a handoff or a tick: the
/// 6-hour stop and an hour more.
const LIVE_HOURS: i64 = 7;

/// The branch a cloud run of `slug` works on. A git branch may hold `_`, so
/// the project's name stays as mem spells it.
pub fn branch(project: &str, slug: &str) -> String {
    format!("cloud/{project}-{slug}")
}

/// The prompt for `slug` of `project`, its branch cut from `base`.
pub fn prompt(project: &str, slug: &str, base: &str) -> String {
    PROMPT
        .trim_end()
        .replace("{branch}", &branch(project, slug))
        .replace("{project}", project)
        .replace("{slug}", slug)
        .replace("{base}", base)
}

/// What the next cloud run is cut from: the branch of the cloud run that
/// landed last, which main may not hold yet, or main.
pub fn base(row: &ProjectRow) -> String {
    match &row.runner {
        Some(runner) if runner.machine == "cloud" => match &runner.milestone {
            Some(done) if row.milestone.as_ref() != Some(done) => branch(&row.name, done),
            _ => "main".to_string(),
        },
        _ => "main".to_string(),
    }
}

/// How many commits `dir`'s main holds that origin/main, as last fetched,
/// lacks: a cloud run clones origin, so it would start without them. `None`
/// when git cannot tell, as with no origin or no main.
pub fn unpushed(dir: &str) -> Option<u64> {
    let out = Git::at(dir).capture(&["rev-list", "--count", "origin/main..main"]);
    out.ok.then(|| out.text().trim().parse().ok()).flatten()
}

/// Where a cloud run stands, as the hub tells it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Running, as far as anyone can tell.
    Live,
    /// Its handoff says `parked:`: it waits for an answer.
    Parked,
    /// Its milestone is ticked.
    Landed,
    /// It wrote another handoff, or said nothing for 7 hours.
    Ended,
}

/// The state of `row`'s cloud run, or `None` when a device runs it or
/// nothing does. `handoff` is read only when one was written after the start.
pub fn state(
    row: &ProjectRow,
    handoff: impl FnOnce() -> Option<String>,
    now: jiff::Timestamp,
) -> Option<State> {
    let runner = row.runner.as_ref().filter(|r| r.machine == "cloud")?;
    let milestone = runner.milestone.as_ref()?;
    if row.milestone.as_ref() != Some(milestone) {
        return Some(State::Landed);
    }
    let time = |text: &Option<String>| text.as_deref()?.parse::<jiff::Timestamp>().ok();
    let since = time(&runner.since);
    // Seven hours from the start end it whatever it said: a 6-hour stop
    // parks with no question, and must not hold the one slot for ever.
    let within = since.is_some_and(|since| {
        now.duration_since(since) < jiff::SignedDuration::from_hours(LIVE_HOURS)
    });
    if let (Some(since), Some(at)) = (since, time(&row.handoff_at))
        && at > since
    {
        let parked = handoff().is_some_and(|text| text.trim_start().starts_with("parked:"));
        return Some(if parked && within {
            State::Parked
        } else {
            State::Ended
        });
    }
    Some(if within { State::Live } else { State::Ended })
}

/// `workflow go <project> --cloud`, or with `print` only its prompt.
pub fn cmd_go_cloud(opts: &Options, print: bool) -> i32 {
    let Some(projects) = memcli::projects() else {
        warn("go: mem cannot list its projects");
        return exit::FAILED;
    };
    let start = match go::decide(
        opts,
        &projects,
        memcli::project_current(),
        memcli::roadmap,
        memcli::handoff,
        || Ok(Vec::new()),
        None,
    ) {
        Decision::Start(start) => start,
        Decision::Done(project) => {
            println!("{project}: the roadmap is done");
            return exit::OK;
        }
        Decision::Paused(project) => {
            println!("{project}: paused");
            return exit::USAGE;
        }
        Decision::Refuse(why) => {
            warn(format!("go: {why}"));
            return exit::USAGE;
        }
        Decision::Fail(why) => {
            warn(format!("go: {why}"));
            return exit::FAILED;
        }
    };
    let Some(row) = projects.iter().find(|row| row.name == start.project) else {
        warn(format!("go: mem lists no {}", start.project));
        return exit::FAILED;
    };
    // A parked milestone may carry on locally while paused; a cloud run
    // never starts on a paused project.
    if row.paused.is_some() {
        println!("{}: paused", start.project);
        return exit::USAGE;
    }
    if let Some(why) = refusal(row, &start.slug, &start.dir) {
        warn(format!("go: {why}"));
        return exit::USAGE;
    }
    // One cloud run at a time, across every project; a parked one still
    // holds its session, which its answers go to.
    let now = jiff::Timestamp::now();
    let held = projects.iter().find(|row| {
        matches!(
            state(row, || memcli::handoff(&row.name), now),
            Some(State::Live | State::Parked)
        )
    });
    if let Some(other) = held {
        let runner = other.runner.as_ref().expect("a cloud state has a runner");
        warn(format!(
            "go: a cloud run is live: {} ({}, {}); one runs at a time",
            other.name,
            runner.milestone.as_deref().unwrap_or("?"),
            runner.id
        ));
        return exit::USAGE;
    }
    let base = base(row);
    let text = prompt(&start.project, &start.slug, &base);
    if print {
        println!("{text}");
        return exit::OK;
    }
    let (ok, said) = match launch(&start.dir, &text) {
        Launch::Ran(ok, said) => (ok, said),
        Launch::Untrusted => {
            warn(format!(
                "go: claude asks whether to trust {dir} and starts nothing until it is; \
run `claude` in {dir} once, answer Yes, I trust this folder, then go again",
                dir = start.dir
            ));
            return exit::FAILED;
        }
        Launch::Hung(said) => {
            warn(format!(
                "go: claude --cloud started no session within 2 minutes:\n{}",
                said.trim()
            ));
            return exit::FAILED;
        }
    };
    let Some(session) = session_of(&said) else {
        warn(format!(
            "go: claude --cloud started no session{}:\n{}",
            if ok { "" } else { " and failed" },
            said.trim()
        ));
        return exit::FAILED;
    };
    // claude's own notices, such as commits the cloud will not see.
    for line in said.lines().filter(|line| !line.contains(&session)) {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with("Created cloud session") {
            warn(line);
        }
    }
    if !memcli::set_cloud_runner(&start.project, &session, &start.slug) {
        warn(format!(
            "go: started {session}, but mem would not record it as the runner of {}",
            start.project
        ));
    } else if !memcli::sync() {
        // The session records itself too, on the hub's store, within the
        // minute; a record still only here then forks into a conflict copy.
        warn("go: no sync round carried the runner record to the hub; run `mem sync`");
    }
    println!("{CODE_PAGE}/{session}");
    exit::OK
}

/// Runs argv[1:] under a pty and copies what it prints. It stops the child
/// and exits 3 on claude's folder-trust prompt, which waits for a key nobody
/// will press, and exits 124 after two minutes of anything else.
const PTY: &str = r#"
import os, pty, re, select, sys, time
pid, fd = pty.fork()
if pid == 0:
    os.execvp(sys.argv[1], sys.argv[1:])
out, code, deadline = b"", None, time.time() + 120
while code is None:
    if time.time() > deadline:
        code = 124
        break
    ready, _, _ = select.select([fd], [], [], 0.5)
    if not ready:
        continue
    try:
        data = os.read(fd, 65536)
    except OSError:
        data = b""
    if not data:
        break
    out += data
    if b"trustthisfolder" in re.sub(rb"\x1b\[[0-9;?<>]*[a-zA-Z]|\s", b"", out):
        code = 3
sys.stdout.buffer.write(out)
sys.stdout.flush()
if code is not None:
    os.kill(pid, 9)
    os.waitpid(pid, 0)
    sys.exit(code)
_, status = os.waitpid(pid, 0)
sys.exit(os.waitstatus_to_exitcode(status))
"#;

/// Why `claude --cloud` started nothing, when it said why.
enum Launch {
    /// Its exit status was 0, and what it printed, escapes stripped.
    Ran(bool, String),
    /// It asked whether to trust the folder.
    Untrusted,
    /// It neither finished nor asked within two minutes.
    Hung(String),
}

/// Why `row`'s milestone `slug` cannot run in the cloud, if it cannot. The
/// session clones the repo's main and finds its project by the remote, so a
/// project working on a branch of its own, or with no remote, stays local;
/// work in `dir` that origin/main lacks waits for Saiful to push main; only
/// the next milestone runs, since a later one would read as landed; and a
/// device orchestrator keeps the project as a local go would.
fn refusal(row: &ProjectRow, slug: &str, dir: &str) -> Option<String> {
    let name = &row.name;
    if row.worktree.is_some() {
        return Some(format!(
            "{name} works on a branch of its own; cloud runs take main"
        ));
    }
    if row.remote.as_deref().is_none_or(str::is_empty) {
        return Some(format!("{name} has no git remote"));
    }
    if let Some(n) = unpushed(dir).filter(|n| *n > 0) {
        let commits = if n == 1 { "commit" } else { "commits" };
        return Some(format!(
            "{name}'s main is {n} {commits} ahead of origin/main; push main first: \
a cloud run starts from origin"
        ));
    }
    if row.milestone.as_deref() != Some(slug) {
        return Some(format!(
            "a cloud run takes {name}'s next milestone, {}, not {slug}",
            row.milestone.as_deref().unwrap_or("none")
        ));
    }
    let runner = row.runner.as_ref().filter(|r| r.machine != "cloud")?;
    if crate::machine::name().as_deref() != Some(runner.machine.as_str()) {
        return Some(format!(
            "{name} runs on {} ({}); start it there, or move it with \
`mem --project {name} project unset runner`",
            runner.machine, runner.id
        ));
    }
    match go::amx_agents() {
        Ok(rows) => go::running(&rows, name, slug, None)
            .map(|id| format!("{name} already has an orchestrator running: {id}")),
        Err(why) => Some(why),
    }
}

/// `claude --cloud <prompt>` in `dir`, under a pty: claude refuses `--cloud`
/// without a terminal.
fn launch(dir: &str, prompt: &str) -> Launch {
    let claude = match std::env::var("WORKFLOW_CLAUDE") {
        Ok(v) if !v.is_empty() => v,
        _ => "claude".to_string(),
    };
    let out = Command::new("python3")
        .args(["-c", PTY, &claude, "--cloud", prompt])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(out) => {
            let said = plain(&format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ));
            match out.status.code() {
                Some(3) => Launch::Untrusted,
                Some(124) => Launch::Hung(said),
                _ => Launch::Ran(out.status.success(), said),
            }
        }
        Err(err) => Launch::Ran(false, format!("cannot run python3 for a pty: {err}")),
    }
}

/// Terminal output as text: escape sequences and carriage returns dropped.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final letter.
                Some('[') => {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// The session id in claude's `View: <CODE_PAGE>/session_...` line.
fn session_of(said: &str) -> Option<String> {
    let page = format!("{CODE_PAGE}/");
    let at = said.find(&format!("{page}session_"))? + page.len();
    let id: String = said[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (id.len() > "session_".len()).then_some(id)
}

/// The heredoc delimiter; no embedded file may hold it as a line of its own.
const END: &str = "WORKFLOW_EOF";

/// The setup script: every file of `skills/` and `agents/` under
/// `~/.claude`, and a `mem` that fetches the real binary from the mem server
/// the first time it runs. The script runs before the session, where the
/// proxy adds no Access token, so the binary cannot come with it.
pub fn setup_script(files: &[(&str, &[u8])]) -> String {
    let mut out = String::from(
        "#!/bin/bash\n\
# The cloud environment `workflow`'s setup script, printed by `workflow cloud-setup`.\n\
set -euo pipefail\n",
    );
    for (rel, bytes) in files {
        if !(rel.starts_with("skills/") || rel.starts_with("agents/")) {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        let dest = format!("\"$HOME/.claude/{rel}\"");
        out.push_str(&format!("mkdir -p \"$(dirname {dest})\"\n"));
        out.push_str(&format!("cat > {dest} <<'{END}'\n{text}{END}\n"));
    }
    out.push_str(&format!(
        "bin=/usr/local/bin\n\
mkdir -p \"$bin\"\n\
cat > \"$bin/mem\" <<'{END}'\n{SHIM}{END}\n\
chmod +x \"$bin/mem\"\n"
    ));
    out
}

/// The `mem` a cloud session runs: the x86_64 binary from `$MEM_REMOTE/dl/mem`,
/// fetched inside the session, where the proxy adds the Access token.
///
/// With `TS_AUTHKEY` and `HUB_PEER_TOKEN` among the environment's secrets, the
/// first mem of a session also joins the tailnet as an ephemeral `tag:cloud`
/// machine (Tailscale in userspace, since the VM has no tun device) and starts
/// the local hub from `$MEM_REMOTE/dl/hub`, in the background. The key goes
/// to tailscale in a file that is removed once read, and no process started
/// here keeps it in its environment.
const SHIM: &str = r#"#!/bin/sh
real="$HOME/.local/bin/mem-x86_64"
if [ ! -x "$real" ]; then
  mkdir -p "$HOME/.local/bin"
  if ! curl -fsSL --max-time 120 "${MEM_REMOTE:?MEM_REMOTE is not set}/dl/mem" -o "$real.part"; then
    echo "mem: cannot fetch the mem binary from $MEM_REMOTE/dl/mem" >&2
    exit 1
  fi
  chmod +x "$real.part"
  mv "$real.part" "$real"
fi
state="$HOME/.local/state/hub"
# Alive when the local hub answers on mem's socket.
hub_alive() { curl -s -m 2 --unix-socket "$state/local.sock" http://local/ping >/dev/null 2>&1; }
if [ -n "${TS_AUTHKEY:-}" ] && [ -n "${HUB_PEER_TOKEN:-}" ] && mkdir -p "$state" && ! hub_alive; then
  # One start at a time; a later mem finds a dead hub and starts it again.
  (
    # The daemons started below close fd 9, so the lock ends with this start.
    flock -n 9 || exit 0
    bin="$HOME/.local/bin"
    ts=1.98.8
    sock="$state/tailscaled.sock"
    set -e
    if [ ! -x "$bin/tailscaled" ]; then
      curl -fsSL --max-time 120 "https://pkgs.tailscale.com/stable/tailscale_${ts}_amd64.tgz" -o "$state/ts.tgz"
      tar xzf "$state/ts.tgz" -C "$state"
      cp "$state/tailscale_${ts}_amd64/tailscale" "$state/tailscale_${ts}_amd64/tailscaled" "$bin/"
    fi
    if [ ! -x "$bin/hub-x86_64" ]; then
      curl -fsSL --max-time 120 "$MEM_REMOTE/dl/hub" -o "$bin/hub-x86_64.part"
      chmod +x "$bin/hub-x86_64.part"
      mv "$bin/hub-x86_64.part" "$bin/hub-x86_64"
    fi
    if ! env -u TS_AUTHKEY "$bin/tailscale" --socket="$sock" status >/dev/null 2>&1; then
      env -u TS_AUTHKEY "$bin/tailscaled" --tun=userspace-networking \
        --socks5-server=localhost:1055 --state=mem: --socket="$sock" >"$state/tailscaled.log" 2>&1 9>&- &
      i=0
      while [ ! -e "$sock" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
      key="$state/ts-authkey"
      (umask 077 && printf '%s' "$TS_AUTHKEY" >"$key")
      joined=0
      # The tag is asked for, so a key that cannot give it fails here rather
      # than joining with its owner's rights.
      env -u TS_AUTHKEY "$bin/tailscale" --socket="$sock" up --auth-key="file:$key" \
        --hostname="cloud-$(hostname | tr -cd 'a-z0-9-' | cut -c1-20)" --advertise-tags=tag:cloud || joined=$?
      rm -f "$key"
      [ "$joined" = 0 ]
    fi
    # The name Tailscale gave this machine, which may not be the one asked
    # for: it is what a Slack button carries back.
    host=$(env -u TS_AUTHKEY "$bin/tailscale" --socket="$sock" status --json --peers=false |
      tr -d '\n' | sed -n 's/.*"Self": *{[^}]*"DNSName": *"\([^"]*\)".*/\1/p' | sed 's/\.$//')
    [ -n "$host" ]
    env -u TS_AUTHKEY HUB_SOCKS5=127.0.0.1:1055 HUB_HOST="$host" \
      nohup "$bin/hub-x86_64" local >"$state/local.log" 2>&1 9>&- &
  ) 9>"$state/start.lock" >"$state/start.log" 2>&1 </dev/null &
  # A question goes to the local hub the moment mem writes it, so an ask
  # waits for the hub it just started.
  if [ "${1:-}" = ask ]; then
    i=0
    while ! hub_alive && [ $i -lt 60 ]; do sleep 0.5; i=$((i + 1)); done
  fi
fi
exec "$real" "$@"
"#;

pub fn cmd_cloud_setup() -> i32 {
    print!("{}", setup_script(embedded::FILES));
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_embedded_file_ends_its_heredoc_early_or_lacks_its_last_newline() {
        for (rel, bytes) in embedded::FILES {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.lines().any(|line| line == END), "{rel}");
            assert!(text.ends_with('\n'), "{rel} must end with a newline");
        }
    }

    #[test]
    fn the_session_comes_out_of_the_view_line_whatever_the_terminal_wrapped_round_it() {
        let said = plain(&format!(
            "\x1b[>4m\x1b[<uCreated cloud session: x\r\nView: {CODE_PAGE}/session_0117trsKBKz?from=cli&m=0\r\n\x1b]8;;\x07",
        ));
        assert_eq!(session_of(&said).as_deref(), Some("session_0117trsKBKz"));
        assert_eq!(session_of(&format!("View: {CODE_PAGE}/session_")), None);
    }

    /// Tailscale drops an offline ephemeral machine only after 30 to 60
    /// minutes; a landed run leaves the tailnet itself.
    #[test]
    fn a_landed_run_logs_out_of_the_tailnet_last() {
        assert!(PROMPT.contains("tailscaled.sock\" logout"), "{PROMPT}");
        assert!(PROMPT.contains("not when parked"), "{PROMPT}");
    }

    #[test]
    fn hooks_stay_out_of_the_cloud() {
        let script = setup_script(&[("hooks/pre-commit", b"x\n"), ("skills/a/SKILL.md", b"a\n")]);
        assert!(!script.contains("pre-commit"), "{script}");
        assert!(script.contains(".claude/skills/a/SKILL.md"), "{script}");
    }
}
