//! `workflow land`: a thread of a Claude Project lands its milestone on the
//! trunk itself. It rebases the milestone branch on the project's trunk on
//! origin, main or an app's own app/<app>, runs the project's test command,
//! pushes the branch to the trunk as a fast-forward, never forced, ticks the
//! milestone, moves an app's trunk onto main, and prints the line the thread
//! ends its report on, which its coordinator reads to start the next thread.
//! A project that names a CI workflow (`mem project set land-ci ci.yml`)
//! pushes the trunk only once that workflow's run on the rebased sha passed.
//! A landing that cannot finish prints a line starting `ask:`, which the
//! thread asks Saiful on the hub, never a park.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::cloud;
use crate::gitcmd::Git;
use crate::memcli;
use crate::{exit, go, warn};

/// The repo's trunk, which a project with a trunk of its own moves onto.
const MAIN: &str = "main";

/// How many times a push the trunk refused, because it moved while the
/// tests ran, starts the landing over before the thread asks Saiful.
const TRIES: usize = 3;

pub fn cmd_land() -> i32 {
    let started = Instant::now();
    let Some(current) = memcli::project_current() else {
        warn("land: this directory is in no project mem knows");
        return exit::USAGE;
    };
    let project = current.name.clone();
    let Some(projects) = memcli::projects() else {
        warn("land: mem cannot list its projects");
        return exit::FAILED;
    };
    let Some(row) = projects.iter().find(|row| row.name == project) else {
        warn(format!("land: mem lists no {project}"));
        return exit::FAILED;
    };
    let Some(slug) = row.milestone.clone() else {
        println!("done: {project}'s roadmap is done");
        return exit::OK;
    };
    let git = Git::here();
    let Some(dir) = git.toplevel() else {
        warn("land: this directory is no git checkout");
        return exit::USAGE;
    };
    let git = Git::at(&dir);
    let branch = cloud::branch(&project, &slug);
    if git.out(&["branch", "--show-current"]).as_deref() != Some(branch.as_str()) {
        warn(format!(
            "land: a milestone lands from its branch {branch}; check it out first"
        ));
        return exit::USAGE;
    }
    if git.out(&["status", "--porcelain"]).is_some() {
        warn("land: commit or drop what is not committed first; the trunk takes commits only");
        return exit::USAGE;
    }
    let Some(verify) = memcli::project_choice("verify") else {
        warn(format!(
            "land: {project} has no test command; `mem project set verify \"<command>\"`"
        ));
        return exit::USAGE;
    };

    let ci = match memcli::project_choice("land_ci") {
        None => None,
        Some(file) => match row
            .remote
            .as_deref()
            .and_then(|r| r.strip_prefix("github.com/"))
        {
            Some(repo) => Some(Ci {
                file,
                repo: repo.to_string(),
                branch: format!("ci/{project}-{slug}"),
            }),
            None => {
                warn(format!(
                    "land: {project} waits on {file}, but its remote is no GitHub repository"
                ));
                return exit::USAGE;
            }
        },
    };

    // An app's folder, where a local run of its tests runs too.
    let folder = match (&current.root, &current.subdir) {
        (Some(root), Some(subdir)) => std::path::Path::new(root).join(subdir),
        _ => dir.clone(),
    };

    // The trunk is read from origin's refs as last fetched, so all of them
    // are fetched first.
    let fetched = git.capture(&["fetch", "-q", "origin"]);
    if !fetched.ok {
        warn(format!(
            "land: cannot fetch origin: {}",
            String::from_utf8_lossy(&fetched.stderr).trim()
        ));
        return exit::FAILED;
    }
    let trunk_name = cloud::trunk(&project, &dir.to_string_lossy());
    let trunk_name = trunk_name.as_str();
    if let Some(line) = claude_line(&git, &format!("origin/{trunk_name}..HEAD")) {
        println!("{line}");
        return exit::USAGE;
    }
    let mut tries = 0;
    loop {
        tries += 1;
        let fetched = git.capture(&["fetch", "-q", "origin", trunk_name]);
        if !fetched.ok {
            warn(format!(
                "land: cannot fetch origin's {trunk_name}: {}",
                String::from_utf8_lossy(&fetched.stderr).trim()
            ));
            return exit::FAILED;
        }
        let trunk = format!("origin/{trunk_name}");
        let base = git.out(&["rev-parse", &trunk]).unwrap_or_default();
        let before = git.out(&["rev-parse", "HEAD"]).unwrap_or_default();
        if let Err(files) = rebase(&git, &trunk) {
            println!(
                "ask: trunk moved: {files} conflict with {slug}'s commits on {branch}; nothing was pushed"
            );
            return exit::USAGE;
        }
        // Each task pushed the branch before the rebase rewrote it; origin's
        // copy follows, with a lease on the tip it last had, so the thread's
        // next push of the branch is a fast-forward whatever comes next.
        if git.out(&["rev-parse", "HEAD"]).unwrap_or_default() != before
            && let Err(why) = push_branch(&git, &dir, &branch, &before)
        {
            warn(format!(
                "land: cannot push {branch} rebased onto {trunk}: {why}; nothing was pushed to {trunk_name}"
            ));
            return exit::FAILED;
        }
        // The ci/ ref takes a sha only once the tests passed on it, so a
        // landing run again while CI runs on that sha does not test it again.
        let head = git.out(&["rev-parse", "HEAD"]).unwrap_or_default();
        let tested = ci
            .as_ref()
            .is_some_and(|ci| remote_tip(&git, &ci.branch).as_deref() == Some(head.as_str()));
        if !tested && !passes(&verify, &folder) {
            warn(format!(
                "land: the tests failed on {branch} rebased onto {trunk}; nothing was pushed to {trunk_name}. \
Fix it on {branch} and land again"
            ));
            return exit::FAILED;
        }
        // Only this sha's own run counts: a trunk that moves while it runs
        // refuses the push below, and the next try's new sha runs again.
        if let Some(ci) = &ci
            && let Err(code) = wait_for_ci(&git, &dir, ci, started)
        {
            return code;
        }
        let why = match push(&dir, &format!("HEAD:{trunk_name}"), None) {
            Ok(()) => break,
            Err(why) => why,
        };
        // Refused because the trunk moved while the tests ran, the landing
        // starts over; for any other reason it stops here, saying why.
        let moved = git.quiet(&["fetch", "-q", "origin", trunk_name])
            && git.out(&["rev-parse", &trunk]).unwrap_or_default() != base;
        if !moved {
            warn(format!(
                "land: cannot push {branch} to origin's {trunk_name}: {why}; nothing was pushed to {trunk_name}"
            ));
            return exit::FAILED;
        }
        if tries == TRIES {
            println!(
                "ask: trunk moved: origin's {trunk_name} kept moving while {slug} was tested; \
nothing was pushed to {trunk_name}"
            );
            return exit::USAGE;
        }
    }

    if !memcli::tick(&project, &slug) {
        warn(format!(
            "land: {slug} is on origin's {trunk_name}, but mem would not tick it; run `mem roadmap --tick {slug}`"
        ));
        return exit::FAILED;
    }
    let mut landed = vec![trunk_name];
    if trunk_name != MAIN {
        if let Err(code) = move_onto_main(&git, &dir, &folder, &verify, &project, &slug) {
            carry_outbox(&git, &dir, &landed);
            return code;
        }
        landed.push(MAIN);
    }
    carry_outbox(&git, &dir, &landed);
    println!("{}", next_line(&project));
    exit::OK
}

/// The GitHub Actions workflow a landing waits on: its file, the repository
/// as owner/name, and the ref only land pushes, which the workflow runs on.
struct Ci {
    file: String,
    repo: String,
    branch: String,
}

/// How many minutes GitHub may take to start a run for the pushed sha, and
/// how many the run may take, before the thread asks Saiful. One call of land
/// waits at most CALL_MINUTES, under the 10 minutes a thread's Bash call
/// may run, and then says to run it again.
const CI_START_MINUTES: u32 = 10;
const CI_RUN_MINUTES: u32 = 90;
const CALL_MINUTES: u32 = 9;

/// A minute; `WORKFLOW_CI_MINUTE_MS` shortens it for the tests.
fn minute() -> Duration {
    let ms = std::env::var("WORKFLOW_CI_MINUTE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000);
    Duration::from_millis(ms)
}

/// The sha origin's `branch` holds: empty when it has none, None when
/// origin cannot be asked.
fn remote_tip(git: &Git, branch: &str) -> Option<String> {
    let listed = git.capture(&["ls-remote", "origin", &format!("refs/heads/{branch}")]);
    listed.ok.then(|| {
        let text = listed.text();
        text.split_whitespace().next().unwrap_or("").to_string()
    })
}

/// When HEAD's sha went to the CI ref, as land noted it in the git dir:
/// the 10 minutes GitHub has to start a run count from there, across calls.
/// A sha with no note, pushed by a call that never wrote one, counts from now.
fn pushed_at(git: &Git, sha: &str, pushed: bool) -> jiff::Timestamp {
    let now = jiff::Timestamp::now();
    let Some(note) = git
        .out(&["rev-parse", "--git-path", "workflow-ci-pushed"])
        .map(|p| git_path(git, &p))
    else {
        return now;
    };
    if !pushed {
        let noted = std::fs::read_to_string(&note).ok().and_then(|text| {
            let (noted, at) = text.trim().split_once(' ')?;
            (noted == sha).then(|| at.parse().ok())?
        });
        if let Some(at) = noted {
            return at;
        }
    }
    let _ = std::fs::write(&note, format!("{sha} {now}\n"));
    now
}

/// A path `git rev-parse --git-path` printed, which is relative to the
/// checkout's top when it is not absolute.
pub fn git_path(git: &Git, path: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(path);
    if path.is_absolute() {
        return path;
    }
    git.toplevel().map_or(path.clone(), |top| top.join(&path))
}

/// Push HEAD to the CI ref, leased on the tip origin holds there now (an
/// earlier try's sha, or none), then wait for the workflow's run of HEAD's
/// sha on that ref. Ok once it passed; otherwise the line said why and the
/// code to exit with. A call told to run again finds its sha on the ref
/// already, pushes nothing and reads the same run.
fn wait_for_ci(git: &Git, dir: &std::path::Path, ci: &Ci, started: Instant) -> Result<(), i32> {
    let sha = git.out(&["rev-parse", "HEAD"]).unwrap_or_default();
    let short = git
        .out(&["rev-parse", "--short", "HEAD"])
        .unwrap_or_default();
    let refname = format!("refs/heads/{}", ci.branch);
    let Some(tip) = remote_tip(git, &ci.branch) else {
        warn(format!(
            "land: cannot read origin's {}; nothing was pushed to the trunk",
            ci.branch
        ));
        return Err(exit::FAILED);
    };
    let pushed = tip != sha;
    if pushed {
        let lease = format!("--force-with-lease={refname}:{tip}");
        if let Err(why) = push(dir, &format!("HEAD:{refname}"), Some(&lease)) {
            let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
            println!(
                "ask: origin refused {}: {why}; nothing was pushed to the trunk",
                ci.branch
            );
            return Err(exit::USAGE);
        }
    }
    let pushed_at = pushed_at(git, &sha, pushed);
    println!("land: waiting on {} for {short}", ci.file);
    let refused = |why: String| {
        println!("ask: GitHub refused the Actions API for {}: {why}", ci.repo);
        exit::USAGE
    };
    // Minutes since `at`, by the clock land runs on.
    let since = |at: jiff::Timestamp| {
        let ms = jiff::Timestamp::now().duration_since(at).as_millis().max(0);
        Duration::from_millis(u64::try_from(ms).unwrap_or(u64::MAX))
    };
    loop {
        let path = format!(
            "repos/{}/actions/workflows/{}/runs?head_sha={sha}&per_page=100",
            ci.repo, ci.file
        );
        let runs = gh(&path).map_err(refused)?;
        let run = runs["workflow_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|run| {
                run["head_sha"] == sha.as_str() && run["head_branch"] == ci.branch.as_str()
            })
            .max_by_key(|run| run["id"].as_u64());
        match run {
            Some(run) if run["status"] == "completed" => {
                if run["conclusion"] == "success" {
                    return Ok(());
                }
                let id = run["id"].as_u64().unwrap_or_default();
                let jobs = gh(&format!(
                    "repos/{}/actions/runs/{id}/jobs?per_page=100",
                    ci.repo
                ))
                .map_err(refused)?;
                let failed: Vec<&str> = jobs["jobs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|job| {
                        !matches!(
                            job["conclusion"].as_str(),
                            Some("success" | "skipped" | "neutral")
                        )
                    })
                    .filter_map(|job| job["name"].as_str())
                    .collect();
                let why = if failed.is_empty() {
                    format!(
                        "the run ended {}",
                        run["conclusion"].as_str().unwrap_or("unknown")
                    )
                } else {
                    failed.join(", ")
                };
                println!("ask: CI red on {short}: {why}");
                return Err(exit::USAGE);
            }
            Some(run) => {
                let created = run["created_at"].as_str().and_then(|t| t.parse().ok());
                if created.is_some_and(|at| since(at) >= minute() * CI_RUN_MINUTES) {
                    println!("ask: {} still running on {short}", ci.file);
                    return Err(exit::USAGE);
                }
            }
            None if since(pushed_at) >= minute() * CI_START_MINUTES => {
                println!("ask: no {} run for {short}", ci.file);
                return Err(exit::USAGE);
            }
            None => {}
        }
        if started.elapsed() >= minute() * CALL_MINUTES {
            println!(
                "again: waiting on {} for {short}; run workflow land again",
                ci.file
            );
            return Err(exit::AGAIN);
        }
        std::thread::sleep(minute() / 2);
    }
}

/// How many times a CI wait asks GitHub before it asks Saiful instead: a
/// 502 or a rate limit passes on a later try.
const GH_TRIES: usize = 3;

/// `gh api <path>`'s JSON, tried GH_TRIES times, a sixth of a minute apart;
/// or gh's own words for why not, the last time.
fn gh(path: &str) -> Result<serde_json::Value, String> {
    let mut tries = 0;
    loop {
        tries += 1;
        match gh_once(path) {
            Err(_) if tries < GH_TRIES => std::thread::sleep(minute() / 6),
            said => return said,
        }
    }
}

/// One `gh api <path>`. `WORKFLOW_GH` replaces gh in the tests.
fn gh_once(path: &str) -> Result<serde_json::Value, String> {
    let bin = std::env::var("WORKFLOW_GH")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "gh".to_string());
    let out = Command::new(&bin)
        .args(["api", path])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run {bin}: {err}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(if said.is_empty() {
            format!("gh api {path} failed")
        } else {
            said
        });
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|err| format!("gh api {path} printed no JSON: {err}"))
}

/// The lines Claude Code adds to a commit, which no trunk takes.
const CLAUDE_LINES: [&str; 2] = [concat!("Co-Authored", "-By"), "Claude-Session"];

/// The identity Claude Code commits under where the kit has not set
/// Saiful's yet, as in an environment set up before the kit did.
const CLAUDE_EMAIL: &str = "noreply@anthropic.com";

/// The ask line for the first commit of `range` made under Claude's
/// identity, or whose message has a line Claude Code adds: one that got
/// past the hooks stops here, before anything is pushed.
fn claude_line(git: &Git, range: &str) -> Option<String> {
    let log = git.out(&[
        "log",
        "--reverse",
        "--format=%h%n%an <%ae>%n%cn <%ce>%n%B%x00",
        range,
    ])?;
    log.split('\0').find_map(|commit| {
        let mut lines = commit.trim_start_matches('\n').lines();
        let sha = lines.next()?;
        for role in ["authored", "committed"] {
            let who = lines.next()?;
            if who.ends_with(&format!("<{CLAUDE_EMAIL}>")) {
                return Some(format!("ask: commit {sha} is {role} by {who}"));
            }
        }
        lines.find_map(|line| {
            CLAUDE_LINES
                .iter()
                .find(|field| {
                    line.get(..=field.len())
                        .is_some_and(|head| head.eq_ignore_ascii_case(&format!("{field}:")))
                })
                .map(|field| format!("ask: commit {sha} carries a {field} line"))
        })
    })
}

/// Does the project's test command pass, run in `folder`? Its output goes
/// to stderr, where the thread reads why it failed.
fn passes(verify: &str, folder: &std::path::Path) -> bool {
    Command::new("bash")
        .args(["-c", verify])
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(std::io::stderr())
        .status()
        .is_ok_and(|status| status.success())
}

/// Move an app, its milestone landed on app/<project>, onto main: rebase it
/// on origin's main, test it there and fast-forward main, never forced,
/// starting over when main moved meanwhile; then app/<project> follows,
/// leased on the tip the landing pushed, since the rebase rewrote it. A
/// conflict or a red run asks with main untouched and the branch as it
/// was pushed, so the thread's own push of it stays a fast-forward.
fn move_onto_main(
    git: &Git,
    dir: &std::path::Path,
    folder: &std::path::Path,
    verify: &str,
    project: &str,
    slug: &str,
) -> Result<(), i32> {
    let app = format!("app/{project}");
    let pushed = git.out(&["rev-parse", "HEAD"]).unwrap_or_default();
    let main = format!("origin/{MAIN}");
    let ask = |why: String| {
        git.quiet(&["reset", "-q", "--hard", &pushed]);
        println!(
            "ask: main moved: {project}: {why}; {slug} is on origin's {app}, main is untouched"
        );
        exit::USAGE
    };
    let mut tries = 0;
    loop {
        tries += 1;
        // Each stop from here on asks with the milestone ticked already.
        if !git.quiet(&["fetch", "-q", "origin", MAIN]) {
            return Err(ask("origin's main cannot be fetched".to_string()));
        }
        let base = git.out(&["rev-parse", &main]).unwrap_or_default();
        // A main that has not moved since app/<project> was cut takes it as
        // it is, tested already.
        if !git.quiet(&["merge-base", "--is-ancestor", &main, "HEAD"]) {
            if let Err(files) = rebase(git, &main) {
                return Err(ask(format!("{files} conflict with main")));
            }
            if !passes(verify, folder) {
                return Err(ask(format!("the tests fail on {app} rebased onto main")));
            }
        }
        let why = match push(dir, &format!("HEAD:{MAIN}"), None) {
            Ok(()) => break,
            Err(why) => why,
        };
        let moved = git.quiet(&["fetch", "-q", "origin", MAIN])
            && git.out(&["rev-parse", &main]).unwrap_or_default() != base;
        if !moved {
            warn(format!("land: origin refused main: {why}"));
            return Err(ask("origin refused the push to main".to_string()));
        }
        if tries == TRIES {
            return Err(ask(format!(
                "origin's main kept moving while {app} was tested on it"
            )));
        }
    }
    if git.out(&["rev-parse", "HEAD"]).unwrap_or_default() != pushed {
        let lease = format!("--force-with-lease=refs/heads/{app}:{pushed}");
        if let Err(why) = push(dir, &format!("HEAD:refs/heads/{app}"), Some(&lease)) {
            warn(format!("land: origin refused {app}: {why}"));
            println!(
                "ask: {app} did not follow main: {slug} is on origin's main, but {app} could not move to it"
            );
            return Err(exit::USAGE);
        }
    }
    Ok(())
}

/// Rebase HEAD on `onto`. Every thread commits its unsent mem writes to the
/// one outbox file at the top of the checkout, so two projects of a repo
/// clash there: the trunk's copy stays, for the next mem here to import,
/// and this thread's own writes still wait in its outbox for the export
/// after the landing. Any other conflict aborts the rebase and names its
/// files.
fn rebase(git: &Git, onto: &str) -> Result<(), String> {
    if git.capture(&["rebase", "-q", onto]).ok {
        return Ok(());
    }
    loop {
        let files = git
            .out(&["diff", "--name-only", "--diff-filter=U"])
            .unwrap_or_default();
        let files: Vec<&str> = files.lines().collect();
        if files != [memcli::OUTBOX_FILE] {
            git.quiet(&["rebase", "--abort"]);
            return Err(if files.is_empty() {
                "its commits".to_string()
            } else {
                files.join(", ")
            });
        }
        // During a rebase, ours is the trunk being rebased onto.
        if !git.quiet(&["checkout", "--ours", "--", memcli::OUTBOX_FILE]) {
            git.quiet(&["rm", "-q", "--", memcli::OUTBOX_FILE]);
        }
        git.quiet(&["add", "--", memcli::OUTBOX_FILE]);
        let next = if git.quiet(&["diff", "--cached", "--quiet"]) {
            "--skip"
        } else {
            "--continue"
        };
        if git.capture(&["-c", "core.editor=true", "rebase", next]).ok {
            return Ok(());
        }
        if !git.quiet(&["rev-parse", "-q", "--verify", "REBASE_HEAD"]) {
            git.quiet(&["rebase", "--abort"]);
            return Err("its commits".to_string());
        }
    }
}

/// `git push origin <refspec>`, never forced but by `lease`, a
/// `--force-with-lease` that holds only while origin's ref is what it last
/// was. The push hook lets an agent's push through only with this variable:
/// a green test run, or a branch of the thread's own, is the yes. On a
/// refusal it says git's reason.
fn push(dir: &std::path::Path, refspec: &str, lease: Option<&str>) -> Result<(), String> {
    let mut args = vec!["push".to_string(), "-q".to_string()];
    args.extend(lease.map(str::to_string));
    args.extend(["origin".to_string(), refspec.to_string()]);
    let out = Command::new("git")
        .args(&args)
        .env("WORKFLOW_ALLOW_PUSH", "1")
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Push the rebased milestone `branch` over origin's copy, leased on the
/// tip origin holds now, or on its having none. A copy holding commits that
/// `before`, the branch as it was before the rebase, lacks, as the other
/// account's would, is kept.
fn push_branch(git: &Git, dir: &std::path::Path, branch: &str, before: &str) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    let listed = git.capture(&["ls-remote", "origin", &refname]);
    if !listed.ok {
        return Err(String::from_utf8_lossy(&listed.stderr).trim().to_string());
    }
    let tip = listed.text();
    let tip = tip.split_whitespace().next().unwrap_or("");
    if !tip.is_empty() && !git.quiet(&["merge-base", "--is-ancestor", tip, before]) {
        // Rewind to the branch as it was: the fix is to merge origin's copy.
        git.quiet(&["reset", "-q", "--hard", before]);
        return Err(format!(
            "origin's {branch} holds commits this checkout lacks; `git pull --rebase origin {branch}` first"
        ));
    }
    let lease = format!("--force-with-lease={refname}:{tip}");
    push(dir, &format!("HEAD:{refname}"), Some(&lease))
}

/// The tick and the writes before it may not have reached the store: a
/// cloud machine that ends takes them with it. They go into the outbox file
/// and onto each trunk that took the landing, where the next machine to
/// read the project sends them.
fn carry_outbox(git: &Git, dir: &std::path::Path, trunks: &[&str]) {
    if memcli::sync_outbox() == Some(0) {
        return;
    }
    if !memcli::share_export() || git.quiet(&["diff", "--cached", "--quiet"]) {
        return;
    }
    let committed = git.quiet(&[
        "commit",
        "-q",
        "-m",
        "Carry the writes the store has not taken yet",
    ]);
    for trunk in trunks {
        if !committed || push(dir, &format!("HEAD:{trunk}"), None).is_err() {
            warn(format!(
                "land: the writes the store has not taken are committed here but not on origin's {trunk}"
            ));
        }
    }
}

/// The line a landed thread ends its report on: the next start line, done,
/// or parked while the project is paused.
fn next_line(project: &str) -> String {
    let paused = memcli::projects()
        .and_then(|rows| rows.into_iter().find(|row| row.name == project))
        .is_some_and(|row| row.paused.is_some());
    let next = memcli::roadmap(project).and_then(|text| go::next_milestone(&text));
    match next {
        None => format!("done: {project}'s roadmap is done"),
        Some(_) if paused => format!("parked: {project} is paused"),
        Some(_) => format!("next: {}", cloud::start_line(project)),
    }
}
