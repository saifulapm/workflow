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

/// The plan skill's ceiling on the Spec and Rulings, the prose every worker
/// reads before its block. Measured 2026-09-18: the plans that read well sat
/// between 700 and 1,480 tokens of prose.
const PROSE_TOKENS: usize = 1500;

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
    // The prose is what every worker reads, so it is the part with a budget;
    // a plan's length is otherwise its task count, each block budgeted on
    // its own below (friction #J0RQN6WY: a whole-plan ceiling forced eco's
    // roadmap into splits no worker would have needed).
    if plan.prose.len() / 4 > PROSE_TOKENS {
        f.warnings.push(format!(
            "plan: the prose is about {} tokens (bytes ÷ 4) -- past the {PROSE_TOKENS} the plan skill sets for what every worker reads; cut the Spec to what the tasks need and move the rest to a wiki page",
            plan.prose.len() / 4
        ));
    }
    // Every type a Gives item defines, this plan's and the milestones before
    // it, so a type named inside a signature can be asked for by name.
    let defined: std::collections::HashSet<String> = plan
        .tasks
        .iter()
        .chain(prior.iter().flat_map(|p| p.tasks.iter()))
        .flat_map(|t| gives_items(t.gives.as_deref().unwrap_or("")))
        .filter_map(|item| defined_type(&item))
        .collect();
    // The block's budget was checked at dispatch alone, where the remedy is
    // stopping the run to recut the plan (friction #QX8GXNQY). It is the
    // block alone that is measured, so nothing about where the run would
    // write the brief is needed to say it here.
    f.refusals.extend(concurrent_claims(plan, &git));
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
        f.warnings
            .extend(included_unclaimed(&t.id, &patterns, &git));
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
        let uses = t.uses.as_deref().unwrap_or("");
        // Uses items no dependency Gives, so the tree is their only ground:
        // the types inside them are asked about below, while an item a
        // dependency Gives is that dependency's to have defined.
        let mut from_tree: Vec<String> = Vec::new();
        for (ident, needle, qualifier) in uses_items(uses) {
            let Some(needle) = needle else {
                continue;
            };
            // Item for item first: a worker reads its Uses line literally,
            // so a Gives that spells the same symbol another way -- a
            // variant of an enum given whole, a signature that grew a return
            // type -- is a name it hunts for (friction #J0RQN6WY). Only when
            // no dependency names the identifier at all is the tree asked.
            let spelled = item_spelled(uses, &ident);
            let deps: Vec<&Task> = waited_for
                .iter()
                .filter_map(|d| plan.get(d))
                .chain(prior.iter().flat_map(|p| p.tasks.iter()))
                .collect();
            let exact = spelled.as_deref().is_some_and(|s| {
                deps.iter().any(|dep| {
                    gives_items(dep.gives.as_deref().unwrap_or(""))
                        .iter()
                        .any(|g| g == s)
                })
            });
            if exact {
                continue;
            }
            let loose = deps.iter().find_map(|dep| {
                gives_items(dep.gives.as_deref().unwrap_or(""))
                    .into_iter()
                    .find(|g| g.contains(ident.as_str()))
                    .map(|g| (dep.id.clone(), g))
            });
            if let Some((dep_id, given)) = loose {
                f.warnings.push(format!(
                    "plan: task {}: Uses '{}' and {dep_id} Gives it as '{given}' -- one spelling in both, or the worker hunts",
                    t.id,
                    spelled.unwrap_or(ident)
                ));
                continue;
            }
            // No dependency Gives it, but a task of this plan does: the plan
            // itself says the two are coupled by this symbol and no `[after:]`
            // orders them, so they may be dispatched at once and the worker
            // builds against whatever the tree holds. Said here with the edge
            // that mends it, rather than as a name nobody gives.
            let unordered = plan
                .tasks
                .iter()
                .filter(|o| o.id != t.id && !waited_for.contains(&o.id))
                .find(|o| {
                    gives_items(o.gives.as_deref().unwrap_or(""))
                        .iter()
                        .any(|g| g.contains(ident.as_str()))
                });
            if let Some(giver) = unordered {
                let shown = spelled.clone().unwrap_or_else(|| ident.clone());
                f.warnings.push(format!(
                    "plan: task {}: Uses '{shown}' and {} Gives it, but {} does not wait on {} -- the two may run at once; add [after: {}] or order them the other way",
                    t.id, giver.id, t.id, giver.id, giver.id
                ));
                continue;
            }
            if named_under(&git, &needle, qualifier.as_deref(), itself.as_deref()).is_empty() {
                f.warnings.push(format!(
                    "plan: task {}: Uses names '{ident}' and no task it waits for Gives it, nor does the tree",
                    t.id
                ));
            } else if let Some(s) = spelled {
                from_tree.push(s);
            }
        }
        // A block that points at another task -- "as in t1", "see t2", "t1's
        // helper" -- sends the worker after a block it never sees: it has its
        // own block and the plan's prose, nothing else. Only an id after a
        // pointer word counts, since ids are words too (`brief`, `reader`)
        // and a bare mention is prose.
        let others: Vec<String> = plan
            .ids()
            .into_iter()
            .filter(|id| *id != t.id)
            .chain(prior.iter().map(|p| p.plan_id.clone()))
            .collect();
        let done = t.done.as_deref().unwrap_or("");
        let gives = t.gives.as_deref().unwrap_or("");
        for (key, text) in [
            ("the title", t.title.as_str()),
            ("Done", done),
            ("Uses", uses),
            ("Gives", gives),
        ] {
            for (id, phrase) in points_at(text, &others) {
                f.warnings.push(format!(
                    "plan: task {}: {key} says '{phrase}' -- a worker sees only its own block; say here what {id} does or gives, spelled as {id} spells it",
                    t.id
                ));
            }
        }
        // A type named inside a signature is a name too: `ctx: &mut ToolCtx`
        // in a Gives, `-> Result<Outcome, SessionError>` in a Uses. One that
        // no Gives item defines and the tree never names is what a worker
        // spends its first turns hunting for (friction #J0RQN6WY). Names the
        // languages' own libraries own are left alone, and so is a name the
        // item qualifies with a lowercase path, since `tokio::sync::Receiver`
        // says where it lives.
        let mut asked = std::collections::HashSet::new();
        let gives_line: Vec<String> = gives_items(t.gives.as_deref().unwrap_or(""));
        for (key, items) in [("Uses", &from_tree), ("Gives", &gives_line)] {
            for item in items {
                for ty in type_tokens(item) {
                    if defined.contains(&ty) || !asked.insert(ty.clone()) {
                        continue;
                    }
                    if named_by(&git, &ty, itself.as_deref()).is_empty() {
                        f.warnings.push(format!(
                            "plan: task {}: {key} names type '{ty}' inside '{item}' and nothing defines it -- no Gives item here or earlier, and not the tree; give it an item of its own or the worker hunts",
                            t.id
                        ));
                    }
                }
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
        if runs_tests(verify) && !has_test_file(&git, &patterns) {
            f.warnings.push(format!(
                "plan: task {}: its Verify runs tests and its Files list no test file -- the worker cannot add the test that proves it",
                t.id
            ));
        } else if done_asks_test(t.done.as_deref().unwrap_or("")) && !has_test_file(&git, &patterns)
        {
            f.warnings.push(format!(
                "plan: task {}: its Done asks for a test and its Files list no test file -- the worker cannot add the test it is asked for",
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
/// fails on it (friction #JRS7GAA5). Each tracked file naming the exact
/// tracked path, outside what this task's own Files claims, is worth a
/// warning -- a file naming the same basename by another spelling is
/// `included_unclaimed`'s to catch when it is an include, not this one's
/// (friction #1TVAS5X9). A Files entry qualifies by its own extension --
/// `assets/*.toml` does, a directory
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
        let hits = zlines(&git.bytes(&["grep", "-l", "-z", "-F", tracked]));
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

/// `include_str!` and `include_bytes!` graft another file's bytes into the
/// binary at compile time, so a task whose Files claims the file that
/// includes but not the file it names owns only half the change: whatever a
/// test asserts about those bytes is left for nobody to fix (ruling 10, wiki
/// review-2026-09 defect 10). Only a tracked `.rs` file has a compile time to
/// scan -- a shell script or a page can carry the macro names in a comment or
/// a fixture without calling either. The literal is resolved against the
/// including file's own directory, `..` folded, the way the compiler
/// resolves it -- so `include_str!("../README.md")` in `src/cli.rs` names
/// `README.md`, not `src/README.md`; a literal whose `..` climbs above the
/// repo root names nothing. Only a tracked target counts: a file the task
/// means to create is `matches_nothing`'s business, not this one's. A (file,
/// target) pair warns once, however many calls name it.
fn included_unclaimed(task: &str, files: &[String], git: &Git) -> Vec<String> {
    let Some(root) = git.toplevel() else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut tracked: Vec<String> = Vec::new();
    for f in files {
        let spec = gitcmd::glob_top(f);
        for t in zlines(&git.bytes(&["ls-files", "-z", "--", &spec])) {
            if seen.insert(t.clone()) {
                tracked.push(t);
            }
        }
    }
    let mut out = Vec::new();
    let mut warned = std::collections::HashSet::new();
    for file in &tracked {
        if !file.ends_with(".rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        let dir = Path::new(file).parent().unwrap_or(Path::new(""));
        for literal in include_literals(&text) {
            let Some(target) = fold_dots(&dir.join(&literal)) else {
                continue; // climbed above the repo root: names nothing
            };
            if git.bytes(&["ls-files", "-z", "--", &target]).is_empty() {
                continue; // not a tracked path: not this check's to raise
            }
            if files.iter().any(|p| covers(p, &target)) {
                continue;
            }
            if !warned.insert((file.clone(), target.clone())) {
                continue; // one (file, target) pair warns once
            }
            out.push(format!(
                "plan: task {task}: {file} includes {target} at compile time and Files does not claim it -- a test in {file} asserts its contents, so that side of the change is nobody's to make"
            ));
        }
    }
    out
}

/// The literal path of each `include_str!` or `include_bytes!` call in
/// source text, in the order they appear: the first quoted span after the
/// macro name, the shape both take here.
fn include_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for macro_name in ["include_str!", "include_bytes!"] {
        let mut from = 0;
        while let Some(at) = text[from..].find(macro_name) {
            let start = from + at + macro_name.len();
            from = start;
            let rest = &text[start..];
            let Some(open) = rest.find('"') else { continue };
            let Some(len) = rest[open + 1..].find('"') else {
                continue;
            };
            out.push(rest[open + 1..open + 1 + len].to_string());
        }
    }
    out
}

/// A path with its `.` and `..` components folded away lexically, the way
/// `include_str!`'s path resolves without touching the filesystem. A `..`
/// with nothing left to pop climbs above the root the path started under --
/// that escapes the tree, so `None` says the path names nothing.
fn fold_dots(path: &Path) -> Option<String> {
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop()?;
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(s) => out.push(s.to_os_string()),
            _ => {}
        }
    }
    Some(
        out.iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// A run dispatches from the ready set, not one wave at a time, so two tasks
/// run at once whenever neither waits for the other -- a wave was the wrong
/// test for this, since two tasks a level apart with no `[after:]` between
/// them can still be dispatched together (wiki review-2026-09, defects 1 and
/// 2). A file both claim is one the second merge conflicts on: the rule that
/// concurrent tasks never touch the same files, made checkable rather than
/// found at the gate and hand-sequenced by the orchestrator (friction
/// #GWD8A4BD). A pattern is judged by the tracked files it expands to, plus
/// the literal path it names when nothing matches yet -- a file the task
/// creates -- and each shared path refuses once, naming both tasks in plan
/// order. A ticked task is out of it: it is not dispatched again.
fn concurrent_claims(plan: &Plan, git: &Git) -> Vec<String> {
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
    let tasks: Vec<(&Task, std::collections::BTreeSet<String>, Vec<String>)> = plan
        .tasks
        .iter()
        .filter(|t| !t.checked)
        .map(|t| (t, owned(t), ancestors(plan, t)))
        .collect();
    let mut out = Vec::new();
    for (i, (a, a_owned, a_ancestors)) in tasks.iter().enumerate() {
        for (b, b_owned, b_ancestors) in &tasks[i + 1..] {
            if a_ancestors.contains(&b.id) || b_ancestors.contains(&a.id) {
                continue; // one waits for the other, so they never run at once
            }
            for path in a_owned.intersection(b_owned) {
                out.push(format!(
                    "plan: tasks {} and {} can run at once and both claim {path} -- give one an [after:] on the other",
                    a.id, b.id
                ));
            }
        }
    }
    out
}

fn runs_tests(verify: &str) -> bool {
    verify.contains("test")
}

/// A Done that names a test as a deliverable -- "tests for each", "a spec
/// per screen" -- whole-word, so "latest" or "contest" say nothing
/// (friction #8KNJHX6M: a Done demanded tests, Files claimed no test file,
/// and the run failed the worker for writing them).
fn done_asks_test(done: &str) -> bool {
    done.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "test" | "tests" | "spec" | "specs"
        )
    })
}

/// A Files pattern naming "test" is the common case; the other shape cargo
/// finds without a file of its own is an inline `#[cfg(test)]` module in a
/// tracked `.rs` file the patterns already expand to (ruling 9, wiki
/// review-2026-09 defect 9).
fn has_test_file(git: &Git, patterns: &[String]) -> bool {
    if patterns.iter().any(|p| p.to_lowercase().contains("test")) {
        return true;
    }
    let rs_files: Vec<String> = patterns
        .iter()
        .flat_map(|p| zlines(&git.bytes(&["ls-files", "-z", "--", &gitcmd::glob_top(p)])))
        .filter(|f| f.ends_with(".rs"))
        .collect();
    if rs_files.is_empty() {
        return false;
    }
    let mut args: Vec<&str> = vec![
        "grep",
        "-l",
        "-z",
        "-F",
        "-e",
        "#[cfg(test)]",
        "-e",
        "#[test]",
        "--",
    ];
    args.extend(rs_files.iter().map(String::as_str));
    !git.bytes(&args).is_empty()
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

/// The words a block uses to point at another task rather than name what it
/// gives: `as in t1`, `same as t1`, `see t1`, `like t1`, `per t1`, `from
/// t1`, `task t1`, `milestone m1`, and the possessive `t1's`. `in t1` and
/// `of t1` are left out: ids are words too, and "in brief" is English.
const POINTER_WORDS: [&str; 12] = [
    "as in",
    "same as",
    "like",
    "see",
    "per",
    "from",
    "mirrors",
    "mirroring",
    "matching",
    "following",
    "task",
    "milestone",
];

/// Every place `text` points at one of `ids` -- a whole-word id after a
/// pointer word, or with `'s` on it -- as the id and the phrase as written,
/// so the warning can quote it. An id is a word of [a-z0-9-], so a match
/// stops at anything else on both sides; the pointer word must be a whole
/// word too, or `unlike t1` would read as `like t1`.
fn points_at(text: &str, ids: &[String]) -> Vec<(String, String)> {
    let lower = text.to_lowercase();
    let is_id_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    let mut found = Vec::new();
    for id in ids {
        let mut from = 0;
        while let Some(at) = lower[from..].find(id.as_str()) {
            let start = from + at;
            let end = start + id.len();
            from = end;
            let before_ok = lower[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_id_char(c));
            let after_ok = lower[end..].chars().next().is_none_or(|c| !is_id_char(c));
            if !before_ok || !after_ok {
                continue;
            }
            if lower[end..].starts_with("'s") {
                found.push((id.clone(), format!("{id}'s")));
                continue;
            }
            let before = lower[..start].trim_end();
            let pointer = POINTER_WORDS.iter().find(|w| {
                before.ends_with(*w)
                    && before[..before.len() - w.len()]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_alphanumeric())
            });
            if let Some(w) = pointer {
                found.push((id.clone(), format!("{w} {id}")));
            }
        }
    }
    found
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

/// The ` · `-separated items of a Uses or Gives line, each with its
/// whitespace folded to single spaces, so two spellings compare as the
/// worker reads them and not as the author wrapped them.
fn gives_items(line: &str) -> Vec<String> {
    line.split(" · ")
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|item| !item.is_empty())
        .collect()
}

/// The Uses item that `uses_items` reduced to this identifier, spelled as
/// the line spells it, so it can be held against a Gives item for item.
fn item_spelled(uses: &str, ident: &str) -> Option<String> {
    gives_items(uses)
        .into_iter()
        .find(|item| ident_of(&without_locations(item)).as_deref() == Some(ident))
}

/// The type a Gives item defines, if it defines one: its first identifier
/// past any declaration keyword when that identifier is capitalised.
/// `Outcome { stop: Stop }` defines `Outcome`, `Stop::{Done, Budget}` gives
/// `Stop` its variants, `trait ToolHost { .. }` and `type Shared = ..`
/// define what they name, `Session::new(..)` stands for `Session`, and
/// `scrub(text) -> Scrubbed` defines nothing by this rule -- `Scrubbed` is
/// its own item's to define.
fn defined_type(item: &str) -> Option<String> {
    const KEYWORDS: [&str; 12] = [
        "fn", "pub", "struct", "enum", "class", "function", "def", "let", "const", "type", "impl",
        "trait",
    ];
    // `Receipt.cache: CacheLabel::{Verified, Unverifiable}` defines the enum
    // whose variants it lists, wherever in the item that list sits.
    if let Some(at) = item.find("::{") {
        let name: String = item[..at]
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Some(name);
        }
    }
    item.split(|c: char| !c.is_alphanumeric() && c != '_')
        .find(|t| !t.is_empty() && !KEYWORDS.contains(t))
        .filter(|t| t.starts_with(|c: char| c.is_ascii_uppercase()))
        .map(str::to_string)
}

/// The capitalised identifiers inside an item that stand for types it uses
/// without defining: everything but the item's own defined name, constants
/// spelled in capitals, names the languages' libraries own, and a name the
/// item reaches through a lowercase path (`serde_json::Value`,
/// `tokio::sync::broadcast::Receiver`), which says where it lives.
fn type_tokens(item: &str) -> Vec<String> {
    const LIBRARY: [&str; 46] = [
        "Result",
        "Option",
        "Vec",
        "String",
        "Box",
        "Arc",
        "Rc",
        "Mutex",
        "RwLock",
        "Path",
        "PathBuf",
        "Duration",
        "Instant",
        "SystemTime",
        "HashMap",
        "HashSet",
        "BTreeMap",
        "BTreeSet",
        "VecDeque",
        "Value",
        "Self",
        "Send",
        "Sync",
        "Sized",
        "Fn",
        "FnMut",
        "FnOnce",
        "Iterator",
        "Future",
        "Stream",
        "Read",
        "Write",
        "AsyncRead",
        "AsyncWrite",
        "Clone",
        "Debug",
        "Default",
        "Display",
        "Promise",
        "Array",
        "Record",
        "Map",
        "Set",
        "Date",
        "Error",
        "Closure",
    ];
    let own = defined_type(item);
    // `Stop::{Done, Budget(BudgetKind)}` lists variants at the first brace
    // depth and the types they carry inside their parens; a plain
    // `Outcome { stop: Stop }` lists field types at that depth instead.
    let variants_at_one = item.contains("::{");
    let mut out: Vec<String> = Vec::new();
    let mut braces = 0usize;
    let mut parens = 0usize;
    // A capitalised word after an article is prose inside the item -- "as a
    // User entry", "the Fast results" -- not a type the worker must find.
    const ARTICLES: [&str; 12] = [
        "a", "an", "the", "as", "every", "each", "one", "any", "its", "this", "that", "no",
    ];
    let mut tok = String::new();
    let mut prev_tok = String::new(); // the identifier before this one
    let mut prev_sep: Option<&str> = None; // what stood right before the token
    let mut tail = String::new(); // the non-identifier run since the last token
    // A trailing space closes the last token the way any separator would.
    for c in item.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            if tok.is_empty() {
                prev_sep = None;
                if tail.ends_with("::") {
                    prev_sep = Some("::");
                } else if tail.chars().all(char::is_whitespace)
                    && ARTICLES.contains(&prev_tok.as_str())
                {
                    prev_sep = Some("article");
                }
            }
            tok.push(c);
            continue;
        }
        if !tok.is_empty() {
            let is_type = tok.starts_with(|c: char| c.is_ascii_uppercase())
                && tok.chars().any(|c| c.is_ascii_lowercase());
            let member = prev_sep == Some("::");
            let prose = prev_sep == Some("article");
            let variant = variants_at_one && braces == 1 && parens == 0;
            let owned = tok.ends_with("Error") || tok.ends_with("Exception");
            if is_type
                && !member
                && !prose
                && !variant
                && !owned
                && !LIBRARY.contains(&tok.as_str())
                && own.as_deref() != Some(tok.as_str())
                && !out.contains(&tok)
            {
                out.push(tok.clone());
            }
            prev_tok = std::mem::take(&mut tok);
            tail.clear();
        }
        tail.push(c);
        match c {
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            '(' | '<' => parens += 1,
            ')' | '>' => parens = parens.saturating_sub(1),
            _ => {}
        }
    }
    out
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

    /// A block points at another task only through a pointer word or a
    /// possessive; an id that is also a word, mentioned bare, is prose.
    #[test]
    fn a_block_points_at_a_task_through_a_pointer_word_or_a_possessive() {
        let ids = vec!["t1".to_string(), "brief".to_string(), "m1-auth".to_string()];
        let phrases = |text: &str| -> Vec<String> {
            points_at(text, &ids).into_iter().map(|(_, p)| p).collect()
        };
        assert_eq!(phrases("the same shape as in t1"), vec!["as in t1"]);
        assert_eq!(phrases("Same as T1, for the reader"), vec!["same as t1"]);
        assert_eq!(phrases("reuse t1's helper"), vec!["t1's"]);
        assert_eq!(
            phrases("the sessions milestone m1-auth made"),
            vec!["milestone m1-auth"]
        );
        assert!(phrases("the brief carries the answer in brief").is_empty());
        assert!(phrases("unlike t1, this one waits").is_empty());
        assert!(phrases("see t10 for the shape").is_empty());
        assert!(phrases("the pretty-brief file").is_empty());
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

    /// A Uses item is held against a Gives item for item: the same symbol
    /// spelled another way -- a variant out of an enum given whole, a
    /// signature that grew a return type -- is what a worker hunts for
    /// (friction #J0RQN6WY).
    #[test]
    fn a_uses_item_is_spelled_as_its_line_spells_it() {
        let uses = "EntryKind::FileChange · Registry::register(&mut self,   tool: Box<dyn Tool>)";
        assert_eq!(
            item_spelled(uses, "FileChange").as_deref(),
            Some("EntryKind::FileChange")
        );
        // Whitespace folds, so a wrapped line and a flat one agree.
        assert_eq!(
            item_spelled(uses, "register").as_deref(),
            Some("Registry::register(&mut self, tool: Box<dyn Tool>)")
        );
        assert_eq!(item_spelled(uses, "nothing"), None);
        assert_eq!(gives_items(" a · b  c ·  · d "), vec!["a", "b c", "d"]);
    }

    /// A Gives item defines the capitalised name it opens with, past any
    /// declaration keyword; a lowercase opener defines nothing.
    #[test]
    fn a_gives_item_defines_the_type_it_opens_with() {
        assert_eq!(
            defined_type("Outcome { stop: Stop, receipt: Receipt }").as_deref(),
            Some("Outcome")
        );
        assert_eq!(
            defined_type("Stop::{EndTurn, Budget(BudgetKind)}").as_deref(),
            Some("Stop")
        );
        assert_eq!(
            defined_type("trait ToolHost { fn defs(&self) -> Vec<ToolDef>; }").as_deref(),
            Some("ToolHost")
        );
        assert_eq!(
            defined_type("type SharedLedger = Arc<Mutex<Ledger>>").as_deref(),
            Some("SharedLedger")
        );
        assert_eq!(
            defined_type("Session::new(dir: &Path) -> Session").as_deref(),
            Some("Session")
        );
        assert_eq!(
            defined_type("Receipt.cache: CacheLabel::{Verified { hit_rate: f64 }, Unverifiable}")
                .as_deref(),
            Some("CacheLabel")
        );
        assert_eq!(defined_type("scrub(text: &str) -> Scrubbed"), None);
        assert_eq!(defined_type("eco run --json <prompt>"), None);
        assert_eq!(defined_type("the closing section"), None);
    }

    /// The types an item uses without defining: field and argument types,
    /// return types, what a variant carries. Not its own name, not a variant
    /// of its own enum, not a member reached through `::`, not a library
    /// name, not an error type, not a constant in capitals.
    #[test]
    fn type_tokens_are_the_names_an_item_leans_on() {
        assert_eq!(
            type_tokens("Outcome { stop: Stop, receipt: Receipt }"),
            vec!["Stop", "Receipt"]
        );
        assert_eq!(
            type_tokens(
                "Stop::{EndTurn, Budget(BudgetKind), Breaker(BreakerCause), Error(String)}"
            ),
            vec!["BudgetKind", "BreakerCause"]
        );
        assert_eq!(
            type_tokens(
                "Event::{Text(String), Thinking { text: String, signature: Option<String> }, Stop(StopReason)}"
            ),
            vec!["StopReason"]
        );
        assert_eq!(
            type_tokens(
                "Session::new(dir: &Path, gen: Box<dyn Generator>, budget: Budget) -> Result<Session, SessionError>"
            ),
            vec!["Generator", "Budget"]
        );
        assert_eq!(
            type_tokens(
                "Session::subscribe(&self) -> tokio::sync::broadcast::Receiver<Notification>"
            ),
            vec!["Notification"]
        );
        assert_eq!(
            type_tokens("CheckOutcome::Flaky { passes: u8, runs: u8 } added to CheckOutcome"),
            Vec::<String>::new()
        );
        assert_eq!(
            type_tokens(
                "trait Generator { fn stream(&self, req: Request) -> BoxStream<'static, Result<Event, ProviderError>>; }"
            ),
            vec!["Request", "BoxStream", "Event"]
        );
        assert_eq!(
            type_tokens("DEFAULT_MODEL · RPC request eco/verify/contract"),
            Vec::<String>::new()
        );
        assert_eq!(
            type_tokens("CartPricing::price(Basket $b): Cents"),
            vec!["Basket", "Cents"]
        );
        // Prose inside an item: a capitalised word after an article is a
        // word, and the enum a `::{` list belongs to is defined, not used.
        assert_eq!(
            type_tokens(
                "Perms::reject_cascade(&mut self, reason: &str) -> String (the steering line the dispatcher appends as a User entry)"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            type_tokens("Receipt.cache: CacheLabel::{Verified { hit_rate: f64 }, Unverifiable}"),
            vec!["Receipt"]
        );
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
    fn done_asks_test_is_whole_word() {
        assert!(done_asks_test("tests for each screen"));
        assert!(done_asks_test(
            "a spec per handler and every caller migrated"
        ));
        assert!(!done_asks_test("the latest contest total is identical"));
        assert!(!done_asks_test(""));
    }

    #[test]
    fn a_test_run_is_recognised_by_the_word_and_true_is_not() {
        assert!(runs_tests("cargo test --test e2e"));
        assert!(runs_tests("pnpm run test:unit"));
        assert!(runs_tests("bin/php artisan test --filter=Cart"));
        assert!(!runs_tests("true"));
        assert!(!runs_tests("cargo build"));
    }

    /// A Files pattern naming "test" already covers most tasks; an inline
    /// `#[cfg(test)]` module is the other shape cargo finds without a file
    /// of its own (ruling 9, wiki review-2026-09 defect 9).
    #[test]
    fn an_inline_test_module_counts_as_a_test_file() {
        let git = Git::at(env!("CARGO_MANIFEST_DIR"));
        assert!(has_test_file(
            &git,
            &["workflow/src/plancheck.rs".to_string()]
        ));
        assert!(!has_test_file(
            &git,
            &["workflow/src/gitcmd.rs".to_string()]
        ));
        assert!(has_test_file(
            &git,
            &["src/never-created-eee-test.rs".to_string()]
        ));
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

    /// Two tasks with no `[after:]` path between them, either way, are
    /// refused for sharing a file even when a wave would have kept them
    /// apart: t2 and t3 share no ordering even though t1 and t2 do, so the
    /// pair test is ancestry, not level (ruling 5 and the t040 half of
    /// ruling 6, wiki review-2026-09 defects 1 and 2).
    #[test]
    fn concurrent_tasks_sharing_a_file_are_refused_a_chained_pair_is_not() {
        let git = Git::at(env!("CARGO_MANIFEST_DIR"));
        let text = "\
# plan: p

- [ ] t1 first
      Files: src/never-created-aaa.rs
      Verify: true
- [ ] t2 second
      Files: src/never-created-aaa.rs
      Verify: true
";
        let plan = crate::plan::parse(text, true).expect("the plan parses");
        let out = concurrent_claims(&plan, &git);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("t1 and t2"), "{out:?}");

        let text = "\
# plan: p

- [ ] t1 first
      Files: src/never-created-bbb.rs
      Verify: true
- [ ] t2 second  [after: t1]
      Files: src/never-created-ccc.rs
      Verify: true
- [ ] t3 third
      Files: src/never-created-ccc.rs
      Verify: true
";
        let plan = crate::plan::parse(text, true).expect("the plan parses");
        let out = concurrent_claims(&plan, &git);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("t2 and t3"), "{out:?}");

        let text = "\
# plan: p

- [ ] t1 first
      Files: src/never-created-ddd.rs
      Verify: true
- [ ] t2 second  [after: t1]
      Files: src/never-created-ddd.rs
      Verify: true
";
        let plan = crate::plan::parse(text, true).expect("the plan parses");
        assert!(concurrent_claims(&plan, &git).is_empty());
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

    /// A file naming the data file's basename in passing is not asserting on
    /// its path, and used to warn on every such mention repo-wide (friction
    /// #1TVAS5X9). The exact tracked path is a narrower claim than a
    /// basename shared with anything else in the tree.
    #[test]
    fn data_file_asserted_greps_the_tracked_path_not_the_basename() {
        let dir = std::env::temp_dir().join(format!(
            "workflow-plancheck-data-file-asserted-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).expect("make the fixture dir");
        std::fs::write(dir.join("assets/config.toml"), "value = 1\n").expect("write the data file");
        std::fs::write(dir.join("caller.rs"), "// reads assets/config.toml\n")
            .expect("write the full-path caller");
        std::fs::write(dir.join("prose.rs"), "// mentions config.toml in passing\n")
            .expect("write the basename-only caller");
        let git = Git::at(&dir);
        git.capture(&["init", "-q"]);
        git.capture(&["add", "-A"]);

        let files = vec!["assets/config.toml".to_string()];
        let out = data_file_asserted("t1", &files, &git);
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            out.iter().any(|w| w.contains("caller.rs")),
            "a file naming the full tracked path still warns: {out:?}"
        );
        assert!(
            !out.iter().any(|w| w.contains("prose.rs")),
            "a file naming only the basename no longer warns: {out:?}"
        );
    }
}
