//! `workflow park` -- put a branch's work somewhere safe and get out of the way
//! (spec §8.9).
//!
//! Uncommitted work becomes a `wip:` commit, and everything from <base> to the
//! branch tip goes into a git bundle under the parked directory. `resume` puts
//! it back, including the uncommitted state -- the wip commit is an envelope,
//! not a decision to commit that work.
//!
//! Bundles live in ~/.local/share/workflow/parked/<project>/, which syncs on its
//! own unit. They never go near mem's store.

use std::path::{Path, PathBuf};

use crate::gitcmd::Git;
use crate::{exit, memcli, paths, sys, warn_as};

const PREFIX: &str = "park";

fn warn(msg: impl AsRef<str>) {
    warn_as(PREFIX, msg);
}

fn say(quiet: bool, msg: impl AsRef<str>) {
    if !quiet {
        warn(msg);
    }
}

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub struct Args<'a> {
    pub repo: Option<&'a Path>,
    pub branch: Option<&'a str>,
    pub base: Option<&'a str>,
    pub note: &'a str,
    pub label: Option<&'a str>,
    /// The orchestrator parks in the middle of its own report and wants none of
    /// this on its output.
    pub quiet: bool,
}

/// The bundle, or nothing when the branch has no work on it beyond the base.
pub fn park(args: &Args) -> Result<Option<PathBuf>, String> {
    let start = args
        .repo
        .map(|p| p.to_path_buf())
        .unwrap_or_else(paths::cwd);
    let probe = Git::at(&start);
    if !probe.inside_worktree() {
        return Err(format!("{} is not a work tree", start.display()));
    }
    let repo = probe
        .toplevel()
        .ok_or_else(|| format!("cannot enter {}", start.display()))?;
    let git = Git::at(&repo);

    let branch = match args.branch {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => git
            .out(&["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_default(),
    };
    if branch.is_empty() || branch == "HEAD" {
        return Err("detached HEAD: name the branch with --branch".into());
    }

    // A base nobody named: the point this branch left the trunk.
    let mut base = args.base.unwrap_or("").to_string();
    if base.is_empty() {
        let upstream = git
            .out(&["rev-parse", "--abbrev-ref", "@{upstream}"])
            .unwrap_or_default();
        for cand in [
            upstream.as_str(),
            "origin/main",
            "origin/master",
            "main",
            "master",
        ] {
            if cand.is_empty() || git.rev_parse_commit(cand).is_none() {
                continue;
            }
            if let Some(mb) = git.out(&["merge-base", cand, &branch]) {
                base = mb;
                break;
            }
        }
    }
    if base.is_empty() {
        let roots = git
            .out(&["rev-list", "--max-parents=0", &branch])
            .unwrap_or_default();
        base = roots.lines().last().unwrap_or("").to_string();
        say(
            args.quiet,
            "no trunk to measure against: parking from the root commit",
        );
    }

    let project = memcli::project_current()
        .map(|p| p.name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            repo.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".into())
        })
        .replace('/', "-");

    // The wip commit. Untracked files come too: half-written work is still work.
    let wip_parent = git.out(&["rev-parse", &branch]).unwrap_or_default();
    if !git.bytes(&["status", "--porcelain", "-uall"]).is_empty() {
        if !git.quiet(&["add", "-A"]) {
            return Err("cannot stage the working tree".into());
        }
        if !git.quiet(&[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "wip: parked work, restore with resume",
        ]) {
            return Err("cannot record the wip commit".into());
        }
        say(args.quiet, "uncommitted work saved as a wip: commit");
    }
    let tip = git.out(&["rev-parse", &branch]).unwrap_or_default();

    let stamp = jiff::Timestamp::now()
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string();
    let label = match args.label {
        Some(l) if !l.is_empty() => l.to_string(),
        _ => branch.replace('/', "-"),
    };
    let dir = paths::parked_root().join(&project);
    std::fs::create_dir_all(&dir).map_err(|_| format!("cannot create {}", dir.display()))?;
    let bundle = dir.join(format!("{label}-{stamp}.bundle"));

    if !git.quiet(&[
        "bundle",
        "create",
        &bundle.to_string_lossy(),
        &format!("{base}..{branch}"),
    ]) {
        let _ = std::fs::remove_file(&bundle);
        say(
            args.quiet,
            format!("nothing to park: {branch} has no work on it beyond {base}"),
        );
        return Ok(None);
    }

    let meta = bundle.with_extension("meta");
    let _ = std::fs::write(
        &meta,
        format!(
            "repo = {}\nbranch = {branch}\nbase = {base}\ntip = {tip}\nwip_parent = {wip_parent}\n\
             machine = {}\nparked = {}\nnote = {}\n",
            repo.display(),
            sys::hostname(),
            jiff::Timestamp::now().strftime("%Y-%m-%dT%H:%M:%SZ"),
            args.note,
        ),
    );

    prune(&dir, args.quiet);
    let used = megabytes(&paths::parked_root());
    let warn_mb = env_num("WORKFLOW_PARK_WARN_MB", 50);
    if used > warn_mb {
        say(
            args.quiet,
            format!("parked bundles now use {used} MB (over the {warn_mb} MB mark)"),
        );
    }

    if !args.quiet {
        println!("{}", bundle.display());
        warn(format!(
            "parked {branch} ({base}..{tip}) -- resume with: workflow resume {}",
            bundle.display()
        ));
    }
    Ok(Some(bundle))
}

/// Retention: five per project, newest kept.
fn prune(dir: &Path, quiet: bool) {
    let keep = env_num("WORKFLOW_PARK_RETENTION", 5) as usize;
    let mut bundles: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "bundle").unwrap_or(false))
        .map(|p| (sys::mtime(&p), p))
        .collect();
    // Newest first, and by name when two share a second.
    bundles.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    for (_, f) in bundles.into_iter().skip(keep) {
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_file(f.with_extension("meta"));
        if let Some(name) = f.file_name() {
            say(
                quiet,
                format!("retention: dropped {}", name.to_string_lossy()),
            );
        }
    }
}

fn megabytes(dir: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else {
                *total += meta.len();
            }
        }
    }
    let mut total = 0;
    walk(dir, &mut total);
    total / (1024 * 1024)
}

pub fn cmd_park(args: &Args) -> i32 {
    match park(args) {
        Ok(_) => exit::OK,
        Err(e) => {
            warn(e);
            exit::FAILED
        }
    }
}

/// The orchestrator's park: the same bundle, none of the narration.
pub fn park_quietly(args: &Args) -> Result<Option<PathBuf>, String> {
    park(&Args {
        quiet: true,
        ..*args
    })
}
