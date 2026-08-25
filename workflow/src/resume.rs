//! `workflow resume` -- bring a parked branch back, uncommitted state and all
//! (spec §8.9, AC9).

use std::path::{Path, PathBuf};

use crate::gitcmd::Git;
use crate::{exit, paths, warn_as};

const PREFIX: &str = "resume";

fn warn(msg: impl AsRef<str>) {
    warn_as(PREFIX, msg);
}

fn meta_field(meta: &Path, key: &str) -> String {
    let Ok(text) = std::fs::read_to_string(meta) else {
        return String::new();
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{key} = ")) {
            return rest.to_string();
        }
    }
    String::new()
}

pub fn cmd_resume(bundle: &Path, repo: Option<&Path>, branch: Option<&str>, force: bool) -> i32 {
    match resume(bundle, repo, branch, force) {
        Ok(()) => exit::OK,
        Err(e) => {
            warn(e);
            exit::FAILED
        }
    }
}

fn resume(
    bundle: &Path,
    repo_arg: Option<&Path>,
    branch_arg: Option<&str>,
    force: bool,
) -> Result<(), String> {
    let bundle = paths::realpath_m(bundle);
    if std::fs::File::open(&bundle).is_err() {
        // Parked on the other machine and not here yet: ask the parked unit
        // for a round, wait it out, look again (friction #KAPRWGBB).
        warn("the bundle is not here; asking for a sync round");
        crate::sync::await_round();
        if std::fs::File::open(&bundle).is_err() {
            return Err(format!(
                "cannot read {} -- it did not arrive with the sync round either",
                bundle.display()
            ));
        }
    }
    let meta = bundle.with_extension("meta");

    let mut start = match repo_arg {
        Some(r) => r.to_path_buf(),
        None => {
            let from_meta = meta_field(&meta, "repo");
            if from_meta.is_empty() {
                paths::cwd()
            } else {
                PathBuf::from(from_meta)
            }
        }
    };
    let mut probe = Git::at(&start);
    // The checkout the meta file names can be gone: a run's park records the
    // task worktree, and run-end cleanup removes it (friction #DWNP07BC). The
    // bundle applies to any checkout that holds its base commit, so the one
    // this command is run from is the fallback -- never over an explicit
    // --repo, whose failure should stay a failure.
    if !probe.inside_worktree() && repo_arg.is_none() {
        let cwd = paths::cwd();
        if cwd != start {
            warn(format!(
                "{} is gone; restoring into this checkout instead",
                start.display()
            ));
            start = cwd;
            probe = Git::at(&start);
        }
    }
    if !probe.inside_worktree() {
        return Err(format!("{} is not a work tree", start.display()));
    }
    let repo = probe
        .toplevel()
        .ok_or_else(|| format!("cannot enter {}", start.display()))?;
    let git = Git::at(&repo);

    let branch = match branch_arg {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => meta_field(&meta, "branch"),
    };
    if branch.is_empty() {
        return Err("the bundle has no meta file: name the branch with --branch".into());
    }

    let bundle_str = bundle.to_string_lossy().to_string();
    if !git.quiet(&["bundle", "verify", &bundle_str]) {
        return Err("the bundle does not apply to this repo (its base commit is missing)".into());
    }

    if git.rev_parse_commit(&branch).is_some() && !force {
        return Err(format!(
            "{branch} already exists here; --force to overwrite it"
        ));
    }

    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let forced = format!("+{refspec}");
    if !git.quiet(&["fetch", "-q", &bundle_str, &refspec])
        && !git.quiet(&["fetch", "-q", "--update-head-ok", &bundle_str, &forced])
    {
        return Err(format!("cannot unbundle {branch}"));
    }

    if !git.bytes(&["status", "--porcelain", "-uall"]).is_empty() && !force {
        return Err("the working tree is dirty; commit or stash it first".into());
    }

    if !git.quiet(&["checkout", "-q", &branch]) {
        return Err(format!("cannot check out {branch}"));
    }

    // Undo the envelope: the wip commit's content goes back to being uncommitted.
    let parent = meta_field(&meta, "wip_parent");
    let tip = git.head().unwrap_or_default();
    if !parent.is_empty()
        && parent != tip
        && git
            .out(&["log", "-1", "--format=%s", &tip])
            .unwrap_or_default()
            .starts_with("wip:")
    {
        if !git.quiet(&["reset", "-q", "--soft", &parent]) {
            return Err("cannot restore the uncommitted state".into());
        }
        warn("restored the uncommitted work from the wip: commit");
    }

    warn(format!("resumed {branch} in {}", repo.display()));
    Ok(())
}
