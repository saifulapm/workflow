//! Cloud runs: `workflow go <project> --cloud`, the prompt a cloud session
//! starts on, and the setup script of the claude.ai cloud environment
//! `workflow`.

use std::process::Command;

use crate::gitcmd::Git;
use crate::go::{self, Decision, Options};
use crate::memcli::{self, ProjectRow};
use crate::{embedded, exit, warn};

/// The prompt a cloud run follows, which `workflow go --cloud --print`
/// prints in a thread of the project's Claude Project.
const PROMPT: &str = include_str!("cloud-prompt.txt");

/// The standing text of a repository's Claude Project, pasted once into its
/// Project instructions field: what its coordinator and its threads do.
const PROJECT_TEXT: &str = include_str!("project-text.txt");

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

/// The prompt for `slug` of `project` in its checkout `dir`, its branch cut
/// from `base`.
pub fn prompt(project: &str, slug: &str, base: &str, dir: &str) -> String {
    PROMPT
        .trim_end()
        .replace("{dir}", dir)
        .replace("{branch}", &branch(project, slug))
        .replace("{project}", project)
        .replace("{slug}", slug)
        .replace("{base}", base)
}

/// The branch `project` lands on: `app/<project>`, an app's own trunk in a
/// monorepo, once origin has it, or else main. It reads `dir`'s refs as
/// last fetched, so a machine that has not fetched since the branch was
/// pushed still says main; a thread fetches before it asks.
pub fn trunk(project: &str, dir: &str) -> String {
    let app = format!("app/{project}");
    // A new app's folder may exist only on its own branch: git asks from the
    // nearest folder that is there.
    let dir = std::path::Path::new(dir)
        .ancestors()
        .find(|d| d.is_dir())
        .unwrap_or(std::path::Path::new(dir));
    let known = Git::at(dir).quiet(&[
        "rev-parse",
        "--verify",
        "-q",
        &format!("refs/remotes/origin/{app}"),
    ]);
    if known { app } else { "main".to_string() }
}

/// What the next cloud run of `row`, checked out in `dir`, is cut from: its
/// trunk, which a landed run pushed its work to, or the branch of a cloud
/// plan that ended, whose writes the store may not have taken yet.
pub fn base(row: &ProjectRow, dir: &str) -> String {
    match &row.runner {
        Some(runner)
            if runner.machine == "cloud"
                && runner.milestone.as_deref() == Some(PLAN)
                && row.milestone.as_deref() != Some(PLAN) =>
        {
            branch(&row.name, PLAN)
        }
        _ => trunk(&row.name, dir),
    }
}

/// What a coordinator types to start `project`'s next milestone in a thread
/// of its Claude Project, and the one thing it passes on.
pub fn start_line(project: &str) -> String {
    format!("Run the next milestone of {project}.")
}

/// The Project instructions text for the repository `remote`, one Project
/// for its root and every project mem keeps inside it: a thread finds its
/// clone at /home/user/<the remote's last segment>.
pub fn project_text(remote: &str) -> String {
    let remote = remote.trim().trim_end_matches('/');
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let repo = remote.rsplit(['/', ':']).next().unwrap_or(remote);
    PROJECT_TEXT
        .replace("{remote}", remote)
        .replace("{repo}", repo)
}

/// The variables every Project's environment holds for the kit, by name:
/// the ones `workflow cloud-switch` tells Saiful how to fill.
const KIT_VARIABLES: &[&str] = &[
    "HUB_STORE_URL",
    "HUB_STORE_NAME",
    "HUB_STORE_TOKEN",
    "HUB_CLOUD_TOKEN",
    "HUB_PEER_TOKEN_SHA256",
    "HUB_STORE_ACCESS_ID",
    "HUB_STORE_ACCESS_SECRET",
    "TS_AUTHKEY",
    "CONTEXT7_API_KEY",
    "SLACK_BOT_TOKEN",
    "SLACK_USER_ID",
];

/// The network secrets every Project's environment holds for the kit.
const KIT_SECRETS: &[&str] = &["device.on-rexa.com"];

/// The network secrets one mem project's work needs, by host: ebdfy's
/// deploys run through the proxy's Cloudflare token (its verify page's
/// Cloud section), and shopify_apps' staging deploys put each app's staging
/// Worker up with wrangler.
const PROJECT_SECRETS: &[(&str, &str)] = &[
    ("ebdfy", "api.cloudflare.com"),
    ("shopify_apps", "api.cloudflare.com"),
];

/// What the Claude Project of the repository `remote` needs besides its
/// instructions: its name and repository, a start line for each mem project
/// in `repo` (the projects of that repository), and its environment's
/// variables by name, each with the project it belongs to.
fn sheet(repo: &[&ProjectRow], remote: &str) -> String {
    let remote = remote.trim().trim_end_matches('/');
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let mut segments = remote.rsplit(['/', ':']);
    let last = segments.next().unwrap_or(remote);
    let owner = segments.next().unwrap_or_default();
    let root = repo.iter().find(|row| row.subdir.is_none());
    let name = root.map_or(last, |row| row.name.as_str());
    let mut out = format!("Claude Project {name}, repository {owner}/{last}\n");
    out.push_str("Start lines, one for each mem project of the repository:\n");
    for row in repo {
        out.push_str(&format!("  {}\n", start_line(&row.name)));
    }
    out.push_str(&format!(
        "Environment {name}, one on each account. Its variables by name, with the project each \
belongs to; each value comes from Saiful's pass, never from here:\n"
    ));
    for variable in KIT_VARIABLES {
        out.push_str(&format!("  {variable}  every Project\n"));
    }
    out.push_str(
        "  WORKFLOW_ACCOUNT  every Project: work on the work account, main on the main account\n",
    );
    let checkout = root.and_then(|row| {
        row.checkouts
            .iter()
            .map(std::path::Path::new)
            .find(|dir| dir.is_dir())
    });
    let shopify = |row: &&&ProjectRow| {
        row.subdir
            .as_deref()
            .is_some_and(|s| s.starts_with("apps/") || s.starts_with("themes/"))
    };
    if checkout.is_none() && repo.iter().any(|row| shopify(&row)) {
        out.push_str(&format!(
            "  (no checkout of {owner}/{last} here, so its apps' and themes' Shopify variables are left out: run this where it is checked out)\n"
        ));
    }
    for row in repo {
        let Some(subdir) = row.subdir.as_deref() else {
            continue;
        };
        let kind = subdir.split('/').next().unwrap_or_default();
        if !checkout.is_some_and(|dir| crate::shopify::holds(dir, kind)) {
            continue;
        }
        for variable in crate::shopify::variable_names(subdir) {
            out.push_str(&format!("  {variable}  {}\n", row.name));
        }
    }
    out.push_str("Network secrets by host:\n");
    for host in KIT_SECRETS {
        out.push_str(&format!("  {host}  every Project\n"));
    }
    for (project, host) in PROJECT_SECRETS {
        if repo.iter().any(|row| row.name == *project) {
            out.push_str(&format!("  {host}  {project}\n"));
        }
    }
    let workers: Vec<(&str, String, Option<String>, Vec<String>)> = repo
        .iter()
        .filter_map(|row| {
            let app = row.subdir.as_deref()?.strip_prefix("apps/")?;
            let (worker, account, secrets) =
                crate::shopify::staging_worker_of(&checkout?.join("apps").join(app))?;
            Some((row.name.as_str(), worker, account, secrets))
        })
        .collect();
    let mut accounts: Vec<&str> = workers
        .iter()
        .filter_map(|(_, _, account, _)| account.as_deref())
        .collect();
    accounts.sort_unstable();
    accounts.dedup();
    for account in accounts {
        out.push_str(&format!(
            "  api.cloudflare.com path prefixes for the staging deploys, every one of them and no other (the assets upload carries its own token, which the secret would replace): {}\n",
            crate::shopify::staging_api_paths(account).join(", ")
        ));
    }
    if !workers.is_empty() {
        out.push_str(
            "Secrets of each staging Worker by name, with the project each belongs to; each value comes from Saiful's pass, put on the Worker with `wrangler secret put <name> --env staging` in the app's folder before its first deploy:\n",
        );
        for (project, worker, _, secrets) in workers {
            for secret in secrets {
                out.push_str(&format!("  {secret}  {project}, on {worker}\n"));
            }
        }
    }
    out
}

/// `workflow project-text [<project>]`.
pub fn cmd_project_text(project: Option<&str>) -> i32 {
    let Some(projects) = memcli::projects() else {
        warn("project-text: mem cannot list its projects");
        return exit::FAILED;
    };
    let Some(name) = project
        .map(str::to_string)
        .or_else(|| memcli::project_current().map(|p| p.name))
    else {
        warn("project-text: name a project, or run it in a project's checkout");
        return exit::USAGE;
    };
    let Some(row) = projects.iter().find(|row| row.name == name) else {
        warn(format!("project-text: mem lists no {name}"));
        return exit::USAGE;
    };
    let Some(remote) = row.remote.as_deref().filter(|r| !r.trim().is_empty()) else {
        warn(format!(
            "project-text: {name} has no git remote, so no Claude Project can clone it"
        ));
        return exit::USAGE;
    };
    let repo: Vec<&ProjectRow> = projects
        .iter()
        .filter(|other| other.remote.as_deref().map(str::trim) == Some(remote.trim()))
        .collect();
    print!("{}", sheet(&repo, remote));
    println!("--- Project instructions (Settings > Memory): everything after this line");
    print!("{}", project_text(remote));
    exit::OK
}

/// This cloud session as claude.ai's links name it, `session_...`, when this
/// runs in one: `CLAUDE_CODE_REMOTE_SESSION_ID` says `cse_...`.
pub fn this_session() -> Option<String> {
    let id = std::env::var("CLAUDE_CODE_REMOTE_SESSION_ID").ok()?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    Some(match id.strip_prefix("cse_") {
        Some(rest) => format!("session_{rest}"),
        None => id.to_string(),
    })
}

/// The prompt for a cloud plan of `project`.
pub fn plan_prompt(project: &str) -> String {
    PLAN_PROMPT
        .trim_end()
        .replace("{branch}", &branch(project, PLAN))
        .replace("{project}", project)
}

/// How many commits `dir`'s `trunk` holds that origin's, as last fetched,
/// lacks: a cloud run clones origin, so it would start without them. `None`
/// when git cannot tell, as with no origin or no such branch here.
pub fn unpushed(dir: &str, trunk: &str) -> Option<u64> {
    let range = format!("origin/{trunk}..{trunk}");
    let out = Git::at(dir).capture(&["rev-list", "--count", &range]);
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
    // parks with no question, and must not hold its project back for ever.
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

/// `workflow go <project> --cloud --print`. On a machine it prints the
/// prompt and starts nothing: the project's Claude Project starts every
/// cloud run, and `--cloud` alone refuses, naming the start line. In a cloud
/// session, a thread of that Project, it first claims the run as the
/// session's own, and says each outcome but a start as the one line the
/// thread ends its report on: `done: ...` or `parked: ...`.
pub fn cmd_go_cloud(opts: &Options, print: bool, dry_run: bool) -> i32 {
    let me = this_session();
    // In a thread a refusal is the report's last line; on a machine it is
    // said on stderr, as every go refusal is.
    let refuse = |why: String, code: i32| {
        if me.is_some() {
            println!("parked: {why}");
        } else {
            warn(format!("go: {why}"));
        }
        code
    };
    if !print && !dry_run {
        let name = opts
            .project
            .map(str::to_string)
            .or_else(|| memcli::project_current().map(|p| p.name))
            .unwrap_or_else(|| "<project>".to_string());
        warn(format!(
            "go: the Claude Project carries every cloud run: type \"{}\" in {name}'s Claude Project",
            start_line(&name)
        ));
        return exit::USAGE;
    }
    // mem records only these two, and a claim it refused would park on a
    // line that names neither the variable nor its fix.
    let account = std::env::var("WORKFLOW_ACCOUNT").unwrap_or_default();
    if me.is_some() && !matches!(account.trim(), "" | "work" | "main") {
        return refuse(
            format!(
                "this environment's WORKFLOW_ACCOUNT is \"{}\"; set it to work or main in the Project's environment",
                account.trim()
            ),
            exit::USAGE,
        );
    }
    // A thread's replica is the copy its environment baked until a sync
    // reaches the store, and a first mem whose sync failed reads it as it
    // was: a stale roadmap there once read as done. So a thread decides
    // nothing until a sync has performed.
    if me.is_some() && !dry_run && !(0..3).any(|_| memcli::sync_outbox() == Some(0)) {
        return refuse(
            "mem in this thread has not reached the store, so what it reads may be the \
environment's old copy; run `mem sync` until it says performed, then start again"
                .into(),
            exit::USAGE,
        );
    }
    let Some(projects) = memcli::projects() else {
        return refuse("mem cannot list its projects".into(), exit::FAILED);
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
            if me.is_some() {
                println!("done: {project}'s roadmap is done");
            } else {
                println!("{project}: the roadmap is done");
            }
            return exit::OK;
        }
        Decision::Paused(project) => return paused(&project, me.is_some()),
        Decision::Refuse(why) => return refuse(why, exit::USAGE),
        Decision::Fail(why) => return refuse(why, exit::FAILED),
    };
    let Some(row) = projects.iter().find(|row| row.name == start.project) else {
        return refuse(format!("mem lists no {}", start.project), exit::FAILED);
    };
    // A thread woken after its park whose milestone has landed, or that
    // another thread took up since, ends on one line and writes nothing.
    if let Some(me) = me.as_deref()
        && let Some(runner) = row.runner.as_ref().filter(|r| r.machine == "cloud")
        && let Some(slug) = runner.milestone.as_deref()
        && (runner.id == me || runner.earlier.iter().any(|s| s == me))
    {
        if slug != start.slug {
            println!("next: {}", start_line(&start.project));
            return exit::OK;
        }
        if runner.id != me {
            return refuse(
                format!(
                    "another thread took up {slug} of {}: {CODE_PAGE}/{}",
                    start.project, runner.id
                ),
                exit::USAGE,
            );
        }
    }
    // A parked milestone may carry on locally while paused; a cloud run
    // never starts on a paused project.
    if row.paused.is_some() {
        return paused(&start.project, me.is_some());
    }
    if let Some(why) = refusal(row, &start.slug, &start.dir, me.is_some()) {
        return refuse(why, exit::USAGE);
    }
    if let Some(why) = held(row, me.as_deref(), jiff::Timestamp::now()) {
        return refuse(why, exit::USAGE);
    }
    // A run parked on Saiful's questions waits for every answer: the start
    // line he types after answering is the one that carries on.
    let Some(asked) = parked_questions(row) else {
        return refuse(
            format!(
                "mem cannot list {}'s questions; run `mem sync` and start again",
                row.name
            ),
            exit::FAILED,
        );
    };
    // The record's own thread, woken by a message after it parked, carries
    // on past its open questions: only a start line waits for every answer.
    let own = me.as_deref().is_some_and(|me| {
        row.runner
            .as_ref()
            .is_some_and(|r| r.machine == "cloud" && r.id == me)
    });
    if !own && asked.iter().any(|q| !q.answered) {
        println!(
            "parked: {} on Saiful",
            crate::wait::named(&asked, "still waits", "still wait")
        );
        return exit::OK;
    }
    // A woken run is recorded again, so a start line reads it live.
    let again = own
        && state(row, || memcli::handoff(&row.name), jiff::Timestamp::now()) != Some(State::Live);
    if let Some(me) = me.as_deref().filter(|_| !dry_run)
        && let Err(why) = claim(&start.project, &start.slug, me, again)
    {
        return refuse(why, exit::USAGE);
    }
    let mut text = prompt(
        &start.project,
        &start.slug,
        &base(row, &start.dir),
        &start.dir,
    );
    let branch = branch(&start.project, &start.slug);
    if again {
        text.push_str(&so_far(&asked, &branch));
    } else if !asked.is_empty() {
        text.push_str(&answered(&asked, &branch));
    }
    println!("{text}");
    exit::OK
}

/// What a thread taking up a run that parked on Saiful's `asked` questions
/// reads after its prompt: each question with his answer.
fn answered(asked: &[memcli::Question], branch: &str) -> String {
    let each: Vec<String> = asked
        .iter()
        .rev()
        .map(|q| {
            format!(
                "question #{} ({}), answered: {}",
                q.short_id,
                q.title.as_deref().unwrap_or("untitled").trim(),
                q.answer.as_deref().unwrap_or_default().trim()
            )
        })
        .collect();
    format!(
        " The thread before you parked, and Saiful has answered the questions it asked: {}. \
Act on any answer its work has not used yet. {branch} on origin holds that thread's landed \
tasks: keep its commits and start from the first open task in `mem plan`.",
        each.join("; ")
    )
}

/// What this run's own thread, woken after it parked, reads after its
/// prompt: each question it asked with his answer so far, or as still open.
fn so_far(asked: &[memcli::Question], branch: &str) -> String {
    let each: Vec<String> = asked
        .iter()
        .rev()
        .map(|q| {
            let title = q.title.as_deref().unwrap_or("untitled").trim();
            match q.answer.as_deref().filter(|_| q.answered) {
                Some(answer) => format!(
                    "question #{} ({title}), answered: {}",
                    q.short_id,
                    answer.trim()
                ),
                None => format!("question #{} ({title}), still open", q.short_id),
            }
        })
        .collect();
    let asked = if each.is_empty() {
        String::new()
    } else {
        format!(" The questions you asked: {}.", each.join("; "))
    };
    format!(
        " This is a new turn of your own run, which parked.{asked} {branch} on origin holds \
your landed tasks: fetch it, keep its commits and carry on from the first open task in \
`mem plan`."
    )
}

/// When this thread's own cloud run was last recorded, as a start or a
/// wake; `None` outside a thread, or when the run is another session's.
pub fn own_since() -> Option<jiff::Timestamp> {
    let me = this_session()?;
    let current = memcli::project_current()?;
    let projects = memcli::projects()?;
    let row = projects.iter().find(|row| row.name == current.name)?;
    let runner = row
        .runner
        .as_ref()
        .filter(|r| r.machine == "cloud" && r.id == me)?;
    runner.since.as_deref()?.parse().ok()
}

/// A thread that got its answer carries on. When it parked first, its run
/// is recorded again, so a start line now reads it live, not parked, and
/// starts no thread beside it. `Err` with the line to end on when another
/// thread has taken the run up since. Outside a thread, nothing to do.
pub fn carry_on() -> Result<(), String> {
    let Some(me) = this_session() else {
        return Ok(());
    };
    let Some(current) = memcli::project_current() else {
        return Ok(());
    };
    let Some(projects) = memcli::projects() else {
        return Ok(());
    };
    let Some(row) = projects.iter().find(|row| row.name == current.name) else {
        return Ok(());
    };
    let Some(runner) = row.runner.as_ref().filter(|r| r.machine == "cloud") else {
        return Ok(());
    };
    let Some(slug) = runner.milestone.as_deref() else {
        return Ok(());
    };
    let project = &current.name;
    if runner.id != me {
        return Err(format!(
            "parked: another thread took up {slug} of {project}: {CODE_PAGE}/{}",
            runner.id
        ));
    }
    let time = |text: &Option<String>| text.as_deref()?.parse::<jiff::Timestamp>().ok();
    let parked = matches!((time(&runner.since), time(&row.handoff_at)), (Some(since), Some(at)) if at > since)
        && memcli::handoff(project).is_some_and(|text| text.trim_start().starts_with("parked:"));
    if parked
        && !(memcli::set_cloud_runner(project, &me, slug)
            && (0..3).any(|_| memcli::sync_outbox() == Some(0)))
    {
        warn("wait-answer: the store has not taken this run's record again; run `mem sync`");
    }
    Ok(())
}

/// The questions `row`'s cloud run asked before it parked: those of its
/// project asked since the run first started, when its handoff since its
/// latest start says `parked:` and its milestone is still the open one;
/// none for any other run. A thread has no amx session, and it asks
/// Saiful, never an orchestrator, so a question with either is another's.
/// Read past the 7 hours that end a run, since a question may wait
/// overnight. `None` when mem cannot list them.
fn parked_questions(row: &ProjectRow) -> Option<Vec<memcli::Question>> {
    let Some(runner) = row.runner.as_ref().filter(|r| r.machine == "cloud") else {
        return Some(Vec::new());
    };
    if runner.milestone.is_none() || runner.milestone != row.milestone {
        return Some(Vec::new());
    }
    let time = |text: &Option<String>| text.as_deref()?.parse::<jiff::Timestamp>().ok();
    let Some(since) = time(&runner.since) else {
        return Some(Vec::new());
    };
    if time(&row.handoff_at).is_none_or(|at| at <= since)
        || !memcli::handoff(&row.name).is_some_and(|text| text.trim_start().starts_with("parked:"))
    {
        return Some(Vec::new());
    }
    let first = time(&runner.first).unwrap_or(since);
    let asked = memcli::questions(&row.name)?;
    Some(
        asked
            .into_iter()
            .filter(|q| q.asked().is_some_and(|at| at > first))
            .filter(|q| q.session.is_none() && q.audience.as_deref() != Some("orchestrator"))
            .collect(),
    )
}

/// A paused project's start: it says so on stdout and exits 2.
fn paused(project: &str, thread: bool) -> i32 {
    if thread {
        println!("parked: {project} is paused");
    } else {
        println!("{project}: paused");
    }
    exit::USAGE
}

/// Record the run of `slug` as session `me`'s, and make sure the store took
/// it: two threads that claim at once both write, the store keeps the first
/// and refuses the second, whose replica then holds the first's record. So
/// the claim stands once nothing waits in the outbox and the runner read
/// back is `me`. `again` records a run of `me`'s own once more, a woken one.
fn claim(project: &str, slug: &str, me: &str, again: bool) -> Result<(), String> {
    let mine = |projects: &[ProjectRow]| {
        projects
            .iter()
            .find(|row| row.name == project)
            .and_then(|row| row.runner.as_ref())
            .is_some_and(|r| r.machine == "cloud" && r.id == me)
    };
    // A record of its own already in the replica may never have reached
    // the store, so it is written only when missing and checked either way.
    if (again || !memcli::projects().is_some_and(|rows| mine(&rows)))
        && !memcli::set_cloud_runner(project, me, slug)
    {
        return Err(format!("mem would not record this run as {project}'s"));
    }
    let mut waiting = None;
    for _ in 0..3 {
        waiting = memcli::sync_outbox();
        if waiting == Some(0) {
            break;
        }
    }
    if waiting != Some(0) {
        return Err(format!(
            "the store has not taken this run's record of {project} ({}), so another \
run could hold it; run `mem sync` and start again",
            match waiting {
                Some(1) => "1 write still waits".to_string(),
                Some(n) => format!("{n} writes still wait"),
                None => "mem sync failed".to_string(),
            }
        ));
    }
    let Some(projects) = memcli::projects() else {
        return Err("mem cannot list its projects".to_string());
    };
    if mine(&projects) {
        return Ok(());
    }
    let row = projects.iter().find(|row| row.name == project);
    Err(row
        .and_then(|row| held(row, Some(me), jiff::Timestamp::now()))
        .unwrap_or_else(|| format!("another run took {project} first")))
}

/// `workflow plan <project> --cloud`, or with `print` only its prompt.
pub fn cmd_plan_cloud(project: Option<&str>, print: bool) -> i32 {
    let _lock = if print { None } else { start_lock() };
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
    if let Some(why) = held(row, None, jiff::Timestamp::now()) {
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

/// Why `row` may not start a cloud run, if it may not: its own run is live
/// or parked. A parked run still holds its session, which its answers go
/// to. Other projects' runs hold nothing back: there is no cap across
/// projects. `me`, a thread's own session, is never held back by its own
/// run, which it may be resuming. A thread waits for its answers inside
/// its turn, so for a thread a cloud run's park means it ended, but one
/// parked local-only holds it back, since a device takes that from its
/// branch, and so does a device's run under 7 hours that has written no
/// handoff since, or a park.
fn held(row: &ProjectRow, me: Option<&str>, now: jiff::Timestamp) -> Option<String> {
    let project = &row.name;
    let runner = row.runner.as_ref()?;
    if me == Some(runner.id.as_str()) {
        return None;
    }
    let milestone = runner.milestone.as_deref().unwrap_or("?");
    if runner.machine == "cloud" {
        let link = format!("{CODE_PAGE}/{}", runner.id);
        return match state(row, || memcli::handoff(project), now)? {
            // A thread waits for an answer inside its turn and never parks
            // for one: a thread's park means it ended.
            State::Live if me.is_some() => Some(match runner.account.as_deref() {
                Some(account) => {
                    format!("{project} already runs {milestone} from the {account} account: {link}")
                }
                None => format!("{project} already runs {milestone} in {link}"),
            }),
            State::Live | State::Parked if me.is_none() => Some(format!(
                "{project} already has a cloud run: {milestone}, {}",
                runner.id
            )),
            State::LocalOnly if me.is_some() => Some(format!(
                "{project}'s {milestone} is parked local-only in {link}; a device takes it from its branch"
            )),
            _ => None,
        };
    }
    me?;
    let time = |text: &Option<String>| text.as_deref()?.parse::<jiff::Timestamp>().ok();
    let since = time(&runner.since)?;
    if now.duration_since(since) >= jiff::SignedDuration::from_hours(LIVE_HOURS) {
        return None;
    }
    if time(&row.handoff_at).is_some_and(|at| at > since)
        && !memcli::handoff(project).is_some_and(|text| text.trim_start().starts_with("parked:"))
    {
        return None;
    }
    Some(format!(
        "{project} already runs on {} ({})",
        runner.machine, runner.id
    ))
}

/// Held from the own-run check to the runner record, so two starts of one
/// project on this machine cannot both run: the second reads the projects
/// once the first has recorded its run. `None`, said on stderr, when the lock
/// file cannot be had, and the start goes on unguarded.
fn start_lock() -> Option<std::fs::File> {
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
            "go: cannot lock the cloud starts; a start of the same project at the same moment could run it twice",
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
/// session clones the repo and finds its project by the remote, so a
/// project with no remote stays local; work on `dir`'s trunk that origin's
/// lacks waits for Saiful to push it; only
/// the next milestone runs, since a later one would read as landed; and a
/// device orchestrator keeps the project as a local go would.
fn refusal(row: &ProjectRow, slug: &str, dir: &str, thread: bool) -> Option<String> {
    if let Some(why) = repo_refusal(row, dir, thread) {
        return Some(why);
    }
    let name = &row.name;
    if row.milestone.as_deref() != Some(slug) {
        return Some(format!(
            "a cloud run takes {name}'s next milestone, {}, not {slug}",
            row.milestone.as_deref().unwrap_or("none")
        ));
    }
    // A thread has no amx and is no device: `held` weighs a device's run
    // by its record alone.
    if thread {
        return None;
    }
    let runner = row.runner.as_ref().filter(|r| r.machine != "cloud")?;
    if crate::machine::name().as_deref() != Some(runner.machine.as_str()) {
        return Some(format!(
            "{name} runs on {} ({}); start it there, or move it with \
`mem --project {name} project unset runner`",
            runner.machine, runner.id
        ));
    }
    // A device orchestrator parked on a question has stopped its own session,
    // so amx shows nothing running, but the hub resumes it on the answer.
    let time = |text: &Option<String>| text.as_deref()?.parse::<jiff::Timestamp>().ok();
    if let (Some(since), Some(at)) = (time(&runner.since), time(&row.handoff_at))
        && at > since
        && memcli::handoff(name).is_some_and(|text| text.trim_start().starts_with("parked:"))
    {
        return Some(format!(
            "{name}'s {slug} is parked on {} ({}), waiting for an answer; \
answer it and the hub resumes it there",
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
/// session clones origin and finds the project by its remote. In a
/// `thread` the checkout is that clone, so an unpushed main is no reason.
fn repo_refusal(row: &ProjectRow, dir: &str, thread: bool) -> Option<String> {
    let name = &row.name;
    if row.remote.as_deref().is_none_or(str::is_empty) {
        return Some(format!("{name} has no git remote"));
    }
    let trunk = trunk(name, dir);
    let app = format!("app/{name}");
    if !thread
        && trunk != app
        && Git::at(dir).out(&["branch", "--show-current"]).as_deref() == Some(app.as_str())
    {
        return Some(format!(
            "{name} works on {app}, which origin lacks; push {app} first: a cloud run starts from origin"
        ));
    }
    if let Some(n) = unpushed(dir, &trunk).filter(|n| *n > 0 && !thread) {
        let commits = if n == 1 { "commit" } else { "commits" };
        return Some(format!(
            "{name}'s {trunk} is {n} {commits} ahead of origin/{trunk}; push {trunk} first: \
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
    let mut kit_deny = None;
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
        } else if *rel == "cloud/settings-deny.json" {
            kit_deny = Some(text.into_owned());
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
deny=$(mktemp)\n\
cat > \"$kit\" <<'{END}'\n{text}{END}\n\
cat > \"$keys\" <<'{END}'\n{}{END}\n\
cat > \"$deny\" <<'{END}'\n{}{END}\n\
\"$bin/mem-cloud\" hooks \"$kit\" \"$keys\" \"$deny\"\n\
rm -f \"$kit\" \"$keys\" \"$deny\"\n",
            kit_keys.as_deref().unwrap_or("{}\n"),
            kit_deny.as_deref().unwrap_or("[]\n")
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
cat > \"$bin/device\" <<'{END}'\n{DEVICE_SHIM}{END}\n\
chmod +x \"$bin/device\"\n\
{hooks}\"$bin/mem-cloud\" bundle\n\
\"$bin/mem-cloud\" git\n\
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
    want=$(curl -fsSL --retry 5 --retry-delay 2 --max-time 10 ${head:+-H "$head"} "$url.sha256" 2>/dev/null) &&
      [ "${want%% *}" != "$(sha256sum "$bin/$1-x86_64" | cut -d' ' -f1)" ] ||
      { rm -f "$hdr"; return 0; }
  fi
  curl -fsSL --retry 5 --retry-delay 2 --max-time 120 ${head:+-H "$head"} "$url" -o "$part"
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
  gitkit
}
# bundle: the release's skills bundle, put in place when its sum differs
# from the one here. The setup runs it with no secret; the release is public.
bundle() {
  mkdir -p "$state"
  url="$release/workflow-skills.tar.gz"
  want=$(curl -fsSL --retry 5 --retry-delay 2 --max-time 10 "$url.sha256" 2>/dev/null) &&
    want=${want%% *} && [ "$want" != "$(cat "$state/skills.sha256" 2>/dev/null)" ] &&
    curl -fsSL --retry 5 --retry-delay 2 --max-time 60 "$url" -o "$state/skills.tar.gz" || return 0
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
  # The git hook stubs and the global ignore list, which gitkit points git at.
  if [ -d "$new/hooks" ]; then
    mkdir -p "$HOME/.config/git/hooks"
    for item in "$new/hooks"/*; do
      [ -f "$item" ] || continue
      cp "$item" "$HOME/.config/git/hooks/" && chmod 755 "$HOME/.config/git/hooks/${item##*/}"
    done
  fi
  if [ -s "$new/cloud/gitignore" ]; then
    mkdir -p "$HOME/.config/git" && cp "$new/cloud/gitignore" "$HOME/.config/git/ignore"
  fi
  hooks "$new/cloud/settings-hooks.json" "$new/cloud/settings-keys.json" "$new/cloud/settings-deny.json"
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
# hooks <file> [<keys>] [<deny>]: merge the cloud kit's hooks into
# ~/.claude/settings.json, each event the kit names taking the kit's entries,
# its settings (the advisor, the locale) at the top, and its deny rules
# beside the image's own, keeping whatever else the image's settings hold.
# A second merge changes nothing.
hooks() {
  [ -s "$1" ] || [ -s "${2:-}" ] || [ -s "${3:-}" ] || return 0
  mkdir -p "$HOME/.claude"
  python3 - "$HOME/.claude/settings.json" "$1" "${2:-}" "${3:-}" <<'PY_EOF' || echo "mem-cloud: could not merge the cloud kit into settings.json" >&2
import json, os, sys
path, kit, keys, deny = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
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
    # The kit's variables join the image's rather than replace them.
    if isinstance(extra.get("env"), dict):
        settings["env"] = {**(settings.get("env") or {}), **extra["env"]}
    settings.update({k: v for k, v in extra.items() if k not in ("hooks", "env")})
# The deny rules join the image's: a permissions key among the settings
# above would replace its whole block.
rules = []
if deny and os.path.exists(deny):
    try:
        rules = json.load(open(deny))
    except ValueError:
        rules = []
if isinstance(rules, list) and rules:
    perms = settings.get("permissions")
    if not isinstance(perms, dict):
        perms = settings["permissions"] = {}
    have = perms.get("deny") if isinstance(perms.get("deny"), list) else []
    perms["deny"] = have + [r for r in rules if r not in have]
with open(path, "w") as out:
    json.dump(settings, out, indent=2)
PY_EOF
}
# gitkit: Saiful's identity, and the hook stubs and ignore list the bundle
# laid, in git's global config. Every boot, not only with a new bundle:
# claude.ai may write an identity of its own after the snapshot. With no
# stubs, git keeps each repo's own hooks.
gitkit() {
  command -v git >/dev/null 2>&1 || return 0
  git config --global user.name "Saiful Islam"
  git config --global user.email saiful.apm@gmail.com
  if [ -x "$HOME/.config/git/hooks/pre-commit" ]; then
    git config --global core.hooksPath "$HOME/.config/git/hooks"
  fi
  if [ -s "$HOME/.config/git/ignore" ]; then
    git config --global core.excludesFile "$HOME/.config/git/ignore"
  fi
  return 0
}
bake() {
  mkdir -p "$bin"
  for name in mem hub workflow; do
    [ -x "$bin/$name-x86_64" ] && continue
    part="$bin/$name-x86_64.part.$$"
    if ! curl -fsSL --retry 5 --retry-delay 2 --max-time 120 "$release/$name-x86_64" -o "$part"; then
      rm -f "$part"
      # mem and hub are baked by now; without workflow the guard passes.
      [ "$name" = workflow ] && return 3
      return 1
    fi
    chmod +x "$part"
    mv "$part" "$bin/$name-x86_64"
  done
}
# runtimes: the devices' Node 24, pnpm 11, PHP 8.4, playwright-cli, cf, Shopify
# CLI 5 and tmux 3.7, linked into ~/.local/bin, which comes before the image's
# /opt/node22/bin on a session's PATH. Each install is skipped when the cached
# environment already has it.
runtimes() {
  node="${MEM_CLOUD_ROOT:-}/opt/node24/bin"
  failed=
  mkdir -p "$bin"
  if [ -x "$node/node" ]; then
    case "$("$node/pnpm" -v 2>/dev/null)" in
      11.*) ;;
      # The image's NODE_OPTIONS already trusts the system's CA store, which
      # holds the network proxy's; a second CA flag makes node refuse to run.
      *) "$node/node" "$node/npm" install -g --force pnpm@11 >&2 || failed="$failed pnpm" ;;
    esac
    [ "$("$node/playwright-cli" --version 2>/dev/null)" = 0.1.23 ] ||
      "$node/node" "$node/npm" install -g @playwright/cli@0.1.23 >&2 ||
      failed="$failed playwright-cli"
    # Cloudflare's CLI; the environment's network secret for
    # api.cloudflare.com carries the token, so the session holds none.
    case "$("$node/cf" --version 2>/dev/null)" in
      *1.0.0-beta.28*) ;;
      *) "$node/node" "$node/npm" install -g cf@1.0.0-beta.28 >&2 || failed="$failed cf" ;;
    esac
    # Node's fetch reaches the proxy, which adds that token, only with
    # NODE_USE_ENV_PROXY on, and cf sends nothing without a token variable to
    # replace. Here alone: a global NODE_USE_ENV_PROXY hangs pnpm.
    printf '%s\n' '#!/bin/sh' \
      'export NODE_USE_ENV_PROXY=1' \
      'export CLOUDFLARE_API_TOKEN="${CLOUDFLARE_API_TOKEN:-proxy-injected}"' \
      "exec \"$node/cf\" \"\$@\"" >"$bin/cf"
    chmod +x "$bin/cf"
    # Shopify CLI 5, the first that takes a service account token. Its own
    # agent reads the proxy only from SHOPIFY_-named variables; Node's
    # NODE_USE_ENV_PROXY would reach the pnpm it runs and hang it.
    case "$("$node/shopify" version 2>/dev/null)" in
      *5.0.1*) ;;
      *) "$node/node" "$node/npm" install -g @shopify/cli@5.0.1 >&2 || failed="$failed shopify" ;;
    esac
    printf '%s\n' '#!/bin/sh' \
      'export SHOPIFY_HTTPS_PROXY="${SHOPIFY_HTTPS_PROXY:-${HTTPS_PROXY:-${https_proxy:-}}}"' \
      'export SHOPIFY_HTTP_PROXY="${SHOPIFY_HTTP_PROXY:-${HTTP_PROXY:-${http_proxy:-}}}"' \
      'export SHOPIFY_NO_PROXY="${SHOPIFY_NO_PROXY:-${NO_PROXY:-${no_proxy:-}}}"' \
      "exec \"$node/shopify\" \"\$@\"" >"$bin/shopify"
    chmod +x "$bin/shopify"
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
    # The ondrej PPA, written as a source: the image's add-apt-repository
    # crashes, its python3 having no apt_pkg. The key is the one that signs
    # the PPA's noble InRelease (checked 2026-11-05), fetched by its
    # fingerprint.
    apt="${MEM_CLOUD_ROOT:-}/etc/apt"
    key=14AA40EC0831756756D7F66C4F4EA0AAE5267A6C
    suite=$(. "${MEM_CLOUD_ROOT:-}/etc/os-release" 2>/dev/null; echo "${VERSION_CODENAME:-noble}")
    mkdir -p "$apt/keyrings" "$apt/sources.list.d"
    { curl -fsSL --max-time 60 "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x$key" \
        -o "$apt/keyrings/ondrej-php.asc" &&
      printf 'Types: deb\nURIs: https://ppa.launchpadcontent.net/ondrej/php/ubuntu\nSuites: %s\nComponents: main\nSigned-By: /etc/apt/keyrings/ondrej-php.asc\n' \
        "$suite" >"$apt/sources.list.d/ondrej-php.sources" &&
      apt-get update -q >&2 &&
      DEBIAN_FRONTEND=noninteractive apt-get install -y -q php8.4-cli php8.4-intl \
        php8.4-mbstring php8.4-xml php8.4-curl php8.4-zip php8.4-sqlite3 \
        php8.4-mysql php8.4-pgsql php8.4-bcmath php8.4-gd >&2; } || failed="$failed php8.4"
  fi
  php=$(command -v php8.4) && ln -sfn "$php" "$bin/php"
  # tmux 3.7c, the devices': amx's tmux.conf sets extended-keys-format,
  # which the image's tmux 3.4 lacks, and its tests read formats 3.5a
  # expands otherwise (probe-amx2, 2026-11-05). Built from the release's
  # source, checked by its sum; about a minute with its build tools.
  tmux="${MEM_CLOUD_ROOT:-}/opt/tmux-3.7c"
  if [ ! -x "$tmux/bin/tmux" ]; then
    src=$(mktemp -d)
    { apt-get update -q >&2 &&
      DEBIAN_FRONTEND=noninteractive apt-get install -y -q libevent-dev libncurses-dev \
        bison pkg-config build-essential >&2 &&
      curl -fsSL --max-time 120 https://github.com/tmux/tmux/releases/download/3.7c/tmux-3.7c.tar.gz \
        -o "$src/tmux.tar.gz" &&
      echo "7c60cae9a0e25288e2e24750aafc9e8800fc7fd4555e447e1b29ee4201cfb3bf  $src/tmux.tar.gz" |
        sha256sum -c - >&2 &&
      tar -xzf "$src/tmux.tar.gz" -C "$src" &&
      (cd "$src/tmux-3.7c" && ./configure --prefix="$tmux" >&2 && make -j4 >&2 && make install >&2); } ||
      failed="$failed tmux"
    rm -rf "$src"
  fi
  [ -x "$tmux/bin/tmux" ] && ln -sfn "$tmux/bin/tmux" "$bin/tmux"
  [ -z "$failed" ] || { echo "setup: did not install:$failed" >&2; return 1; }
}
case "${1:-}" in
  fetch) fetch "$2" ;;
  runtimes) runtimes ;;
  bake) bake ;;
  skills) skills ;;
  bundle) bundle ;;
  hooks) hooks "$2" "${3:-}" "${4:-}" ;;
  git) gitkit ;;
  # The binaries come from the release; the hub then works on the replica.
  sync) fetch mem && fetch hub && env -u MEM_REMOTE timeout "$2" "$bin/hub-x86_64" share sync ;;
  *) echo "mem-cloud: fetch <mem|hub|workflow>, sync <seconds>, bake, runtimes, skills, bundle, git or hooks <file> [<keys>] [<deny>]" >&2; exit 2 ;;
esac
"#;

/// The `workflow` the guard hook calls by name: the binary the setup baked
/// or a store session's mem fetched. With none, it fails, and the hook lets
/// every call run; a git hook stub's call passes, as the stub's own
/// fail-open would let a commit through.
const WORKFLOW_SHIM: &str = r#"#!/bin/sh
real="$HOME/.local/bin/workflow-x86_64"
if [ ! -x "$real" ]; then
  [ "${1:-}" = hook ] && exit 0
  exit 1
fi
exec "$real" "$@"
"#;

/// `device <verb> ...` for a session: the same as `workflow device <verb> ...`.
const DEVICE_SHIM: &str = r#"#!/bin/sh
exec workflow device "$@"
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

/// The setup script a Claude Project's environment holds: it runs the full
/// setup, [`setup_script`], which CI publishes on the release with its sum,
/// so a change to the kit reaches every environment with no paste.
const BOOTSTRAP: &str = r#"#!/bin/bash
# The setup script of a Claude Project's cloud environment, printed by
# `workflow cloud-setup --bootstrap`. It runs the full setup the workflow
# repo's CI publishes on every push to main.
set -euo pipefail
release=https://github.com/saifulapm/workflow/releases/download/cloud-bins
dir=$(mktemp -d)
cd "$dir"
# CI puts up the file, then its sum: a fetch between the two sees a new
# file and an old sum, so a mismatch is fetched once more.
for try in 1 2; do
  curl -fsSL --retry 5 --retry-delay 2 --max-time 120 "$release/cloud-setup.sh" -o cloud-setup.sh
  curl -fsSL --retry 5 --retry-delay 2 --max-time 60 "$release/cloud-setup.sh.sha256" -o cloud-setup.sh.sha256
  if sha256sum -c --quiet cloud-setup.sh.sha256; then break; fi
  [ "$try" = 2 ] && exit 1
  sleep 5
done
cd /
exec bash "$dir/cloud-setup.sh"
"#;

pub fn cmd_cloud_setup(bootstrap: bool) -> i32 {
    if bootstrap {
        print!("{BOOTSTRAP}");
    } else {
        print!("{}", setup_script(embedded::FILES));
    }
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
     device.on-rexa.com  (the device tunnel: device browser and device run on the routed machine)
   Network secrets: none for {host}
     (the proxy would send its own Authorization over HUB_STORE_TOKEN, and its
     interception drops the WebSocket upgrade: the store's socket gets 426)
   Network secret device.on-rexa.com: type Bearer, header Authorization, the value the output of:
     jq -nc --arg i \"$(pass show workflow/device-access-id)\" --arg s \"$(pass show workflow/device-access-secret)\" '{{\"cf-access-client-id\":$i,\"cf-access-client-secret\":$s}}'
     (the workflow device Access app reads its service token from that one header)
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
     CONTEXT7_API_KEY: pass show AI/context7/clt
       (the ctx7 docs lookups the user CLAUDE.md asks for)
     SLACK_BOT_TOKEN: pass show workflow/slack-bot-token
     SLACK_USER_ID: pass show workflow/slack-user-id
       (the thread's hub posts its questions to Slack; it is the only hub that does)
   No MEM_REMOTE, HUB_PEER_TOKEN or Slack app token: the mem server is gone, the peer
   token stays on the devices, and a cloud session opens no Slack socket (the devices
   take the taps).
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
        // The handoff leaves with the landing, and `workflow land` carries
        // the tick it makes after the push.
        assert!(
            PROMPT.contains(
                "then `mem share export` run and the file it stages committed; then `workflow land` run"
            ),
            "{PROMPT}"
        );
    }

    /// A thread waits on Saiful's answer 30 minutes from the ask, then
    /// parks with a handoff naming the question and the branch (#26D44388).
    #[test]
    fn a_question_is_waited_for_30_minutes_then_parks() {
        assert!(PROMPT.contains("workflow wait-answer <id>"), "{PROMPT}");
        assert!(PROMPT.contains("30 minutes from the ask"), "{PROMPT}");
        assert!(PROMPT.contains("again:"), "{PROMPT}");
        assert!(
            PROMPT.contains("a handoff starting `parked: question #<id> waits on Saiful:`"),
            "{PROMPT}"
        );
        assert!(PROMPT.contains("names the question, {branch}"), "{PROMPT}");
        // The park pushes the branch with its landed tasks, and the report
        // ends on the parked line; an answer is acted on in the same turn.
        assert!(
            PROMPT.contains("then that export and a push of {branch}; then run `workflow wait-answer <id>` once more"),
            "{PROMPT}"
        );
        assert!(
            PROMPT.contains("otherwise end your report with the line it printed"),
            "{PROMPT}"
        );
        assert!(
            PROMPT.contains("On a line starting `answered:`, carry on with that answer"),
            "{PROMPT}"
        );
    }

    /// An answer that came while the thread wrote its park is acted on, and
    /// a parked thread something wakes again asks its start command whether
    /// the run is still its own before it works.
    #[test]
    fn a_parked_thread_checks_once_more_and_asks_again_when_woken() {
        assert!(
            PROMPT.contains("run `workflow wait-answer <id>` once more"),
            "{PROMPT}"
        );
        assert!(
            PROMPT.contains(
                "On every new turn after a park, a message Saiful typed here or a comment on an artifact, run `cd {dir}`, then `workflow go {project} --cloud --print` and follow what it prints"
            ),
            "{PROMPT}"
        );
        // The 6-hour stop is checked before each wait too.
        assert!(
            PROMPT.contains("before each `workflow land` or `workflow wait-answer` run again"),
            "{PROMPT}"
        );
    }

    /// A local-only task parks the run for a device with no question, which
    /// `workflow go` there reads off the handoff.
    #[test]
    fn a_local_only_task_parks_for_a_device() {
        assert!(PROMPT.contains("`[local]`"), "{PROMPT}");
        assert!(PROMPT.contains("parked: local-only"), "{PROMPT}");
    }

    /// A first mem that misses the store reads the replica the environment
    /// baked, which can lack a newer project; a cloud branch then resolves by
    /// its prefix to a sibling (`cloud/ebdfy-ui-m1` to ebdfy), and the runner
    /// record and every later write land there.
    #[test]
    fn a_session_checks_its_project_before_its_first_write() {
        let check = PLAN_PROMPT.find("mem project current").expect(PLAN_PROMPT);
        let write = PLAN_PROMPT
            .find("mem project set runner")
            .expect(PLAN_PROMPT);
        assert!(check < write, "{PLAN_PROMPT}");
        assert!(PLAN_PROMPT.contains("mem sync"), "{PLAN_PROMPT}");
        assert!(
            PLAN_PROMPT.contains("mem project set runner cloud <id> --project {project}"),
            "{PLAN_PROMPT}"
        );
        // A thread's run is recorded by `workflow go --cloud --print`, which
        // the Project instructions run only once mem names the project.
        let check = PROJECT_TEXT
            .find("mem --project <project> project current")
            .expect(PROJECT_TEXT);
        let claim = PROJECT_TEXT
            .find("workflow go <project> --cloud --print")
            .expect(PROJECT_TEXT);
        assert!(check < claim, "{PROJECT_TEXT}");
        assert!(PROJECT_TEXT.contains("mem sync"), "{PROJECT_TEXT}");
    }

    /// The cloud rules park for a device only after the review and the
    /// walk, so the prompt's local-only done says the same.
    #[test]
    fn a_local_only_park_comes_after_the_review_and_walk() {
        assert!(PROMPT.contains("review and walk done at"), "{PROMPT}");
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

    /// The hook stubs reach a thread with the release's bundle, which the
    /// per-boot refresh lays down again; the setup never inlines them.
    #[test]
    fn hook_stubs_come_with_the_bundle_never_inline() {
        let script = setup_script(&[
            ("hooks/pre-commit", b"the stub's own text\n"),
            ("skills/a/SKILL.md", b"a\n"),
        ]);
        assert!(!script.contains("the stub's own text"), "{script}");
        assert!(HELPER.contains("$new/hooks"), "{HELPER}");
        // The skills come from the release's bundle, never inline.
        assert!(!script.contains(".claude/skills/a/SKILL.md"), "{script}");
    }
}
