//! The checkout half of `workflow plan-check` (frictions #DBHZBFY1 and
//! #6485CNC0): a plan is judged against the tree it will run in, not only
//! against its own grammar. Everything here is knowable before dispatch, and
//! each finding used to cost a worker a whole attempt to rediscover.
//!
//! Refusals are Verify lines that cannot pass in this checkout. Warnings are
//! Files lines that do not look like they can hold their task -- warnings,
//! because a plan can mean to create what is not there yet.

use std::path::Path;

use crate::gitcmd::{self, Git};
use crate::plan::{self, Plan, Task};
use crate::{brief, memcli, ownership};

pub struct Findings {
    pub refusals: Vec<String>,
    pub warnings: Vec<String>,
}

const NO_PROSE_WARNING: &str = "plan: no prose above the tasks -- the worker and the reader see only the task blocks; write the Spec and the Rulings first";

/// `prior` is the plans this one waits on: the milestones a roadmap puts ahead
/// of it, so what their tasks write and Give is part of the tree this plan will
/// run in. A plan of its own has none.
pub fn findings(plan: &Plan, prior: &[Plan], root: &Path, plan_file: Option<&Path>) -> Findings {
    let git = Git::at(root);
    let mut f = Findings {
        refusals: Vec::new(),
        warnings: Vec::new(),
    };
    // A plan names every interface it plans, so it answers `git grep` for all
    // of them. Counting itself made every new symbol look like a change the
    // plan forgot to own (friction #33WY4FAR).
    let itself = repo_relative(plan_file, root);
    // `data_file_asserted`'s Gives line carries no `itself` parameter, so the
    // exclusion is applied out here instead, against the message it already
    // built -- the same fact `named_by` gets by taking `itself` directly.
    let itself_named = itself
        .as_deref()
        .map(|me| format!("is named by {me}, which Files does not claim"));
    // Every path some task's Files could carry. A Gives identifier living in
    // a file outside this union is a change the plan forgot to own: the value
    // moves in the files one worker holds while the file asserting it belongs
    // to nobody (friction #8M2YDDXH).
    let mut claimed = std::collections::HashSet::new();
    for t in plan
        .tasks
        .iter()
        .chain(prior.iter().flat_map(|p| p.tasks.iter()))
    {
        for p in ownership::split_patterns(t.files.as_deref().unwrap_or("")) {
            let spec = gitcmd::glob_top(&p);
            claimed.extend(zlines(&git.bytes(&["ls-files", "-z", "--", &spec])));
        }
    }
    // A plan with nothing above its tasks hands the worker and the reader
    // only the task blocks: no Spec to work from, no Rulings the reader can
    // hold the diff to (ruling 3 of m1-wiki-first).
    if plan.prose.trim().is_empty() {
        f.warnings.push(NO_PROSE_WARNING.to_string());
    }
    // The block's budget was checked at dispatch alone, where the remedy is
    // stopping the run to recut the plan (friction #QX8GXNQY). It is the
    // block alone that is measured, so nothing about where the run would
    // write the brief is needed to say it here.
    f.refusals.extend(same_wave_claims(plan, &git));
    for t in &plan.tasks {
        if t.checked {
            continue; // never dispatched, so its lines are history, not risk
        }
        let verify = t.verify.as_deref().unwrap_or("");
        if let Some(msg) = lib_test_without_lib(&t.id, verify, root) {
            f.refusals.push(msg);
        }
        if let Some(msg) = gate_verify_as_verify(&t.id, verify) {
            f.refusals.push(msg);
        }
        // Done: states what the task delivers, so deferral there refuses. A
        // title only warns: "Sweep every TBD out of the docs" names the
        // marker it removes, and blocking that plan would be the check
        // defeating its own point.
        match deferral(t.done.as_deref().unwrap_or("")) {
            Some(Deferral::Refuse(p)) => f.refusals.push(format!(
                "plan: task {}: Done says '{p}' -- that defers work this plan should finish; cut the scope honestly or plan the work",
                t.id
            )),
            Some(Deferral::Warn(p)) => f.warnings.push(format!(
                "plan: task {}: Done says '{p}' -- fine if the task removes one, a deferral if it ships one",
                t.id
            )),
            None => {}
        }
        // A Done sentence this long is standing in for the fix verdict the
        // worker will get once it turns out to have shipped less than it
        // said (ruling 3 of m1-wiki-first): split before that happens.
        let done_words = t.done.as_deref().unwrap_or("").split_whitespace().count();
        if done_words > 40 {
            f.warnings.push(format!(
                "plan: task {}: Done is {done_words} words -- a sentence over forty is a fix verdict waiting; split the task or the sentence",
                t.id
            ));
        }
        if let Some(Deferral::Refuse(p) | Deferral::Warn(p)) = deferral(&t.title) {
            f.warnings.push(format!(
                "plan: task {}: '{p}' in the title -- fine if the task removes it, a deferral if it ships it",
                t.id
            ));
        }
        let patterns = ownership::split_patterns(t.files.as_deref().unwrap_or(""));
        if patterns.len() > 8 {
            f.warnings.push(format!(
                "plan: task {}: Files carries {} patterns -- a task owning more than eight is two tasks",
                t.id,
                patterns.len()
            ));
        }
        for p in &patterns {
            if matches_nothing(&git, root, p) && !dir_claimed(prior, p) {
                f.warnings.push(format!(
                    "plan: task {}: '{p}' matches nothing here and its directory does not exist -- a task creating it, or a typo",
                    t.id
                ));
            } else if only_ignored(&git, p) {
                f.warnings.push(format!(
                    "plan: task {}: '{p}' matches only gitignored paths -- no worktree can carry them, so the merge gate will never see this work",
                    t.id
                ));
            }
        }
        f.warnings.extend(
            data_file_asserted(&t.id, &patterns, &git)
                .into_iter()
                .filter(|w| itself_named.as_deref().is_none_or(|m| !w.contains(m))),
        );
        // A Done sentence that names a file is a claim about what the task's
        // commit holds, and the gate refuses everything outside Files: -- so
        // the two disagreeing is knowable here rather than after a worker has
        // spent a whole context on the task (friction #RT818QJG).
        let owned: std::collections::HashSet<String> = patterns
            .iter()
            .flat_map(|p| zlines(&git.bytes(&["ls-files", "-z", "--", &gitcmd::glob_top(p)])))
            .collect();
        for path in done_paths(&git, t.done.as_deref().unwrap_or(""), &owned) {
            f.warnings.push(format!(
                "plan: task {}: Done names '{path}' and no Files: pattern claims it -- the gate refuses whatever a task writes outside them",
                t.id
            ));
        }
        // A Done that quotes a literal is changing or asserting a spelling,
        // and a test elsewhere that hardcodes it goes red at the gate -- one
        // round trip through the orchestrator for a Files line the planner
        // could have widened at the cut (friction #CS0Q2NA4).
        for (literal, files) in done_literals(
            &git,
            t.done.as_deref().unwrap_or(""),
            &owned,
            itself.as_deref(),
        ) {
            let shown = files.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
            let more = match files.len().saturating_sub(5) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            f.warnings.push(format!(
                "plan: task {}: Done quotes '{literal}' and {shown}{more} carry it outside Files: -- whatever asserts the spelling this task changes goes red at the gate",
                t.id
            ));
        }
        // Read and Pattern point at what the worker opens before editing. By
        // the time it runs its dependencies have landed, so a file one of them
        // writes is there to be read even though this checkout has no such
        // path yet (friction #33WY4FAR).
        let waited_for = ancestors(plan, t);
        let missing = |p: &str| {
            !root.join(p).exists()
                && git.bytes(&["ls-files", "-z", "--", p]).is_empty()
                && !written_by(plan, &waited_for, p)
                && !prior.iter().any(|dep| written_by(dep, &dep.ids(), p))
        };
        let read = t.read.as_deref().unwrap_or("");
        for p in ownership::split_patterns(read) {
            // A `wiki:<slug>` item addresses a page in mem, never a path in
            // this checkout, so `missing` has nothing to ask the tree about
            // it -- its own existence is checked below instead (ruling 3 of
            // m1-wiki-first).
            if p.starts_with("wiki:") {
                continue;
            }
            if missing(&p) {
                f.warnings.push(format!(
                    "plan: task {}: Read names '{p}' and it is not here to be read, nor does a task it waits for write it",
                    t.id
                ));
            }
        }
        for slug in t.wiki_slugs() {
            if memcli::wiki_page(&slug).is_none() {
                f.warnings.push(format!(
                    "plan: task {}: Read names wiki:{slug} and the project has no such page",
                    t.id
                ));
            }
        }
        if let Some(pat) = t.pattern.as_deref() {
            let path = pattern_path(pat);
            if missing(path) {
                f.warnings.push(format!(
                    "plan: task {}: Pattern points at '{path}' and it is not here to copy from",
                    t.id
                ));
            }
        }
        // A consumed interface comes from a dependency's Gives or the tree;
        // one that comes from neither is a name the worker will hunt for. The
        // dependency need not be a direct one: a plan chains `[after:]`, and
        // what t1 gives reaches t3 through t2 (friction #EYC8DHKV).
        for (ident, needle, qualifier) in uses_items(t.uses.as_deref().unwrap_or("")) {
            let Some(needle) = needle else {
                continue;
            };
            let given = waited_for
                .iter()
                .filter_map(|d| plan.get(d))
                .chain(prior.iter().flat_map(|p| p.tasks.iter()))
                .any(|dep| {
                    dep.gives
                        .as_deref()
                        .is_some_and(|g| g.contains(ident.as_str()))
                });
            if !given
                && named_under(&git, &needle, qualifier.as_deref(), itself.as_deref()).is_empty()
            {
                f.warnings.push(format!(
                    "plan: task {}: Uses names '{ident}' and no task it waits for Gives it, nor does the tree",
                    t.id
                ));
            }
        }
        for (ident, needle, qualifier) in uses_items(t.gives.as_deref().unwrap_or("")) {
            let Some(needle) = needle else {
                continue;
            };
            let named: Vec<String> =
                named_under(&git, &needle, qualifier.as_deref(), itself.as_deref())
                    .into_iter()
                    .filter(|file| !claimed.contains(file))
                    .collect();
            if named.is_empty() {
                continue;
            }
            let shown = named.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
            let more = match named.len().saturating_sub(5) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            f.warnings.push(format!(
                "plan: task {}: Gives '{ident}' -- {shown}{more} also name(s) it and no task's Files claims them, so that side of the change is nobody's to make",
                t.id
            ));
        }
        if runs_tests(verify) && !patterns.iter().any(|p| p.to_lowercase().contains("test")) {
            f.warnings.push(format!(
                "plan: task {}: its Verify runs tests and its Files list no test file -- the worker cannot add the test that proves it",
                t.id
            ));
        }
        if let Some(over) = brief::over_budget(t) {
            f.warnings.push(format!(
                "plan: task {}: its block is {over}; a block this size is a task to split, not a line to trim",
                t.id
            ));
        }
    }
    f
}

/// A roadmap is a plan of plans: every milestone names `<id>.md` beside it,
/// holding a plan the run lane will be handed as it stands -- so Files: and
/// Verify: are required of its tasks, and a milestone pointing at no plan, or
/// at a file headed as anything but a plan under that milestone's own id, is
/// refused.
///
/// The walk is by wave rather than by line, so a milestone is read after the
/// ones it waits on: their plans are what it is judged against as well as the
/// tree, because by the time it runs their work has landed. A milestone line
/// carries no Files, Verify, Read or Uses of its own -- everything `findings`
/// judges lives in the plan it names.
pub fn roadmap_findings(roadmap: &Plan, root: &Path, file: &Path) -> Findings {
    let dir = file.parent().unwrap_or(Path::new("."));
    let mut f = Findings {
        refusals: Vec::new(),
        warnings: Vec::new(),
    };
    let mut plans: Vec<(String, Plan)> = Vec::new();
    for id in roadmap.waves.iter().flatten() {
        let path = dir.join(format!("{id}.md"));
        let shown = path.display();
        let Ok(text) = std::fs::read_to_string(&path) else {
            f.refusals.push(format!(
                "roadmap: milestone {id}: no plan at {shown} -- a milestone id is the slug of the plan filed beside the roadmap"
            ));
            continue;
        };
        // The grammar prints its diagnostics as it reads, while every finding
        // below is batched and printed after the whole walk. Without this line
        // two broken milestones spill their complaints into one heap that says
        // nothing about which file either belongs to.
        crate::warn(format!("roadmap: milestone {id}: reading {shown}"));
        let Some(plan) = plan::parse(&text, true) else {
            f.refusals.push(format!(
                "roadmap: milestone {id}: the plan at {shown} does not parse; the lines under it say what is wrong with it"
            ));
            continue;
        };
        // The header word carries as much as the slug: `parse` asks a roadmap
        // for no Files: and no Verify:, so a milestone filed as one would reach
        // run with neither.
        if plan.kind != plan::PlanKind::Plan || plan.plan_id != *id {
            f.refusals.push(format!(
                "roadmap: milestone {id}: the plan at {shown} is headed '# {}: {}' -- a milestone names a plan filed under its own id",
                plan.kind.word(),
                plan.plan_id
            ));
            continue;
        }
        if plan.tasks.len() <= 1 {
            f.warnings.push(format!(
                "roadmap: milestone {id}: its plan has one task, and run refuses a one-task plan -- do that task in the session, or plan the whole milestone"
            ));
        }
        let waited_for = roadmap
            .get(id)
            .map(|m| ancestors(roadmap, m))
            .unwrap_or_default();
        let prior: Vec<Plan> = plans
            .iter()
            .filter(|(mid, _)| waited_for.contains(mid))
            .map(|(_, p)| p.clone())
            .collect();
        let found = findings(&plan, &prior, root, Some(&path));
        // Which milestone a finding came from is the first thing its reader
        // needs: a roadmap prints four plans' worth of them at once. A
        // milestone plan is judged exactly as a plan of its own would be,
        // no-prose finding included: ruling 3 names no exemption for it, and
        // poshra's 36 prose-less plans were a roadmap's milestones.
        f.refusals
            .extend(found.refusals.into_iter().map(|m| format!("{id}: {m}")));
        f.warnings
            .extend(found.warnings.into_iter().map(|m| format!("{id}: {m}")));
        plans.push((id.clone(), plan));
    }
    f
}

/// A prior plan's Files claims something in the directory this pattern points
/// into, so the directory is there by the time this plan runs -- which is the
/// one thing `matches_nothing` could not know about a milestone that creates a
/// tree the next milestone builds in.
fn dir_claimed(prior: &[Plan], pattern: &str) -> bool {
    let dir = fixed_dir(pattern);
    prior
        .iter()
        .flat_map(|p| p.tasks.iter())
        .flat_map(|t| ownership::split_patterns(t.files.as_deref().unwrap_or("")))
        .any(|claim| {
            let claimed = fixed_dir(&claim);
            dir == claimed || dir.starts_with(&format!("{claimed}/"))
        })
}

/// The directory a pattern points into: everything before the last slash of
/// its glob-free head, as `matches_nothing` reads it.
fn fixed_dir(pattern: &str) -> String {
    let fixed: String = pattern
        .chars()
        .take_while(|c| !matches!(c, '*' | '?' | '['))
        .collect();
    match fixed.rsplit_once('/') {
        Some((d, _)) => d.to_string(),
        None => String::new(),
    }
}

/// `cargo test --lib` in a crate with no library target runs nothing and can
/// never go green -- the trap four tasks each paid an attempt to find
/// (friction #DBHZBFY1). A workspace manifest is left alone: the member that
/// Verify would run in is not knowable from here.
fn lib_test_without_lib(task: &str, verify: &str, root: &Path) -> Option<String> {
    if !verify.contains("cargo test") || !verify.split_whitespace().any(|w| w == "--lib") {
        return None;
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    if manifest.contains("[workspace]") || manifest.contains("[lib]") {
        return None;
    }
    if root.join("src/lib.rs").is_file() {
        return None;
    }
    Some(format!(
        "plan: task {task}: Verify runs 'cargo test --lib' and this crate has no library target -- it can never pass here"
    ))
}

/// The gate's own command proves nothing about a task: `workflow verify` runs
/// this whole repo's suite over what is staged, not the change this task
/// makes, so a task copying it as its Verify never learns whether its own
/// work is done. `--gate`, `--hook` or any further word makes it a different,
/// legitimate command; only the bare invocation is refused.
fn gate_verify_as_verify(task: &str, verify: &str) -> Option<String> {
    if !verify.split_whitespace().eq(["workflow", "verify"]) {
        return None;
    }
    Some(format!(
        "plan: task {task}: Verify is 'workflow verify' -- the gate's own command recurses in a worktree; name the check itself, or 'workflow verify --gate' for a task with nothing of its own to run"
    ))
}

/// Nothing tracked matches the pattern, the literal path is not there, and
/// neither is the directory it points into. One of those three usually holds
/// even for a file the task will create; when none does, the pattern is
/// probably not naming what the author thought.
fn matches_nothing(git: &Git, root: &Path, pattern: &str) -> bool {
    if root.join(pattern).exists() {
        return false;
    }
    let spec = gitcmd::glob_top(pattern);
    if !git.bytes(&["ls-files", "-z", "--", &spec]).is_empty() {
        return false;
    }
    !root.join(fixed_dir(pattern)).is_dir()
}

/// Nothing tracked answers the pattern and git sees only ignored matches --
/// files that exist on disk, which is why `matches_nothing` waves them
/// through, but that no commit can carry (friction #A3WHPGE3). The
/// `check-ignore` probe catches the literal path a task would create straight
/// into an ignored directory; on a glob it never matches and decides nothing.
fn only_ignored(git: &Git, pattern: &str) -> bool {
    let spec = gitcmd::glob_top(pattern);
    if !git.bytes(&["ls-files", "-z", "--", &spec]).is_empty() {
        return false;
    }
    let ignored = [
        "ls-files",
        "-z",
        "-o",
        "-i",
        "--exclude-standard",
        "--",
        &spec,
    ];
    !git.bytes(&ignored).is_empty() || git.quiet(&["check-ignore", "-q", "--", pattern])
}

/// A data file's own Files entry says nothing about what reads it: a test
/// elsewhere can assert on its contents while owning none of the change, and
/// the worker who edits the file never sees that test until the assertion
/// fails on it (friction #JRS7GAA5). Each tracked file naming the basename,
/// outside what this task's own Files claims, is worth a warning. A Files
/// entry qualifies by its own extension -- `assets/*.toml` does, a directory
/// or a non-data glob like `workflow/` does not, even though `ls-files`
/// would happily expand either into a `.toml` path underneath. A qualifying
/// entry is then expanded through `ls-files`, the way `matches_nothing` and
/// `only_ignored` already resolve one, so `assets/*.toml` is judged by the
/// data files it actually matches rather than by grepping its own literal
/// asterisk. Entries can overlap the same tracked file, so the expansion is
/// deduped before the grep: a file two patterns both cover is named once,
/// not once per pattern. The plan's own tracked file is excluded from a hit
/// by the caller, the way `named_by` excludes it, since this signature has
/// no room for that `itself` path (friction #33WY4FAR).
fn data_file_asserted(task: &str, files: &[String], git: &Git) -> Vec<String> {
    const DATA_EXTENSIONS: [&str; 6] = ["toml", "json", "yaml", "yml", "csv", "txt"];
    let mut seen = std::collections::HashSet::new();
    let mut data_files: Vec<String> = Vec::new();
    for f in files {
        let is_data = Path::new(f)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| DATA_EXTENSIONS.contains(&e));
        if !is_data {
            continue;
        }
        let spec = gitcmd::glob_top(f);
        for tracked in zlines(&git.bytes(&["ls-files", "-z", "--", &spec])) {
            if seen.insert(tracked.clone()) {
                data_files.push(tracked);
            }
        }
    }
    let mut out = Vec::new();
    for tracked in &data_files {
        let Some(basename) = Path::new(tracked).file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let hits = zlines(&git.bytes(&["grep", "-l", "-z", "-F", basename]));
        for hit in &hits {
            if files.iter().any(|p| covers(p, hit)) {
                continue;
            }
            out.push(format!(
                "plan: task {task}: {tracked} is named by {hit}, which Files does not claim -- a test there may assert its contents"
            ));
        }
    }
    out
}

/// Two tasks in one wave run at once, and a file both claim is one the second
/// merge conflicts on: the rule that tasks running together never touch the
/// same files, made checkable rather than found at the gate and hand-sequenced
/// by the orchestrator (friction #GWD8A4BD). A pattern is judged by the tracked
/// files it expands to, plus the literal path it names when nothing matches yet
/// -- a file the task creates -- and each shared path refuses once, naming
/// both tasks. A ticked task is out of it: it is not dispatched again.
fn same_wave_claims(plan: &Plan, git: &Git) -> Vec<String> {
    let owned = |t: &Task| -> std::collections::BTreeSet<String> {
        let mut set = std::collections::BTreeSet::new();
        for p in ownership::split_patterns(t.files.as_deref().unwrap_or("")) {
            let tracked = zlines(&git.bytes(&["ls-files", "-z", "--", &gitcmd::glob_top(&p)]));
            if tracked.is_empty() && !p.contains(['*', '?', '[']) {
                set.insert(p.trim_end_matches('/').to_string());
            }
            set.extend(tracked);
        }
        set
    };
    let mut out = Vec::new();
    for wave in &plan.waves {
        let tasks: Vec<(&Task, std::collections::BTreeSet<String>)> = wave
            .iter()
            .filter_map(|id| plan.get(id))
            .filter(|t| !t.checked)
            .map(|t| (t, owned(t)))
            .collect();
        for (i, (a, a_owned)) in tasks.iter().enumerate() {
            for (b, b_owned) in &tasks[i + 1..] {
                for path in a_owned.intersection(b_owned) {
                    out.push(format!(
                        "plan: tasks {} and {} run in one wave and both claim {path} -- give one an [after:] on the other",
                        a.id, b.id
                    ));
                }
            }
        }
    }
    out
}

fn runs_tests(verify: &str) -> bool {
    verify.contains("test")
}

/// The paths a Done sentence names that `owned` does not hold, in the order
/// the sentence names them and once each.
///
/// A token is a candidate when it carries a dot or a slash; the tree settles
/// the rest, so prose that merely looks path-shaped -- "e.g.", "identical." --
/// names nothing here and is dropped. A token naming a directory counts as
/// claimed the moment any file under it is.
fn done_paths(git: &Git, done: &str, owned: &std::collections::HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in done.split_whitespace() {
        let token = token
            .trim_matches(|c| "`'\"()[]{},;:!?".contains(c))
            .trim_end_matches('.');
        if !token.contains('.') && !token.contains('/') {
            continue;
        }
        if out.iter().any(|seen| seen == token) {
            continue;
        }
        let hits = zlines(&git.bytes(&["ls-files", "-z", "--", token]));
        if hits.is_empty() || hits.iter().any(|h| owned.contains(h)) {
            continue;
        }
        out.push(token.to_string());
    }
    out
}

/// The quoted spans of a Done sentence -- backticks or double quotes -- that
/// read as strings rather than words (see [`string_like`]), each with the
/// tracked files outside `owned` that carry it, the plan's own file aside.
fn done_literals(
    git: &Git,
    done: &str,
    owned: &std::collections::HashSet<String>,
    itself: Option<&str>,
) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for literal in quoted(done) {
        if !string_like(&literal) || out.iter().any(|(seen, _)| *seen == literal) {
            continue;
        }
        let files: Vec<String> =
            zlines(&git.bytes(&["grep", "-l", "-z", "-F", "-e", &literal, "--"]))
                .into_iter()
                .filter(|file| !owned.contains(file) && Some(file.as_str()) != itself)
                .collect();
        if !files.is_empty() {
            out.push((literal, files));
        }
    }
    out
}

/// A quoted span worth asking the tree about: one with a token that is
/// not a word. Words are what names, paths, commands, flags and
/// placeholders are made of -- `plan::tick`, `src/plan.rs`, `mem log`,
/// `cargo install --path <crate>` -- and Uses, Files and the prose own
/// those; grepping them named README and half the tree on every plan.
/// `- [x]`, `[X]` and `price(Basket $b)` carry a bracket, a dollar, a
/// paren: a spelling something else may assert.
fn string_like(literal: &str) -> bool {
    literal.len() >= 2
        && literal.split_whitespace().any(|token| {
            !token
                .chars()
                .all(|c| c.is_alphanumeric() || "_:./<>-".contains(c))
        })
}

/// The spans between matching backticks or double quotes, in order.
fn quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut open: Option<(char, usize)> = None;
    for (i, c) in text.char_indices() {
        match open {
            None if c == '`' || c == '"' => open = Some((c, i + c.len_utf8())),
            Some((q, start)) if c == q => {
                out.push(text[start..i].to_string());
                open = None;
            }
            _ => {}
        }
    }
    out
}

/// The plan file as git names it, when it is inside this checkout.
fn repo_relative(file: Option<&Path>, root: &Path) -> Option<String> {
    let file = file?.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    Some(file.strip_prefix(root).ok()?.to_string_lossy().to_string())
}

/// The files naming the identifier, narrowed to those naming its qualifier
/// too. A tree carries several `label`s and the item says which it means:
/// `Composer::label()` is the one in the file that also says `Composer`, so
/// the rest are neither work the task forgot to own nor the name it consumes.
fn named_under(
    git: &Git,
    needle: &str,
    qualifier: Option<&str>,
    itself: Option<&str>,
) -> Vec<String> {
    let named = named_by(git, needle, itself);
    let Some(qualifier) = qualifier else {
        return named;
    };
    let under = named_by(git, qualifier, itself);
    named.into_iter().filter(|f| under.contains(f)).collect()
}

/// The tracked files naming the identifier, the plan's own file aside.
fn named_by(git: &Git, ident: &str, itself: Option<&str>) -> Vec<String> {
    zlines(&git.bytes(&["grep", "-l", "-z", "-F", ident]))
        .into_iter()
        .filter(|file| Some(file.as_str()) != itself)
        .collect()
}

/// Every task this one waits for, directly or through another. A cycle would
/// already have been refused by the wave sort; `seen` guards anyway.
fn ancestors(plan: &Plan, task: &Task) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut queue: Vec<String> = task.deps.clone();
    while let Some(id) = queue.pop() {
        if seen.contains(&id) {
            continue;
        }
        if let Some(t) = plan.get(&id) {
            queue.extend(t.deps.iter().cloned());
        }
        seen.push(id);
    }
    seen
}

/// Some task in `deps` claims the path in its Files, so it will exist by the
/// time this task runs.
fn written_by(plan: &Plan, deps: &[String], path: &str) -> bool {
    deps.iter().filter_map(|d| plan.get(d)).any(|dep| {
        ownership::split_patterns(dep.files.as_deref().unwrap_or(""))
            .iter()
            .any(|p| covers(p, path))
    })
}

/// Does the pattern claim the path? A pattern with no glob in it is the path
/// itself or a directory holding it; otherwise `*` stops at a slash and `**`
/// crosses one, as in the pathspec the pattern becomes.
pub(crate) fn covers(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    if pattern == path {
        return true;
    }
    if !pattern.contains(['*', '?']) {
        return path.starts_with(&format!("{pattern}/"));
    }
    glob(pattern.as_bytes(), path.as_bytes())
}

fn glob(pat: &[u8], s: &[u8]) -> bool {
    match pat.first() {
        None => s.is_empty(),
        Some(b'*') if pat.get(1) == Some(&b'*') => {
            // `a/**/b` covers `a/b` too, so the crossing wildcard may eat the
            // separator that follows it or nothing at all.
            let rest = &pat[2..];
            let rest = if rest.first() == Some(&b'/') {
                &rest[1..]
            } else {
                rest
            };
            (0..=s.len()).any(|i| glob(rest, &s[i..]))
        }
        Some(b'*') => {
            let rest = &pat[1..];
            let stop = s.iter().position(|c| *c == b'/').unwrap_or(s.len());
            (0..=stop).any(|i| glob(rest, &s[i..]))
        }
        Some(b'?') => !s.is_empty() && s[0] != b'/' && glob(&pat[1..], &s[1..]),
        Some(c) => s.first() == Some(c) && glob(&pat[1..], &s[1..]),
    }
}

/// An identifier worth asking the tree about by itself. Uses and Gives name
/// exact signatures, so a real symbol carries an underscore or a capital --
/// `set_data`, `StackEntry`, `setData`. A bare lowercase word is usually what
/// this reader made of prose it could not parse: 'untouched' out of
/// "engine.rs untouched", 'rs' out of "pub mod data in lib.rs". Grepping one
/// names half the repo and says nothing (friction #33WY4FAR).
fn greppable(ident: &str) -> bool {
    ident.contains('_') || ident.chars().any(|c| c.is_ascii_uppercase())
}

/// What the tree is asked for on an item's behalf. A symbol-shaped identifier
/// is asked for as it is. A bare word is asked for with the shape the item
/// gave it -- `price(` when the item calls it, `::fixture` when the item
/// qualifies it -- which names a definition or a call and not every sentence
/// with the word in it. A bare word with neither is prose and asks nothing,
/// so `Uses: Basket::fixture(): Basket` is grounded rather than skipped.
fn needle_for(item: &str, ident: &str) -> Option<String> {
    if greppable(ident) {
        return Some(ident.to_string());
    }
    let call = format!("{ident}(");
    if item.contains(&call) {
        return Some(call);
    }
    let scoped = format!("::{ident}");
    if item.contains(&scoped) {
        return Some(scoped);
    }
    None
}

/// NUL-separated git output, one path per entry.
fn zlines(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect()
}

/// `path:12-25` and `path:40` point into a file; anything else is the path
/// itself, colons and all.
fn pattern_path(p: &str) -> &str {
    match p.rsplit_once(':') {
        Some((path, lines))
            if !lines.is_empty() && lines.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            path
        }
        _ => p,
    }
}

/// An item of a Uses or Gives line: its identifier, what to grep the tree for
/// on its behalf, and the qualifier it hangs off.
type Item = (String, Option<String>, Option<String>);

/// One identifier per ` · `-separated item: the token nearest the call site
/// (`CartPricing::price(...)` names `price`), or the item's only token when
/// nothing is called, and declaration keywords never count as the name. Two
/// items reducing to one identifier are reported once, not twice.
fn uses_items(uses: &str) -> Vec<Item> {
    uses.split(" · ")
        .filter_map(|item| {
            let item = without_locations(item);
            let ident = ident_of(&item)?;
            let needle = needle_for(&item, &ident);
            Some((ident, needle, qualifier_of(&item)))
        })
        .fold(Vec::new(), |mut out: Vec<Item>, item| {
            if !out.iter().any(|(ident, _, _)| *ident == item.0) {
                out.push(item);
            }
            out
        })
}

/// The item with its line locations taken out: `layout.rs:219`, a bare
/// `:448`, a `:448-470` span. They point into a file and name nothing, and
/// read as tokens the digits stood where the symbol was, so plan-check asked
/// the tree for '448' (friction #QG0SDXQ4).
fn without_locations(item: &str) -> String {
    item.split(' ')
        .filter_map(|tok| {
            let Some((head, tail)) = tok.rsplit_once(':') else {
                return Some(tok);
            };
            let location = tail.starts_with(|c: char| c.is_ascii_digit())
                && tail.chars().all(|c| c.is_ascii_digit() || c == '-');
            match (location, head.is_empty()) {
                (false, _) => Some(tok),
                (true, true) => None,
                (true, false) => Some(head),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn ident_of(item: &str) -> Option<String> {
    const KEYWORDS: [&str; 12] = [
        "fn", "pub", "struct", "enum", "class", "function", "def", "let", "const", "type", "impl",
        "trait",
    ];
    head_of(item)
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .rfind(|t| !t.is_empty() && !KEYWORDS.contains(t))
        .map(str::to_string)
}

/// What an item declares, with its arguments and its return type dropped:
/// `CartPricing::price: Cents` declares `CartPricing::price`, and `Cents` is
/// what it hands back.
fn head_of(item: &str) -> &str {
    let head = item.split('(').next().unwrap_or(item);
    match head.rsplit_once(": ") {
        Some((h, _)) if !h.is_empty() => h,
        _ => head,
    }
}

/// The type or module an item hangs its identifier off: the token before the
/// last `::` or `.` in its head, when that separator stands right in front of
/// the identifier. `Composer::label()` means the `label` in `Composer` and no
/// other; `price(Basket $b)` hangs off nothing, and `src/brief.rs BUDGET`
/// names a file beside a constant rather than a constant inside one.
pub fn qualifier_of(item: &str) -> Option<String> {
    let head = head_of(item);
    let ident = ident_of(item)?;
    let at = match (head.rfind("::"), head.rfind('.')) {
        (Some(colons), Some(dot)) => colons.max(dot),
        (Some(at), None) | (None, Some(at)) => at,
        (None, None) => return None,
    };
    let sep = if head[at..].starts_with("::") { 2 } else { 1 };
    if head[at + sep..] != ident {
        return None;
    }
    head[..at]
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next_back()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// What a deferral idiom in a task block means for the plan.
#[derive(Debug)]
pub enum Deferral {
    /// The plan defers work it should finish; not ready to dispatch.
    Refuse(&'static str),
    /// The word can name deferred work or work that removes it; a human reads.
    Warn(&'static str),
}

/// Scope-reduction language is how a plan quietly ships less than was asked
/// (research/reports/11): the idioms below are refused outright, while a bare
/// "placeholder" or "stub" only warns because a removal task legitimately
/// names the thing it deletes. Matches are case-insensitive and stop at word
/// boundaries, so "for nowhere" and "stubborn" pass. Only the title and the
/// Done: line are read -- a Verify that greps for TBD or a Files path named
/// tbd/ is a task removing deferrals, not making one.
pub fn deferral(intent: &str) -> Option<Deferral> {
    const REFUSE: [&str; 5] = [
        "tbd",
        "for now",
        "wired later",
        "implement later",
        "simplified version",
    ];
    const WARN: [&str; 2] = ["placeholder", "stub"];
    let lower = intent.to_lowercase();
    for p in REFUSE {
        if found_whole(&lower, p) {
            return Some(Deferral::Refuse(p));
        }
    }
    for p in WARN {
        if found_whole(&lower, p) {
            return Some(Deferral::Warn(p));
        }
    }
    None
}

/// The phrase appears with nothing word-like touching either end.
fn found_whole(haystack: &str, phrase: &str) -> bool {
    let mut from = 0;
    while let Some(at) = haystack[from..].find(phrase) {
        let start = from + at;
        let end = start + phrase.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary(before) && boundary(after) {
            return true;
        }
        from = end;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_pointer_gives_up_its_line_suffix_and_nothing_else() {
        assert_eq!(pattern_path("src/old.rs:12-25"), "src/old.rs");
        assert_eq!(pattern_path("src/old.rs:40"), "src/old.rs");
        assert_eq!(pattern_path("src/old.rs"), "src/old.rs");
        assert_eq!(
            pattern_path("scripts/build:release"),
            "scripts/build:release"
        );
    }

    fn idents(uses: &str) -> Vec<String> {
        uses_items(uses)
            .into_iter()
            .map(|(ident, _, _)| ident)
            .collect()
    }

    /// A qualified item says which of the tree's several `label`s it means,
    /// and the qualifier is what narrows the files naming it down to that one.
    #[test]
    fn a_qualified_item_yields_the_token_it_hangs_off() {
        assert_eq!(qualifier_of("Composer::label()"), Some("Composer".into()));
        assert_eq!(qualifier_of("Cart.total(): Cents"), Some("Cart".into()));
        assert_eq!(qualifier_of("price(Basket $b)"), None);
        // A return type the item names after a colon qualifies nothing.
        assert_eq!(
            qualifier_of("CartPricing::price: crate::Cents"),
            Some("CartPricing".into())
        );
        assert_eq!(qualifier_of("DEFAULT_MODEL"), None);
        // A path standing beside the symbol is not what the symbol hangs off.
        assert_eq!(qualifier_of("src/brief.rs BRIEF_BUDGET"), None);
    }

    #[test]
    fn a_uses_item_yields_the_identifier_nearest_its_call_site() {
        assert_eq!(
            idents("fn price(basket: &Basket) -> Cents · Basket::fixture(): Basket"),
            vec!["price", "fixture"]
        );
        assert_eq!(
            idents("CartPricing::price(Basket $b): Cents"),
            vec!["price"]
        );
        // A colon-typed item without parens names the symbol, not its type.
        assert_eq!(idents("CartPricing::price: Cents"), vec!["price"]);
        assert_eq!(idents("DEFAULT_MODEL"), vec!["DEFAULT_MODEL"]);
        assert!(idents("").is_empty());
    }

    /// A bare lowercase word is not grepped for by itself, but the item
    /// around it usually gives it a shape the tree can be asked for -- so the
    /// plan skill's own example, `Basket::fixture(): Basket`, is grounded
    /// rather than passed over.
    #[test]
    fn a_bare_word_is_asked_for_with_the_shape_its_item_gives_it() {
        assert_eq!(
            needle_for("set_data(v)", "set_data"),
            Some("set_data".into())
        );
        assert_eq!(
            needle_for("Basket::fixture(): Basket", "fixture"),
            Some("fixture(".into())
        );
        assert_eq!(
            needle_for("fn price(basket: &Basket) -> Cents", "price"),
            Some("price(".into())
        );
        assert_eq!(
            needle_for("CartPricing::price: Cents", "price"),
            Some("::price".into())
        );
        assert_eq!(needle_for("engine.rs untouched", "untouched"), None);
        assert_eq!(needle_for("per ruling 10", "10"), None);
        assert_eq!(
            uses_items("Basket::fixture(): Basket · engine.rs untouched"),
            vec![
                (
                    "fixture".to_string(),
                    Some("fixture(".to_string()),
                    Some("Basket".to_string())
                ),
                ("untouched".to_string(), None, None)
            ]
        );
    }

    /// A line location points into a file and names nothing: the digits
    /// used to stand where the symbol was, and the symbol before a span
    /// went unread (friction #QG0SDXQ4).
    #[test]
    fn a_line_location_after_a_path_is_a_place_not_a_name() {
        assert_eq!(
            without_locations("engine/src/layout.rs:219 and the empty place arm at :448"),
            "engine/src/layout.rs and the empty place arm at"
        );
        assert_eq!(
            without_locations("StackEntry engine/src/layout.rs:448-470"),
            "StackEntry engine/src/layout.rs"
        );
        // A colon that is not a location is left exactly as it was.
        assert_eq!(
            without_locations("Basket::fixture(): Basket"),
            "Basket::fixture(): Basket"
        );
        assert_eq!(
            without_locations("scripts/build:release"),
            "scripts/build:release"
        );
        assert_eq!(
            uses_items("engine/src/layout.rs:219 and the empty place arm at :448"),
            vec![("at".to_string(), None, None)]
        );
        assert_eq!(
            uses_items("StackEntry :448 · Layout::place :448-470"),
            vec![
                (
                    "StackEntry".to_string(),
                    Some("StackEntry".to_string()),
                    None
                ),
                (
                    "place".to_string(),
                    Some("::place".to_string()),
                    Some("Layout".to_string())
                )
            ]
        );
    }

    /// deferral reads only what findings() hands it -- the Done: line
    /// (refusals) and the title (downgraded to warnings there), never Verify
    /// commands or Files/Read paths -- so a task that greps TBD out of the
    /// docs is removing deferrals, not making one.
    #[test]
    fn deferral_idioms_are_refused_and_a_bare_placeholder_only_warns() {
        for text in [
            "the simplified version of pricing",
            "static for now",
            "TBD",
            "the button gets wired later by t2",
            "render it, implement later the rest",
        ] {
            assert!(
                matches!(deferral(text), Some(Deferral::Refuse(_))),
                "{text:?} should be refused"
            );
        }
        for text in ["Remove the placeholder hub page", "Drop the stub route"] {
            assert!(
                matches!(deferral(text), Some(Deferral::Warn(_))),
                "{text:?} should only warn"
            );
        }
        for text in [
            "totals identical for the fixture basket",
            "A stubborn cache is invalidated", // 'stub' inside a word is not a stub
            "Reach for nowhere-else state",    // nor is 'for now' inside one
        ] {
            assert!(deferral(text).is_none(), "{text:?} is honest work");
        }
    }

    /// The identifiers that made the dactyl m8 plan print 45 warnings for two
    /// real ones: every junk token this reader pulled out of a prose Gives
    /// item is a bare lowercase word (friction #33WY4FAR).
    #[test]
    fn only_a_symbol_shaped_identifier_is_worth_grepping_for() {
        for real in [
            "set_data",
            "setData",
            "StackEntry",
            "text_of",
            "screen_tree_bound",
            "DEFAULT_MODEL",
            "Value",
        ] {
            assert!(greppable(real), "{real:?} is a symbol");
        }
        for junk in [
            "rs",
            "a",
            "l",
            "10",
            "item",
            "untouched",
            "update",
            "once",
            "after",
        ] {
            assert!(!greppable(junk), "{junk:?} is prose, not a symbol");
        }
    }

    #[test]
    fn a_files_pattern_covers_the_paths_its_pathspec_would() {
        assert!(covers("engine/src/data.rs", "engine/src/data.rs"));
        assert!(covers("engine", "engine/src/data.rs"));
        assert!(covers("engine/", "engine/src/data.rs"));
        assert!(covers("engine/tests/*.rs", "engine/tests/repeat.rs"));
        assert!(covers("engine/**/*.rs", "engine/src/a/b.rs"));
        assert!(covers("docs/**", "docs/plan/11.md"));
        // `*` stops at a separator; `**` is how a pattern crosses one.
        assert!(!covers("engine/*.rs", "engine/src/data.rs"));
        assert!(!covers("engine/src/data.rs", "engine/src/data.rs.bak"));
        assert!(!covers("engine", "engineer/x.rs"));
        assert!(!covers("host-web/src/*.ts", "engine/src/data.rs"));
    }

    /// A plan chains `[after:]`, so what t1 gives reaches t3 through t2 --
    /// which is why the Uses check reads the whole chain (friction #EYC8DHKV).
    #[test]
    fn a_task_waits_for_its_dependencies_dependencies_too() {
        let text = "\
# plan: chain

- [ ] t1 first
      Files: a.rs
      Verify: true
- [ ] t2 second  [after: t1]
      Files: b.rs
      Verify: true
- [ ] t3 third  [after: t2]
      Files: c.rs
      Verify: true
- [ ] t4 apart
      Files: d.rs
      Verify: true
";
        let plan = crate::plan::parse(text, true).expect("the plan parses");
        let mut chain = ancestors(&plan, plan.get("t3").unwrap());
        chain.sort();
        assert_eq!(chain, vec!["t1", "t2"]);
        assert!(ancestors(&plan, plan.get("t1").unwrap()).is_empty());
        assert!(ancestors(&plan, plan.get("t4").unwrap()).is_empty());
        // Read: is answered by any of them, not only the direct one.
        assert!(written_by(&plan, &chain, "a.rs"));
        assert!(written_by(&plan, &chain, "b.rs"));
        assert!(!written_by(&plan, &chain, "d.rs"));
    }

    /// A milestone builds in the tree the milestone before it created, so the
    /// directory its Files point into is not missing -- it is the earlier
    /// plan's work, and warning about it once per milestone would teach the
    /// reader to skip the warning.
    #[test]
    fn a_directory_a_prior_plan_claims_answers_a_pattern_under_it() {
        let text = "\
# plan: m1

- [ ] t1 The session store
      Files: engine/auth/*.rs
      Verify: true
- [ ] t2 The engine
      Files: engine/src/lib.rs
      Verify: true
";
        let prior = vec![crate::plan::parse(text, true).expect("the plan parses")];
        assert!(dir_claimed(&prior, "engine/auth/charge.rs"));
        assert!(dir_claimed(&prior, "engine/auth/refund/*.rs"));
        assert!(dir_claimed(&prior, "engine/src/billing.rs"));
        assert!(!dir_claimed(&prior, "host-web/src/cart.ts"));
        assert!(!dir_claimed(&[], "engine/auth/charge.rs"));
    }

    #[test]
    fn a_test_run_is_recognised_by_the_word_and_true_is_not() {
        assert!(runs_tests("cargo test --test e2e"));
        assert!(runs_tests("pnpm run test:unit"));
        assert!(runs_tests("bin/php artisan test --filter=Cart"));
        assert!(!runs_tests("true"));
        assert!(!runs_tests("cargo build"));
    }

    /// A bare `workflow verify` proves nothing about the task that copied it:
    /// it runs the whole gate, not the task's own change. A flag or a further
    /// word makes it a real command again.
    #[test]
    fn a_bare_workflow_verify_is_refused_but_a_flagged_one_is_not() {
        let msg = gate_verify_as_verify("t1", "workflow verify").expect("bare is refused");
        assert!(msg.contains("t1"), "{msg:?} names the task");
        assert!(
            msg.contains("workflow verify --gate"),
            "{msg:?} names the remedy"
        );
        assert!(
            gate_verify_as_verify("t1", "  workflow verify  ").is_some(),
            "surrounding whitespace does not save it"
        );
        assert!(
            gate_verify_as_verify("t1", "workflow  verify").is_some(),
            "extra whitespace between the words does not save it"
        );
        assert!(gate_verify_as_verify("t1", "workflow verify --gate").is_none());
        assert!(gate_verify_as_verify("t1", "workflow verify project").is_none());
        assert!(gate_verify_as_verify("t1", "cargo test").is_none());
    }

    #[test]
    fn quoted_spans_come_out_in_order_and_an_odd_quote_is_dropped() {
        assert_eq!(
            quoted("`plan::tick` writes `- [x]` and \"[X]\" stays"),
            vec!["plan::tick", "- [x]", "[X]"]
        );
        // A backtick span may hold a double quote and the other way round.
        assert_eq!(quoted("say `it's \"done\"` now"), vec!["it's \"done\""]);
        // An apostrophe is not a quote, and a span never closed is nothing.
        assert!(quoted("the task's `box").is_empty());
    }

    #[test]
    fn a_literal_is_a_span_with_a_token_that_is_not_a_word() {
        for lit in ["- [x]", "[X]", "price(Basket $b)", "a = b"] {
            assert!(string_like(lit), "{lit}");
        }
        // Names, paths, commands, flags and placeholders are words.
        for word in [
            "plan::tick",
            "src/plan.rs",
            "mem log",
            "cargo install --path <crate>",
            "mem project set review-model none",
            "x",
        ] {
            assert!(!string_like(word), "{word}");
        }
    }
}
