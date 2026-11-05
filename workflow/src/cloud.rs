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

/// The prompt a cloud plan starts on.
const PLAN_PROMPT: &str = include_str!("cloud-plan-prompt.txt");

/// The milestone a cloud plan records as its runner's: no roadmap names it,
/// so the run is live until its handoff rather than landed at once, and the
/// first cloud milestone after it is cut from its branch.
pub const PLAN: &str = "plan";

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

/// The prompt for a cloud plan of `project`.
pub fn plan_prompt(project: &str) -> String {
    PLAN_PROMPT
        .trim_end()
        .replace("{branch}", &branch(project, PLAN))
        .replace("{project}", project)
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
    /// Its handoff says `parked: local-only`: a task only a device can do
    /// waits, and `workflow go` there takes the run over from its branch.
    LocalOnly,
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
    if milestone != PLAN && row.milestone.as_ref() != Some(milestone) {
        return Some(State::Landed);
    }
    let time = |text: &Option<String>| text.as_deref()?.parse::<jiff::Timestamp>().ok();
    let since = time(&runner.since);
    // Seven hours from the start end it whatever it said: a 6-hour stop
    // parks with no question, and must not hold a cloud slot for ever.
    let within = since.is_some_and(|since| {
        now.duration_since(since) < jiff::SignedDuration::from_hours(LIVE_HOURS)
    });
    if let (Some(since), Some(at)) = (since, time(&row.handoff_at))
        && at > since
    {
        let text = handoff().unwrap_or_default();
        let text = text.trim_start();
        return Some(if text.starts_with("parked: local-only") {
            State::LocalOnly
        } else if text.starts_with("parked:") && within {
            State::Parked
        } else {
            State::Ended
        });
    }
    Some(if within { State::Live } else { State::Ended })
}

/// `git push origin main` in `dir`, past the push hook, which lets a push
/// through only with this variable: the project's cloud key is the yes.
fn push_main(dir: &str) -> Result<(), String> {
    let out = Command::new("git")
        .args(["push", "-q", "origin", "main"])
        .env("WORKFLOW_ALLOW_PUSH", "1")
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `workflow go <project> --cloud`, or with `print` only its prompt.
pub fn cmd_go_cloud(opts: &Options, print: bool) -> i32 {
    let _slot = if print { None } else { slot_lock() };
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
    // A project carried to the cloud pushes its own main: the run starts
    // from origin, and a device has just landed the milestone before it
    // there. Any other unpushed main stays Saiful's to push.
    let ahead = unpushed(&start.dir).filter(|n| *n > 0);
    let carried = row.cloud.as_deref().is_some_and(|m| !m.is_empty());
    if let (true, Some(n)) = (carried, ahead) {
        if print {
            let commits = if n == 1 { "commit" } else { "commits" };
            warn(format!(
                "go: would push {n} {commits} of {}'s main first",
                row.name
            ));
        } else if let Err(why) = push_main(&start.dir) {
            warn(format!("go: cannot push {}'s main: {why}", row.name));
            return exit::FAILED;
        }
    }
    // Printing for a carried project, the push it would make stands in for
    // the unpushed main a start refuses.
    if let Some(why) = refusal(row, &start.slug, &start.dir, print && carried) {
        warn(format!("go: {why}"));
        return exit::USAGE;
    }
    if let Some(why) = held_elsewhere(&projects, &start.project) {
        warn(format!("go: {why}"));
        return exit::USAGE;
    }
    let base = base(row);
    let text = prompt(&start.project, &start.slug, &base);
    if print {
        println!("{text}");
        return exit::OK;
    }
    start_session("go", &start.project, &start.dir, &text, &start.slug)
}

/// Whether a plain `workflow go` for `opts` starts in the cloud: the project
/// names a machine that carries it there (`mem project set cloud`), and no
/// cloud run of it is parked local-only, which a device takes itself.
pub fn carried(opts: &Options) -> bool {
    let Some(projects) = memcli::projects() else {
        return false;
    };
    let name = match opts.project {
        Some(project) => project.to_string(),
        None => match memcli::project_current() {
            Some(current) => current.name,
            None => return false,
        },
    };
    let Some(row) = projects.iter().find(|row| row.name == name) else {
        return false;
    };
    if row.cloud.as_deref().is_none_or(str::is_empty) {
        return false;
    }
    !matches!(
        state(row, || memcli::handoff(&row.name), jiff::Timestamp::now()),
        Some(State::LocalOnly)
    )
}

/// `workflow plan <project> --cloud`, or with `print` only its prompt.
pub fn cmd_plan_cloud(project: Option<&str>, print: bool) -> i32 {
    let _slot = if print { None } else { slot_lock() };
    let Some(projects) = memcli::projects() else {
        warn("plan: mem cannot list its projects");
        return exit::FAILED;
    };
    let current = memcli::project_current();
    let name = match project
        .map(str::to_string)
        .or_else(|| current.as_ref().map(|p| p.name.clone()))
    {
        Some(name) => name,
        None => {
            warn("plan: name a project, or run it in a project's checkout");
            return exit::USAGE;
        }
    };
    let Some(row) = projects.iter().find(|row| row.name == name) else {
        warn(format!("plan: mem lists no {name}"));
        return exit::USAGE;
    };
    let here = current.filter(|p| p.name == name).and_then(|p| p.root);
    let listed = row
        .checkouts
        .iter()
        .find(|dir| std::path::Path::new(dir).is_dir())
        .cloned();
    let Some(dir) = here.or(listed) else {
        warn(format!("plan: {name} has no checkout on this machine"));
        return exit::USAGE;
    };
    if row.paused.is_some() {
        println!("{name}: paused");
        return exit::USAGE;
    }
    if let Some(why) = repo_refusal(row, &dir, false) {
        warn(format!("plan: {why}"));
        return exit::USAGE;
    }
    if let Some(why) = held_elsewhere(&projects, &name) {
        warn(format!("plan: {why}"));
        return exit::USAGE;
    }
    let text = plan_prompt(&name);
    if print {
        println!("{text}");
        return exit::OK;
    }
    start_session("plan", &name, &dir, &text, PLAN)
}

/// How many cloud runs may be live or parked at once, across every
/// project, as Saiful set it on 2026-11-05.
const SLOTS: usize = 2;

/// Why `project` may not take a cloud slot, if it may not: its own cloud
/// run is live or parked, or two others are. A parked run still holds its
/// session, which its answers go to.
fn held_elsewhere(projects: &[ProjectRow], project: &str) -> Option<String> {
    let now = jiff::Timestamp::now();
    let held: Vec<&ProjectRow> = projects
        .iter()
        .filter(|row| {
            matches!(
                state(row, || memcli::handoff(&row.name), now),
                Some(State::Live | State::Parked)
            )
        })
        .collect();
    let named = |row: &ProjectRow| {
        let runner = row.runner.as_ref().expect("a cloud state has a runner");
        (
            runner.milestone.as_deref().unwrap_or("?").to_string(),
            runner.id.clone(),
        )
    };
    if let Some(own) = held.iter().find(|row| row.name == project) {
        let (milestone, id) = named(own);
        return Some(format!(
            "{project} already has a cloud run: {milestone}, {id}"
        ));
    }
    if held.len() < SLOTS {
        return None;
    }
    let runs: Vec<String> = held
        .iter()
        .map(|row| {
            let (milestone, id) = named(row);
            format!("{} ({milestone}, {id})", row.name)
        })
        .collect();
    Some(format!(
        "two cloud runs are live: {}; two run at a time",
        runs.join(" and ")
    ))
}

/// Held from the slot check to the runner record, so two starts on this
/// machine cannot both take the last slot: the second reads the projects
/// once the first has recorded its run. `None`, said on stderr, when the lock
/// file cannot be had, and the start goes on unguarded.
fn slot_lock() -> Option<std::fs::File> {
    let lock = || {
        let state = match std::env::var_os("XDG_STATE_HOME") {
            Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir),
            _ => std::path::PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
        };
        let dir = state.join("workflow");
        std::fs::create_dir_all(&dir).ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("cloud-slots.lock"))
            .ok()?;
        file.lock().ok()?;
        Some(file)
    };
    let file = lock();
    if file.is_none() {
        warn(
            "go: cannot lock the cloud slots; a start at the same moment could take the last one too",
        );
    }
    file
}

/// Starts `text` in a cloud session from `dir`, records it as `project`'s
/// cloud runner of `slug` and prints its link. `verb` names the command in
/// what it says.
fn start_session(verb: &str, project: &str, dir: &str, text: &str, slug: &str) -> i32 {
    let (ok, said) = match launch(dir, text) {
        Launch::Ran(ok, said) => (ok, said),
        Launch::Untrusted => {
            warn(format!(
                "{verb}: claude asks whether to trust {dir} and starts nothing until it is; \
run `claude` in {dir} once, answer Yes, I trust this folder, then {verb} again"
            ));
            return exit::FAILED;
        }
        Launch::Hung(said) => {
            warn(format!(
                "{verb}: claude --cloud started no session within 2 minutes:\n{}",
                said.trim()
            ));
            return exit::FAILED;
        }
    };
    let Some(session) = session_of(&said) else {
        warn(format!(
            "{verb}: claude --cloud started no session{}:\n{}",
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
    if !memcli::set_cloud_runner(project, &session, slug) {
        warn(format!(
            "{verb}: started {session}, but mem would not record it as the runner of {project}"
        ));
    } else if !memcli::sync() {
        // The session records itself too, on the hub's store, within the
        // minute; a record still only here then forks into a conflict copy.
        warn(format!(
            "{verb}: no sync round carried the runner record to the hub; run `mem sync`"
        ));
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
fn refusal(row: &ProjectRow, slug: &str, dir: &str, pushes: bool) -> Option<String> {
    if let Some(why) = repo_refusal(row, dir, pushes) {
        return Some(why);
    }
    let name = &row.name;
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

/// Why `row` cannot run in the cloud at all, whatever it runs: a cloud
/// session clones origin's main and finds the project by its remote.
/// `pushes` says the caller pushes main itself, so an unpushed main is no
/// reason.
fn repo_refusal(row: &ProjectRow, dir: &str, pushes: bool) -> Option<String> {
    let name = &row.name;
    if row.worktree.is_some() {
        return Some(format!(
            "{name} works on a branch of its own; cloud runs take main"
        ));
    }
    if row.remote.as_deref().is_none_or(str::is_empty) {
        return Some(format!("{name} has no git remote"));
    }
    if let Some(n) = unpushed(dir).filter(|n| *n > 0 && !pushes) {
        let commits = if n == 1 { "commit" } else { "commits" };
        return Some(format!(
            "{name}'s main is {n} {commits} ahead of origin/main; push main first: \
a cloud run starts from origin"
        ));
    }
    None
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

/// The setup script: the cloud kit's rules and settings, the skills and
/// agents from the public release's bundle (too big for the setup field
/// inline), and a `mem` that fetches the real binary from the public release
/// (from the mem server without the store's variables) the first time it
/// runs. claude.ai runs the script with none of the environment's
/// variables, so it cannot reach the store: it bakes both binaries into the
/// cached environment from the release the workflow repo's CI publishes, and
/// each session fills its replica from the store.
pub fn setup_script(files: &[(&str, &[u8])]) -> String {
    let mut out = String::from(
        "#!/bin/bash\n\
# The cloud environment `workflow`'s setup script, printed by `workflow cloud-setup`.\n\
set -euo pipefail\n",
    );
    // The cloud kit's hooks and settings, merged once the helper that merges
    // them exists.
    let mut kit_hooks = None;
    let mut kit_keys = None;
    for (rel, bytes) in files {
        let text = String::from_utf8_lossy(bytes);
        // The cloud kit: its rules are the user's CLAUDE.md, its hooks and
        // settings go into settings.json. Skills and agents come with the
        // release's bundle.
        let target = if *rel == "cloud/rules.md" {
            "CLAUDE.md".to_string()
        } else if *rel == "cloud/settings-hooks.json" {
            kit_hooks = Some(text.into_owned());
            continue;
        } else if *rel == "cloud/settings-keys.json" {
            kit_keys = Some(text.into_owned());
            continue;
        } else {
            continue;
        };
        let dest = format!("\"$HOME/.claude/{target}\"");
        out.push_str(&format!("mkdir -p \"$(dirname {dest})\"\n"));
        out.push_str(&format!("cat > {dest} <<'{END}'\n{text}{END}\n"));
    }
    let hooks = match kit_hooks {
        Some(text) => format!(
            "kit=$(mktemp)\n\
keys=$(mktemp)\n\
cat > \"$kit\" <<'{END}'\n{text}{END}\n\
cat > \"$keys\" <<'{END}'\n{}{END}\n\
\"$bin/mem-cloud\" hooks \"$kit\" \"$keys\"\n\
rm -f \"$kit\" \"$keys\"\n",
            kit_keys.as_deref().unwrap_or("{}\n")
        ),
        None => String::new(),
    };
    out.push_str(&format!(
        "bin=/usr/local/bin\n\
mkdir -p \"$bin\"\n\
cat > \"$bin/mem-cloud\" <<'{END}'\n{HELPER}{END}\n\
chmod +x \"$bin/mem-cloud\"\n\
cat > \"$bin/mem\" <<'{END}'\n{SHIM}{END}\n\
chmod +x \"$bin/mem\"\n\
cat > \"$bin/workflow\" <<'{END}'\n{WORKFLOW_SHIM}{END}\n\
chmod +x \"$bin/workflow\"\n\
{hooks}\"$bin/mem-cloud\" bundle\n\
if [ -s \"$HOME/.local/state/hub/skills.sha256\" ]; then\n\
  echo \"setup: took the skills and agents from the cloud-bins release\"\n\
else\n\
  echo \"setup: no skills bundle from the release; the first mem of a boot fetches it\"\n\
fi\n\
# The image runs the setup as root; elsewhere the runtimes are the machine's.\n\
if [ \"$(id -u)\" = 0 ]; then\n\
  \"$bin/mem-cloud\" runtimes || echo \"setup: a runtime did not install; see above\"\n\
fi\n\
baked=0\n\
\"$bin/mem-cloud\" bake || baked=$?\n\
case $baked in\n\
  0) echo \"setup: baked mem, hub and workflow from the cloud-bins release\" ;;\n\
  3) echo \"setup: baked mem and hub; no workflow build on cloud-bins, so the guard and worktree hooks pass every call\" ;;\n\
  *) echo \"setup: no baked binaries: the release did not answer; each session fetches its own\" ;;\n\
esac\n"
    ));
    out
}

/// `mem-cloud fetch <mem|hub|workflow>` puts the public `cloud-bins` release's
/// `<name>-x86_64` at `~/.local/bin/<name>-x86_64` once, when `HUB_STORE_URL`
/// and `HUB_STORE_TOKEN` are set, with no header, since the release needs
/// none; an environment without those two still fetches
/// `$MEM_REMOTE/dl/<name>`, the Access service token, when the variables
/// hold one, going as a header read from a file, never on a command line.
/// Once a boot, with the store's variables, a binary already here is checked
/// against the release's `<name>-x86_64.sha256` and fetched again when the
/// release holds another build, so a new build reaches the cached
/// environment without a new setup; a release that gives no sum keeps the
/// binary here, and is not asked again that boot. `mem-cloud sync <seconds>`
/// brings the replica up to the store with `hub share sync`, and fails when
/// the store cannot be reached in time. `mem-cloud bake` takes mem, hub and
/// workflow from the public `cloud-bins` release, which needs no secret: the
/// setup has none. It exits 3 when only workflow did not come.
const HELPER: &str = r#"#!/bin/sh
bin="$HOME/.local/bin"
state="$HOME/.local/state/hub"
# The public release the workflow repo's CI puts each build, its sum and the
# skills on.
release=https://github.com/saifulapm/workflow/releases/download/cloud-bins
# headers <file>: the Access service token, in a file only this user reads.
headers() {
  (umask 077 && : >"$1")
  if [ -n "${HUB_STORE_ACCESS_ID:-}" ] && [ -n "${HUB_STORE_ACCESS_SECRET:-}" ]; then
    printf 'CF-Access-Client-Id: %s\nCF-Access-Client-Secret: %s\n' \
      "$HUB_STORE_ACCESS_ID" "$HUB_STORE_ACCESS_SECRET" >>"$1"
  fi
}
fetch() {
  mark=
  if [ -x "$bin/$1-x86_64" ]; then
    [ -n "${HUB_STORE_URL:-}" ] && [ -n "${HUB_STORE_TOKEN:-}" ] || return 0
    boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null) || return 0
    mark="$state/fresh-$1-$boot"
    [ -e "$mark" ] && return 0
  fi
  mkdir -p "$bin" "$state"
  part="$bin/$1-x86_64.part.$$"
  hdr="$state/fetch.hdr.$$"
  head=
  if [ -n "${HUB_STORE_URL:-}" ] && [ -n "${HUB_STORE_TOKEN:-}" ]; then
    # The release is public: no header goes to GitHub.
    url="$release/$1-x86_64"
  else
    url="${MEM_REMOTE:?MEM_REMOTE is not set}/dl/$1"
    headers "$hdr"
    head="@$hdr"
  fi
  if [ -n "$mark" ]; then
    : >"$mark"
    want=$(curl -fsSL --max-time 10 ${head:+-H "$head"} "$url.sha256" 2>/dev/null) &&
      [ "${want%% *}" != "$(sha256sum "$bin/$1-x86_64" | cut -d' ' -f1)" ] ||
      { rm -f "$hdr"; return 0; }
  fi
  curl -fsSL --max-time 120 ${head:+-H "$head"} "$url" -o "$part"
  got=$?
  rm -f "$hdr"
  if [ "$got" != 0 ]; then
    rm -f "$part"
    # A new build that did not come keeps the one here working.
    [ -n "$mark" ] && return 0
    echo "mem: cannot fetch the $1 binary from $url" >&2
    return 1
  fi
  chmod +x "$part"
  mv "$part" "$bin/$1-x86_64"
}
# Once a boot, with the store's variables: the workflow skills and agents the
# release holds (the workflow repo's CI packs them) replace the folders of the same name
# under ~/.claude, so a changed skill reaches cloud sessions with no new
# setup; Claude Code picks up a changed SKILL.md in a running session. One
# the setup or the last bundle wrote that this bundle leaves out goes.
skills() {
  [ -n "${HUB_STORE_URL:-}" ] && [ -n "${HUB_STORE_TOKEN:-}" ] || return 0
  boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null) || return 0
  [ -e "$state/fresh-skills-$boot" ] && return 0
  mkdir -p "$state" && : >"$state/fresh-skills-$boot"
  bundle
}
# bundle: the release's skills bundle, put in place when its sum differs
# from the one here. The setup runs it with no secret; the release is public.
bundle() {
  mkdir -p "$state"
  url="$release/workflow-skills.tar.gz"
  want=$(curl -fsSL --max-time 10 "$url.sha256" 2>/dev/null) &&
    want=${want%% *} && [ "$want" != "$(cat "$state/skills.sha256" 2>/dev/null)" ] &&
    curl -fsSL --max-time 60 "$url" -o "$state/skills.tar.gz" || return 0
  [ "$(sha256sum "$state/skills.tar.gz" | cut -d' ' -f1)" = "$want" ] || return 0
  new="$state/skills.new"
  rm -rf "$new" && mkdir -p "$new" && tar -xzf "$state/skills.tar.gz" -C "$new" || return 0
  # The bundle is the whole set: a name the last one had (or the setup wrote)
  # that it lacks was removed from the repo. Only skills/<name> and
  # agents/<file> lines are ever removed.
  # The cloud kit: its skills join the others, its rules and hooks go where
  # Claude Code reads them.
  if [ -d "$new/cloud/skills" ]; then
    mkdir -p "$new/skills"
    for item in "$new/cloud/skills"/*; do
      [ -e "$item" ] && rm -rf "$new/skills/${item##*/}" && mv "$item" "$new/skills/"
    done
  fi
  [ -s "$new/cloud/rules.md" ] && mkdir -p "$HOME/.claude" && cp "$new/cloud/rules.md" "$HOME/.claude/CLAUDE.md"
  hooks "$new/cloud/settings-hooks.json" "$new/cloud/settings-keys.json"
  ( cd "$new" && for item in skills/* agents/*; do [ -e "$item" ] && echo "$item"; done ) >"$new.list"
  while IFS= read -r name; do
    case "$name" in
      */*/* | *..* | "") continue ;;
      skills/?* | agents/?*) grep -qxF "$name" "$new.list" || rm -rf "$HOME/.claude/$name" ;;
    esac
  done <"$state/skills.list" 2>/dev/null
  for kind in skills agents; do
    [ -d "$new/$kind" ] || continue
    mkdir -p "$HOME/.claude/$kind"
    for item in "$new/$kind"/*; do
      [ -e "$item" ] || continue
      rm -rf "$HOME/.claude/$kind/${item##*/}"
      mv "$item" "$HOME/.claude/$kind/"
    done
  done
  mv "$new.list" "$state/skills.list"
  rm -rf "$new" "$state/skills.tar.gz"
  printf '%s\n' "$want" >"$state/skills.sha256"
}
# hooks <file> [<keys>]: merge the cloud kit's hooks into
# ~/.claude/settings.json, each event the kit names taking the kit's entries,
# and its settings (the advisor) at the top, keeping whatever else the
# image's settings hold. A second merge changes nothing.
hooks() {
  [ -s "$1" ] || [ -s "${2:-}" ] || return 0
  mkdir -p "$HOME/.claude"
  python3 - "$HOME/.claude/settings.json" "$1" "${2:-}" <<'PY_EOF' || echo "mem-cloud: could not merge the cloud kit into settings.json" >&2
import json, os, sys
path, kit, keys = sys.argv[1], sys.argv[2], sys.argv[3]
settings = {}
if os.path.exists(path):
    try:
        settings = json.load(open(path))
    except ValueError:
        settings = {}
if os.path.exists(kit):
    settings.setdefault("hooks", {}).update(json.load(open(kit)))
# Keys that are no JSON object are passed over: the hooks still go in.
extra = {}
if keys and os.path.exists(keys):
    try:
        extra = json.load(open(keys))
    except ValueError:
        extra = {}
if isinstance(extra, dict):
    settings.update({k: v for k, v in extra.items() if k != "hooks"})
with open(path, "w") as out:
    json.dump(settings, out, indent=2)
PY_EOF
}
bake() {
  mkdir -p "$bin"
  for name in mem hub workflow; do
    [ -x "$bin/$name-x86_64" ] && continue
    part="$bin/$name-x86_64.part.$$"
    if ! curl -fsSL --max-time 120 "$release/$name-x86_64" -o "$part"; then
      rm -f "$part"
      # mem and hub are baked by now; without workflow the guard passes.
      [ "$name" = workflow ] && return 3
      return 1
    fi
    chmod +x "$part"
    mv "$part" "$bin/$name-x86_64"
  done
}
# runtimes: the devices' Node 24, pnpm 11, PHP 8.4 and playwright-cli, linked
# into ~/.local/bin, which comes before the image's /opt/node22/bin on a
# session's PATH. Each install is skipped when the cached environment already
# has it.
runtimes() {
  node="${MEM_CLOUD_ROOT:-}/opt/node24/bin"
  failed=
  mkdir -p "$bin"
  if [ -x "$node/node" ]; then
    case "$("$node/pnpm" -v 2>/dev/null)" in
      11.*) ;;
      # npm trusts the system's CA store, which holds the network proxy's.
      *) "$node/node" --use-system-ca "$node/npm" install -g --force pnpm@11 >&2 || failed="$failed pnpm" ;;
    esac
    [ "$("$node/playwright-cli" --version 2>/dev/null)" = 0.1.23 ] ||
      "$node/node" --use-system-ca "$node/npm" install -g @playwright/cli@0.1.23 >&2 ||
      failed="$failed playwright-cli"
    for tool in node npm npx pnpm pnpx; do
      [ -e "$node/$tool" ] && ln -sfn "$node/$tool" "$bin/$tool"
    done
    # Headless on the image's Chrome, with no sandbox: the session is root.
    printf '%s\n' '#!/bin/sh' \
      'export PLAYWRIGHT_MCP_EXECUTABLE_PATH="${PLAYWRIGHT_MCP_EXECUTABLE_PATH:-${CHROME_PATH:-/usr/bin/google-chrome}}"' \
      'export PLAYWRIGHT_MCP_SANDBOX="${PLAYWRIGHT_MCP_SANDBOX:-false}"' \
      "exec \"$node/playwright-cli\" \"\$@\"" >"$bin/playwright-cli"
    chmod +x "$bin/playwright-cli"
  else
    echo "setup: the image has no Node 24 at /opt/node24; Node stays the image's" >&2
  fi
  if ! command -v php8.4 >/dev/null 2>&1; then
    apt-get update -q >&2
    command -v add-apt-repository >/dev/null 2>&1 ||
      DEBIAN_FRONTEND=noninteractive apt-get install -y -q software-properties-common >&2
    { add-apt-repository -y ppa:ondrej/php >&2 &&
      DEBIAN_FRONTEND=noninteractive apt-get install -y -q php8.4-cli php8.4-intl \
        php8.4-mbstring php8.4-xml php8.4-curl php8.4-zip php8.4-sqlite3 \
        php8.4-mysql php8.4-pgsql php8.4-bcmath php8.4-gd >&2; } || failed="$failed php8.4"
  fi
  php=$(command -v php8.4) && ln -sfn "$php" "$bin/php"
  [ -z "$failed" ] || { echo "setup: did not install:$failed" >&2; return 1; }
}
case "${1:-}" in
  fetch) fetch "$2" ;;
  runtimes) runtimes ;;
  bake) bake ;;
  skills) skills ;;
  bundle) bundle ;;
  hooks) hooks "$2" "${3:-}" ;;
  # The binaries come from the release; the hub then works on the replica.
  sync) fetch mem && fetch hub && env -u MEM_REMOTE timeout "$2" "$bin/hub-x86_64" share sync ;;
  *) echo "mem-cloud: fetch <mem|hub|workflow>, sync <seconds>, bake, runtimes, skills, bundle or hooks <file> [<keys>]" >&2; exit 2 ;;
esac
"#;

/// The `workflow` the guard hook calls by name: the binary the setup baked
/// or a store session's mem fetched. With none, it fails, and the hook lets
/// every call run.
const WORKFLOW_SHIM: &str = r#"#!/bin/sh
real="$HOME/.local/bin/workflow-x86_64"
[ -x "$real" ] || exit 1
exec "$real" "$@"
"#;

/// The `mem` a cloud session runs: the x86_64 binary `mem-cloud fetch mem`
/// takes from the release, or from `$MEM_REMOTE/dl/mem` without the store's
/// variables, fetched inside the session, where the proxy adds the Access
/// token.
///
/// With `HUB_STORE_URL` and `HUB_STORE_TOKEN` among the environment's
/// variables, mem keeps a replica of the store like a device's:
/// the first mem of a session brings it up to the store before it reads,
/// and a store that cannot be reached leaves the replica baked at setup;
/// then the local hub holds the socket. Without them, mem runs on the mem
/// server, as before the move.
///
/// With `TS_AUTHKEY` and a peer token (`HUB_PEER_TOKEN`, or a cloud
/// session's `HUB_CLOUD_TOKEN`) as well, the first mem of a session also
/// joins the tailnet as an ephemeral `tag:cloud` machine (Tailscale in
/// userspace, since the VM has no tun device). The local hub starts from
/// the binary `mem-cloud fetch hub` takes, in the background. The key goes to tailscale in a
/// file that is removed once read, and no process started here keeps it in
/// its environment.
const SHIM: &str = r#"#!/bin/sh
helper="$(dirname "$0")/mem-cloud"
real="$HOME/.local/bin/mem-x86_64"
# The store ties a cloud session's writes to its token by mem's machine name.
if [ ! -s "$HOME/.config/qshell/machine" ]; then
  vm=$(hostname | tr 'A-Z' 'a-z' | tr -cd 'a-z0-9-' | cut -c1-20)
  mkdir -p "$HOME/.config/qshell" &&
    echo "cloud-${vm:-vm}" > "$HOME/.config/qshell/machine" || true
fi
"$helper" fetch mem || exit 1
store=
if [ -n "${HUB_STORE_URL:-}" ] && [ -n "${HUB_STORE_TOKEN:-}" ]; then store=1; fi
# The guard hook's workflow, checked against the release once a boot as mem
# is. A workflow that cannot be had leaves the hook passing every call.
if [ -n "$store" ]; then "$helper" fetch workflow >/dev/null 2>&1 || true; fi
# Every machine shares through the store since the switch, and mem queues a
# write only on a switched replica: without the flag, nothing a session
# saves would leave the VM. The hub's own mem calls come here too, so the
# setup's bake switches the replica it fills.
switched="${XDG_STATE_HOME:-$HOME/.local/state}/mem/share/switched"
if [ -n "$store" ] && [ ! -e "$switched" ]; then
  env -u MEM_REMOTE "$real" share switch >/dev/null ||
    echo "mem: cannot switch the replica to the store" >&2
fi
# The local hub's own calls into the replica: never a sync or a hub of their own.
if [ "${1:-}" = share ] && [ -n "$store" ]; then exec env -u MEM_REMOTE "$real" "$@"; fi
"$helper" skills
state="$HOME/.local/state/hub"
# Alive when the local hub answers on mem's socket.
hub_alive() { curl -s -m 2 --unix-socket "$state/local.sock" http://local/ping >/dev/null 2>&1; }
# Off the tailnet though Tailscale was set up: a run that logged out (its
# last step) and asks again must join again, or no answer reaches it.
ts_down() {
  [ -n "${TS_AUTHKEY:-}" ] && [ -x "$HOME/.local/bin/tailscale" ] &&
    ! env -u TS_AUTHKEY "$HOME/.local/bin/tailscale" --socket="$state/tailscaled.sock" status >/dev/null 2>&1
}
peer="${HUB_PEER_TOKEN:-}${HUB_CLOUD_TOKEN:-}"
if [ -n "$peer" ] && { [ -n "$store" ] || [ -n "${TS_AUTHKEY:-}" ]; } && mkdir -p "$state"; then
  alive=1
  hub_alive || alive=
else
  alive=1
  peer=
fi
if [ -n "$peer" ] && { [ -z "$alive" ] || ts_down; }; then
  # The replica as the store has it now, before this mem reads it. A fresh
  # VM replays the whole store, about 25 s on a cloud VM: the wait is long
  # enough for all of it, so the first read is whole.
  if [ -n "$store" ] && [ -z "$alive" ]; then
    flock "$state/sync.lock" "$helper" sync 120 >"$state/sync.log" 2>&1 ||
      echo "mem: the store did not answer; reading the replica as it was" >&2
  fi
  # One start at a time; a later mem finds a dead hub and starts it again.
  (
    # The daemons started below close fd 9, so the lock ends with this start.
    flock -n 9 || exit 0
    bin="$HOME/.local/bin"
    ts=1.98.8
    sock="$state/tailscaled.sock"
    set -e
    "$helper" fetch hub
    host=
    if [ -n "${TS_AUTHKEY:-}" ]; then
      if [ ! -x "$bin/tailscaled" ]; then
        curl -fsSL --max-time 120 "https://pkgs.tailscale.com/stable/tailscale_${ts}_amd64.tgz" -o "$state/ts.tgz"
        tar xzf "$state/ts.tgz" -C "$state"
        cp "$state/tailscale_${ts}_amd64/tailscale" "$state/tailscale_${ts}_amd64/tailscaled" "$bin/"
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
      export HUB_SOCKS5=127.0.0.1:1055 HUB_HOST="$host"
    fi
    # With the store, mem works on the replica, so the hub records answers
    # there: it records nothing while MEM_REMOTE says mem is remote.
    if [ -n "$store" ]; then unset MEM_REMOTE; fi
    # A live hub keeps its socket; it reaches the tailnet through the same
    # SOCKS5 port the new tailscaled listens on.
    if ! hub_alive; then
      env -u TS_AUTHKEY nohup "$bin/hub-x86_64" local >"$state/local.log" 2>&1 9>&- &
    fi
  ) 9>"$state/start.lock" >"$state/start.log" 2>&1 </dev/null &
  # A question goes to the local hub the moment mem writes it, so an ask
  # waits for the hub it just started.
  if [ "${1:-}" = ask ]; then
    i=0
    while { ! hub_alive || ts_down; } && [ $i -lt 60 ]; do sleep 0.5; i=$((i + 1)); done
  fi
fi
if [ -n "$store" ]; then exec env -u MEM_REMOTE "$real" "$@"; fi
exec "$real" "$@"
"#;

pub fn cmd_cloud_setup() -> i32 {
    print!("{}", setup_script(embedded::FILES));
    exit::OK
}

/// `workflow cloud-switch`: refuse while a cloud run is live or parked, so
/// no session loses a write mid-switch, else print the steps.
pub fn cmd_cloud_switch(store: &str) -> i32 {
    if !crate::switch::store_url(store) {
        warn(format!(
            "cloud-switch: --store must be a ws:// or wss:// URL ending in /store/share, not {store}"
        ));
        return exit::USAGE;
    }
    let Some(projects) = memcli::projects() else {
        warn("cloud-switch: mem cannot list its projects");
        return exit::FAILED;
    };
    let now = jiff::Timestamp::now();
    let mut running = false;
    for row in &projects {
        if matches!(
            state(row, || memcli::handoff(&row.name), now),
            Some(State::Live | State::Parked)
        ) {
            let runner = row.runner.as_ref().expect("a cloud state has a runner");
            warn(format!(
                "cloud-switch: a cloud run is running: {} ({}, {})",
                row.name,
                runner.milestone.as_deref().unwrap_or("?"),
                runner.id
            ));
            running = true;
        }
    }
    if running {
        warn("cloud-switch: wait for it to land or end, then run this again");
        return exit::USAGE;
    }
    // switch::store_url has checked it: a scheme, a host, then /store/share.
    let host = store
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .unwrap_or_default();
    println!(
        "Set up the claude.ai cloud environment `workflow`, on a new account or to check one.

On each machine (the mini, the macbook, the nuc):
1. claude /login with the account. On the mini, then: systemctl --user restart 'claude-rc@*'

In claude.ai:
2. On the Code page, connect GitHub, with access to saifulapm/workflow
3. Open the environment selector under the message box: New environment, named workflow
   (or, for one that exists, hover its row under Cloud and select the settings icon). Then:
   Network: Custom, with these allowed hosts:
     {host}  (the store)
     github.com, *.github.com, *.githubusercontent.com  (the repo, and the cloud-bins release with the binaries, their sums and the skills)
     *.tailscale.com, log.tailscale.io  (Tailscale, which carries questions and answers)
   Network secrets: none for {host}
     (the proxy would send its own Authorization over HUB_STORE_TOKEN, and its
     interception drops the WebSocket upgrade: the store's socket gets 426)
   Environment variables, one KEY=value line each, the value being the output of the command:
     HUB_STORE_URL={store}
     HUB_STORE_NAME=main
     HUB_STORE_TOKEN: pass show workflow/hub-store-cloud
     HUB_CLOUD_TOKEN: pass show workflow/hub-cloud-token
     HUB_PEER_TOKEN_SHA256: pass show workflow/hub-peer-token | head -n1 | tr -d '\\n' | sha256sum
       (the hex digest, the first field)
     HUB_STORE_ACCESS_ID: pass show workflow/cloud-next-access-id
     HUB_STORE_ACCESS_SECRET: pass show workflow/cloud-next-access-secret
     TS_AUTHKEY: pass show workflow/tailscale-cloud-authkey
       (a Tailscale auth key lasts 90 days at most: renew it when cloud sessions stop joining the tailnet)
   No MEM_REMOTE, HUB_PEER_TOKEN or Slack variables: the mem server is gone, the peer
   token stays on the devices, and a cloud session opens no Slack socket.
   Setup script: the output of workflow cloud-setup (workflow cloud-setup | wl-copy)
4. Add a custom connector: https://{host}, and log in with the Access PIN mailed to you

On the mini:
5. In Claude Code, /remote-env: choose workflow, where workflow go --cloud starts its sessions

Check: in a cloud session, mem log \"setup test\" reaches the mini within a minute, from cloud-<host>.

If pass has no workflow/hub-store-cloud, mint the cloud's own store token once, on the mini:
   pass show workflow/hubweb-store-token | HUB_STORE_URL={store} HUB_STORE_NAME=main HUB_STORE_ACCESS_ID=$(pass show workflow/cloud-next-access-id) HUB_STORE_ACCESS_SECRET=$(pass show workflow/cloud-next-access-secret) hub share mint 'session_*' 'cloud-*' | pass insert -m -f workflow/hub-store-cloud
   (the shared bearer stays in pass on the mini, for minting only)"
    );
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
        // A local-only park still takes messages, as the m6 walk showed.
        assert!(PROMPT.contains("a local-only park included"), "{PROMPT}");
    }

    /// No end signal reaches a cloud session, so its unsent writes go on
    /// the branch with every push.
    #[test]
    fn every_push_carries_the_unsent_outbox() {
        assert!(PROMPT.contains("mem share export"), "{PROMPT}");
        assert!(PROMPT.contains("before every push"), "{PROMPT}");
        // The tick and the last handoff come after the last task's push.
        assert!(
            PROMPT.contains("after the handoff, run `mem share export` once more"),
            "{PROMPT}"
        );
    }

    /// Nothing can wake a stopped cloud session, so a person's answer is
    /// waited for inside it.
    #[test]
    fn a_question_is_waited_for_in_the_session() {
        assert!(PROMPT.contains("mem questions --wait"), "{PROMPT}");
        assert!(PROMPT.contains("never stop to wait"), "{PROMPT}");
    }

    /// A local-only task parks the run for a device with no question, which
    /// `workflow go` there reads off the handoff.
    #[test]
    fn a_local_only_task_parks_for_a_device() {
        assert!(PROMPT.contains("`[local]`"), "{PROMPT}");
        assert!(PROMPT.contains("parked: local-only"), "{PROMPT}");
    }

    #[test]
    fn the_cloud_kit_reaches_a_session_from_the_start() {
        let script = setup_script(&[
            ("cloud/rules.md", b"# rules\n"),
            ("cloud/settings-hooks.json", b"{\"SessionStart\": []}\n"),
            ("cloud/skills/vite/SKILL.md", b"vite\n"),
        ]);
        assert!(
            script.contains("cat > \"$HOME/.claude/CLAUDE.md\""),
            "{script}"
        );
        assert!(script.contains("# rules"), "{script}");
        // The kit's skills are too big for the setup field: they come with
        // the refresh the SessionStart hook's first mem call runs.
        assert!(!script.contains("vite"), "{script}");
        assert!(!script.contains(".claude/cloud/"), "{script}");
        // The hooks merge into whatever settings the image holds.
        assert!(script.contains("{\"SessionStart\": []}"), "{script}");
        assert!(script.contains("mem-cloud\" hooks"), "{script}");
    }

    #[test]
    fn the_skills_refresh_carries_the_cloud_kit_too() {
        assert!(HELPER.contains("cloud/skills"), "{HELPER}");
        assert!(HELPER.contains("cloud/rules.md"), "{HELPER}");
        assert!(HELPER.contains("settings-hooks.json"), "{HELPER}");
    }

    #[test]
    fn hooks_stay_out_of_the_cloud() {
        let script = setup_script(&[("hooks/pre-commit", b"x\n"), ("skills/a/SKILL.md", b"a\n")]);
        assert!(!script.contains("pre-commit"), "{script}");
        // The skills come from the release's bundle, never inline.
        assert!(!script.contains(".claude/skills/a/SKILL.md"), "{script}");
    }
}
