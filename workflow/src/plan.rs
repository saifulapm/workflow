//! The plan grammar of spec §5.3, and the Kahn waves `run` executes.
//!
//! Every hard error is reported before the parse gives up, so one run tells the
//! author everything wrong with the file; unknown keys are warnings.

use serde::Serialize;

use crate::warn;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub deps: Vec<String>,
    pub files: Option<String>,
    pub verify: Option<String>,
    pub done: Option<String>,
    /// Paths the worker reads before editing anything.
    pub read: Option<String>,
    /// Interfaces consumed from other tasks or the tree, exact signatures.
    pub uses: Option<String>,
    /// Interfaces this task produces that other tasks rely on.
    pub gives: Option<String>,
    /// One analog to copy the shape of: `path` or `path:12-25`.
    pub pattern: Option<String>,
    pub checked: bool,
    /// The task's own lines, verbatim: the brief quotes them back.
    #[serde(skip)]
    pub block: String,
}

impl Task {
    /// The `wiki:<slug>` names on this task's Read: line, in the order they
    /// were written, each once. Neither a checkout path nor a refusal, since
    /// a page lives in mem and never in the tree a plan-check or a worker
    /// walks (ruling 1 of m1-wiki-first).
    pub fn wiki_slugs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for item in self.read.as_deref().unwrap_or("").split_whitespace() {
            if let Some(slug) = item.strip_prefix("wiki:")
                && !slug.is_empty()
                && !out.iter().any(|s| s == slug)
            {
                out.push(slug.to_string());
            }
        }
        out
    }
}

/// What the first line says the document is. A plan is a wave of worker tasks;
/// a roadmap is a wave of milestones, each naming a stored plan of its own, so
/// its items carry no Files: or Verify: and their ids are plan slugs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanKind {
    #[default]
    Plan,
    Roadmap,
}

impl PlanKind {
    pub fn word(self) -> &'static str {
        match self {
            PlanKind::Plan => "plan",
            PlanKind::Roadmap => "roadmap",
        }
    }

    /// A task id names a branch and a worktree directory and is typed by hand
    /// into a status line; a milestone id is the slug of a stored plan, which
    /// mem allows sixty-four characters of.
    fn max_id(self) -> usize {
        match self {
            PlanKind::Plan => 16,
            PlanKind::Roadmap => 64,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Plan {
    pub plan_id: String,
    pub kind: PlanKind,
    pub tasks: Vec<Task>,
    pub waves: Vec<Vec<String>>,
}

impl Plan {
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.tasks.iter().map(|t| t.id.clone()).collect()
    }
}

fn is_sp(c: char) -> bool {
    c.is_ascii_whitespace()
}

/// `^#[[:space:]]*(plan|roadmap):[[:space:]]*([A-Za-z0-9][A-Za-z0-9._-]*)[[:space:]]*$`
fn header(line: &str) -> Option<(PlanKind, String)> {
    let rest = line.strip_prefix('#')?.trim_start_matches(is_sp);
    let (kind, rest) = match rest.strip_prefix("roadmap:") {
        Some(rest) => (PlanKind::Roadmap, rest),
        None => (PlanKind::Plan, rest.strip_prefix("plan:")?),
    };
    let rest = rest.trim_start_matches(is_sp);
    let slug: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    if slug.is_empty() || !slug.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !rest[slug.len()..].trim_end_matches(is_sp).is_empty() {
        return None;
    }
    Some((kind, slug))
}

/// `^- \[([ xX])\] ([a-z0-9][a-z0-9-]{0,max-1}) (.+)$`
///
/// The id run cannot contain a space, so the only place the title can start is
/// straight after it: a run longer than the ids of this document are allowed to
/// be is not a task line, it is a mistake, and the caller says so out loud.
///
/// The tick is the one part of the line that reads the same in either case:
/// `- [X]` is a ticked box everywhere markdown is rendered, and unlike an id --
/// which names a branch and a directory -- its case means nothing to anything
/// downstream. Ids stay case sensitive; the box does not.
fn task_line(line: &str, max_id: usize) -> Option<(bool, String, String)> {
    let rest = line.strip_prefix("- [")?;
    let mut chars = rest.chars();
    let checked = match chars.next()? {
        ' ' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    let rest = rest[1..].strip_prefix("] ")?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();
    if id.is_empty() || id.len() > max_id || id.starts_with('-') {
        return None;
    }
    let after = &rest[id.len()..];
    let title = after.strip_prefix(' ')?;
    if title.is_empty() {
        return None;
    }
    Some((checked, id, title.to_string()))
}

/// `^(.*[^[:space:]])[[:space:]]+\[after: ([^]]*)\][[:space:]]*$` -- the last
/// `[after:` on the line wins, and the title is what comes before it.
fn split_after(rest: &str) -> (String, String) {
    let s = rest.trim_end_matches(is_sp);
    let Some(stripped) = s.strip_suffix(']') else {
        return (s.to_string(), String::new());
    };
    let Some(at) = stripped.rfind("[after:") else {
        return (s.to_string(), String::new());
    };
    let deps = &stripped[at + "[after:".len()..];
    if deps.contains(']') {
        return (s.to_string(), String::new());
    }
    let head = &s[..at];
    if !head.ends_with(is_sp) || head.trim_end_matches(is_sp).is_empty() {
        return (s.to_string(), String::new());
    }
    (head.trim_end_matches(is_sp).to_string(), deps.to_string())
}

/// `- [` + one character + `]`: every way a line can open a checkbox, including
/// the spellings the grammar refuses. A line that opens one and is not a task is
/// a mistake rather than prose, so the parser has to say so -- see the guard in
/// `parse`. A bullet list item that really does start `- [1]` is refused here
/// too, which is the price of catching the ones that meant to be tasks.
fn opens_a_checkbox(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- [") else {
        return false;
    };
    let mut chars = rest.chars();
    chars.next().is_some() && chars.next() == Some(']')
}

/// Indented by two or more, and not blank: `^[[:space:]][[:space:]]+[^[:space:]]`
fn is_continuation(line: &str) -> bool {
    let indent = line.chars().take_while(|c| is_sp(*c)).count();
    indent >= 2 && !line.trim_start_matches(is_sp).is_empty()
}

/// `require_files`: the plan lane insists on `Files:` and `Verify:` (spec §5.3).
/// A roadmap never does: a milestone is a plan to be cut later, not work a
/// worker can be handed.
/// What the planner wrote between the title and the first task: the why,
/// the rulings, whatever the spec holds. Verbatim, trimmed, and empty for a
/// plan that goes straight to its tasks. The reader at the merge gate holds
/// a diff to the rulings in here, so the worker's brief carries them too;
/// the tasks themselves are not part of it, since a worker sees its own
/// block and restates in Uses what a sibling gives.
pub fn prose(text: &str) -> String {
    let mut seen_header = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if !seen_header {
            seen_header = header(line).is_some();
            continue;
        }
        if task_line(line, usize::MAX).is_some() {
            break;
        }
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

pub fn parse(text: &str, require_files: bool) -> Option<Plan> {
    let mut plan = Plan::default();
    let mut seen_header = false;
    let mut cur: Option<usize> = None;
    let mut rc = true;

    for (i, line) in text.lines().enumerate() {
        let n = i + 1;

        if !seen_header {
            if line.trim().is_empty() {
                continue;
            }
            match header(line) {
                Some((kind, slug)) => {
                    // The slug names branches and directories, so it has to be
                    // something git will accept as a ref component.
                    if slug.contains("..") || slug.ends_with(".lock") || slug.ends_with('.') {
                        warn(format!("plan: '{slug}' cannot be a branch name"));
                        return None;
                    }
                    plan.plan_id = slug;
                    plan.kind = kind;
                    seen_header = true;
                    continue;
                }
                None => {
                    warn(format!(
                        "plan: line {n}: the first line must be '# plan: <slug>' or '# roadmap: <slug>'"
                    ));
                    return None;
                }
            }
        }

        if let Some((checked, id, rest)) = task_line(line, plan.kind.max_id()) {
            if plan.get(&id).is_some() {
                warn(format!("plan: line {n}: task id '{id}' appears twice"));
                rc = false;
            }
            let (title, deps) = split_after(&rest);
            let deps: Vec<String> = deps
                .split(',')
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty())
                .collect();
            plan.tasks.push(Task {
                id,
                title: title.trim().to_string(),
                deps,
                checked,
                block: format!("{line}\n"),
                ..Task::default()
            });
            cur = Some(plan.tasks.len() - 1);
            continue;
        }

        // A line that means to be a task and is not one. Read as prose it takes
        // its own Files: and Verify: down with it -- the fall-through below
        // forgets which task is current -- so a plan could read as complete
        // while a task of it never existed. The test is every checkbox opening
        // rather than the two the grammar accepts: `- [X]` falling through is
        // exactly that failure, one spelling further out.
        if opens_a_checkbox(line) {
            warn(format!("plan: line {n}: this is not a task line: {line}"));
            warn(format!(
                "plan: the box is [ ] or [x], and the id is 1-{} characters of a-z, 0-9 and -, starting with a letter or digit",
                plan.kind.max_id()
            ));
            rc = false;
            cur = None;
            continue;
        }

        if let Some(idx) = cur
            && is_continuation(line)
        {
            plan.tasks[idx].block.push_str(line);
            plan.tasks[idx].block.push('\n');
            let body = line.trim_start_matches(is_sp);
            let Some(cut) = body.find(": ") else {
                warn(format!(
                    "plan: line {n}: continuation lines are 'Key: value'; ignoring"
                ));
                continue;
            };
            let key = &body[..cut];
            let value = body[cut + 2..].trim().to_string();
            let id = plan.tasks[idx].id.clone();
            let slot = match key {
                "Files" => &mut plan.tasks[idx].files,
                "Verify" => &mut plan.tasks[idx].verify,
                "Done" => &mut plan.tasks[idx].done,
                "Read" => &mut plan.tasks[idx].read,
                "Uses" => &mut plan.tasks[idx].uses,
                "Gives" => &mut plan.tasks[idx].gives,
                "Pattern" => &mut plan.tasks[idx].pattern,
                other => {
                    warn(format!("plan: line {n}: unknown key '{other}' ignored"));
                    continue;
                }
            };
            if slot.is_some() {
                warn(format!("plan: line {n}: task {id} has two {key}: lines"));
                rc = false;
            }
            *slot = Some(value);
            continue;
        }

        cur = None;
    }

    if !seen_header {
        warn("plan: no '# plan: <slug>' or '# roadmap: <slug>' header");
        return None;
    }
    if plan.tasks.is_empty() {
        warn("plan: no tasks");
        return None;
    }

    let known = plan.ids();
    for t in &plan.tasks {
        for d in &t.deps {
            if !known.contains(d) {
                warn(format!(
                    "plan: task {} waits for '{d}', which is not a task in this plan",
                    t.id
                ));
                rc = false;
            }
        }
        if require_files && plan.kind == PlanKind::Plan {
            if t.files.as_deref().unwrap_or("").is_empty() {
                warn(format!("plan: task {} has no Files: line", t.id));
                rc = false;
            }
            if t.verify.as_deref().unwrap_or("").is_empty() {
                warn(format!("plan: task {} has no Verify: line", t.id));
                rc = false;
            }
        }
    }
    if !rc {
        return None;
    }

    plan.waves = waves(&plan)?;
    Some(plan)
}

/// The plan text with one task's box ticked, or `None` when no task line
/// carries that id. Every other byte is left as the author wrote it: the file
/// is their document, not this program's scratch space.
///
/// The box written is `- [x]`, the spelling `mem plan --tick` writes, so a
/// plan ticked by either of them reads the same. The parser still takes `[X]`
/// from anyone who writes it.
pub fn tick(text: &str, id: &str) -> Option<String> {
    let max_id = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .and_then(header)
        .map_or(PlanKind::default(), |(kind, _)| kind)
        .max_id();
    let mut found = false;
    let out: String = text
        .split_inclusive('\n')
        .map(
            |line| match task_line(line.trim_end_matches(['\n', '\r']), max_id) {
                // `- [x]` is five ASCII bytes however the box is spelled, so the
                // tail slices cleanly whatever the title holds.
                Some((_, tid, _)) if tid == id => {
                    found = true;
                    format!("- [x]{}", &line[5..])
                }
                _ => line.to_string(),
            },
        )
        .collect();
    found.then_some(out)
}

/// Kahn levels. A cycle is a hard error.
fn waves(plan: &Plan) -> Option<Vec<Vec<String>>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut done: Vec<String> = Vec::new();
    let mut indeg: Vec<usize> = plan.tasks.iter().map(|t| t.deps.len()).collect();

    while done.len() < plan.tasks.len() {
        let wave: Vec<String> = plan
            .tasks
            .iter()
            .enumerate()
            .filter(|(i, t)| !done.contains(&t.id) && indeg[*i] == 0)
            .map(|(_, t)| t.id.clone())
            .collect();
        if wave.is_empty() {
            let left: Vec<String> = plan
                .tasks
                .iter()
                .filter(|t| !done.contains(&t.id))
                .map(|t| t.id.clone())
                .collect();
            warn(format!(
                "plan: these tasks wait on each other in a circle: {}",
                left.join(" ")
            ));
            return None;
        }
        done.extend(wave.iter().cloned());
        for (i, t) in plan.tasks.iter().enumerate() {
            if done.contains(&t.id) {
                continue;
            }
            for d in &t.deps {
                if wave.contains(d) {
                    indeg[i] -= 1;
                }
            }
        }
        out.push(wave);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prose_is_what_sits_between_the_title_and_the_first_task() {
        let text = "\
# plan: p

## Why

Totals drift.

## Rulings

- Ruling 1. Cents, never floats.

- [x] t0 Freeze the fixture
      Files: a
      Verify: true
- [ ] t1 Do it
      Files: b
      Verify: true
";
        assert_eq!(
            prose(text),
            "## Why\n\nTotals drift.\n\n## Rulings\n\n- Ruling 1. Cents, never floats."
        );
        assert_eq!(prose("# plan: p\n\n- [ ] t1 Do it\n      Files: a\n"), "");
        assert_eq!(prose(""), "");
        // A roadmap's prose is read the same way.
        assert_eq!(
            prose("# roadmap: r\n\nThe shape.\n\n- [ ] m1 First\n"),
            "The shape."
        );
    }

    const EXAMPLE: &str = "\
# plan: cart-pricing-v2

- [x] t0 Freeze the fixture basket
      Files: tests/Fixtures/basket.json
      Verify: bin/php artisan test --filter=Fixture

- [ ] t1 Extract cart pricing into a service  [after: t0]
      Files: app/Services/Cart*.php tests/Unit/Cart*
      Verify: bin/php artisan test --filter=Cart
      Done: cart totals identical for the fixture basket
";

    #[test]
    fn the_example_round_trips() {
        let p = parse(EXAMPLE, true).expect("the example parses");
        assert_eq!(p.plan_id, "cart-pricing-v2");
        assert_eq!(p.kind, PlanKind::Plan);
        assert_eq!(p.ids(), vec!["t0", "t1"]);
        let t1 = p.get("t1").unwrap();
        assert_eq!(t1.title, "Extract cart pricing into a service");
        assert_eq!(t1.deps, vec!["t0"]);
        assert_eq!(
            t1.files.as_deref(),
            Some("app/Services/Cart*.php tests/Unit/Cart*")
        );
        assert_eq!(
            t1.verify.as_deref(),
            Some("bin/php artisan test --filter=Cart")
        );
        assert_eq!(
            t1.done.as_deref(),
            Some("cart totals identical for the fixture basket")
        );
        assert!(p.get("t0").unwrap().checked);
        assert!(!t1.checked);
        assert_eq!(p.waves, vec![vec!["t0"], vec!["t1"]]);
        assert!(t1.block.contains("Files: app/Services/Cart*.php"));
    }

    #[test]
    fn a_colon_in_the_value_survives_the_split() {
        let p = parse(
            "# plan: p\n\n- [ ] t1 Do a thing\n      Files: src/**\n      Verify: pnpm run test:unit\n",
            true,
        )
        .unwrap();
        assert_eq!(
            p.get("t1").unwrap().verify.as_deref(),
            Some("pnpm run test:unit")
        );
    }

    #[test]
    fn dependencies_are_comma_separated_and_trimmed() {
        let p = parse(
            "# plan: p\n\n- [ ] a First\n      Files: a\n      Verify: true\n\
             - [ ] b Second\n      Files: b\n      Verify: true\n\
             - [ ] c Third [after: a, b]\n      Files: c\n      Verify: true\n",
            true,
        )
        .unwrap();
        assert_eq!(p.get("c").unwrap().deps, vec!["a", "b"]);
        assert_eq!(p.waves.len(), 2);
    }

    #[test]
    fn every_hard_error_of_the_grammar_is_refused() {
        for (name, text) in [
            (
                "no header",
                "- [ ] t1 No header at all\n      Files: x\n      Verify: true\n",
            ),
            (
                "unknown dependency",
                "# plan: p\n\n- [ ] t1 Nope [after: nope]\n      Files: x\n      Verify: true\n",
            ),
            (
                "cycle",
                "# plan: p\n\n- [ ] a One [after: b]\n      Files: x\n      Verify: true\n\
                 - [ ] b Two [after: a]\n      Files: y\n      Verify: true\n",
            ),
            (
                "no Verify",
                "# plan: p\n\n- [ ] t1 Missing its verifier\n      Files: x\n",
            ),
            (
                "no Files",
                "# plan: p\n\n- [ ] t1 Missing its files\n      Verify: true\n",
            ),
            (
                "duplicate key",
                "# plan: p\n\n- [ ] t1 Two of a kind\n      Files: x\n      Files: y\n      Verify: true\n",
            ),
            (
                "id past sixteen characters",
                "# plan: p\n\n- [ ] this-id-is-far-too-long-for-the-rule Do it\n      Files: x\n      Verify: true\n",
            ),
            (
                "uppercase id",
                "# plan: p\n\n- [ ] T1 Do the thing\n      Files: x\n      Verify: true\n",
            ),
            (
                "no id at all",
                "# plan: p\n\n- [ ] Missing an id entirely\n      Files: y\n      Verify: true\n",
            ),
            ("no tasks", "# plan: p\n\nJust prose.\n"),
            (
                "a slug git would refuse",
                "# plan: bad..slug\n\n- [ ] t1 A thing\n      Files: x\n      Verify: true\n",
            ),
            (
                "duplicate task id",
                "# plan: p\n\n- [ ] a One\n      Files: x\n      Verify: true\n\
                 - [ ] a Again\n      Files: y\n      Verify: true\n",
            ),
        ] {
            assert!(parse(text, true).is_none(), "{name} should be refused");
        }
    }

    /// A `wiki:<slug>` name on Read: is neither a checkout path nor prose --
    /// it addresses a page in mem, in the order it was written, once each.
    #[test]
    fn wiki_slugs_are_read_off_the_read_line_in_order_and_deduplicated() {
        let mut t = Task {
            read: Some("src/lib.rs wiki:run wiki:merge-gate wiki:run".into()),
            ..Task::default()
        };
        assert_eq!(t.wiki_slugs(), vec!["run", "merge-gate"]);
        t.read = Some("src/lib.rs docs/api.md".into());
        assert!(t.wiki_slugs().is_empty());
        t.read = None;
        assert!(t.wiki_slugs().is_empty());
    }

    #[test]
    fn the_middle_tier_keys_parse_into_their_fields() {
        let p = parse(
            "# plan: p\n\n- [ ] t1 Wire the service\n      Files: src/**\n      Verify: true\n      \
             Read: src/lib.rs docs/api.md\n      \
             Uses: fn price(basket: &Basket) -> Cents\n      \
             Gives: fn total(basket: &Basket) -> Cents\n      \
             Pattern: src/old.rs:12-25\n",
            true,
        )
        .expect("the middle-tier keys parse");
        let t = p.get("t1").unwrap();
        assert_eq!(t.read.as_deref(), Some("src/lib.rs docs/api.md"));
        assert_eq!(
            t.uses.as_deref(),
            Some("fn price(basket: &Basket) -> Cents")
        );
        assert_eq!(
            t.gives.as_deref(),
            Some("fn total(basket: &Basket) -> Cents")
        );
        assert_eq!(t.pattern.as_deref(), Some("src/old.rs:12-25"));
        // The brief quotes the block verbatim, so the keys reach the worker.
        assert!(t.block.contains("Uses: fn price"));
        assert!(t.block.contains("Pattern: src/old.rs:12-25"));
    }

    #[test]
    fn a_duplicate_middle_tier_key_is_refused() {
        for key in ["Read", "Uses", "Gives", "Pattern"] {
            let text = format!(
                "# plan: p\n\n- [ ] t1 Twice over\n      Files: x\n      Verify: true\n      \
                 {key}: a\n      {key}: b\n"
            );
            assert!(
                parse(&text, true).is_none(),
                "two {key}: lines should be refused"
            );
        }
    }

    #[test]
    fn an_unknown_key_is_only_a_warning() {
        let p = parse(
            "# plan: p\n\n- [ ] t1 An unknown key\n      Files: x\n      Verify: true\n      Colour: blue\n",
            true,
        );
        assert!(p.is_some());
    }

    #[test]
    fn a_checkbox_that_is_not_at_the_start_of_a_line_is_prose() {
        let p = parse(
            "# plan: p\n\nNotes: the basket is frozen. - [ ] is not a task here.\n\n\
             - [ ] a Fine\n      Files: x\n      Verify: true\n\
             - [ ] b Also fine\n      Files: y\n      Verify: true\n",
            true,
        )
        .unwrap();
        assert_eq!(p.ids(), vec!["a", "b"]);
    }

    #[test]
    fn the_one_shot_lane_does_not_insist_on_files_and_verify() {
        assert!(parse("# plan: p\n\n- [ ] t1 Just a title\n", false).is_some());
    }

    /// `- [X]` is what GitHub markdown renders as a ticked box, and it is the
    /// one place in the grammar where case carries nothing: an id's case names a
    /// branch and a directory, a tick's does not.
    #[test]
    fn an_uppercase_x_is_a_ticked_box() {
        let p = parse(
            "# plan: p\n\n- [X] t1 Already done\n      Files: x\n      Verify: true\n",
            true,
        )
        .expect("an uppercase X parses");
        assert!(p.get("t1").unwrap().checked);
    }

    /// The failure that reopened M-5, as it actually behaved: `- [X] t2` fell
    /// through as prose and took its `Files:` and `Verify:` with it, so the plan
    /// parsed clean with t2 missing entirely -- the task above kept its own
    /// lines, and nothing anywhere said a task had gone. `require_files` is off
    /// so the drop is what the assertions see rather than a missing-Files error.
    #[test]
    fn a_checkbox_the_grammar_accepts_is_a_task_and_keeps_its_own_lines() {
        let p = parse(
            "# plan: p\n\n- [ ] t1 First\n- [X] t2 Second\n      Files: b\n      Verify: false\n",
            false,
        )
        .unwrap();
        assert_eq!(p.ids(), vec!["t1", "t2"]);
        assert_eq!(p.get("t1").unwrap().files, None);
        assert_eq!(p.get("t1").unwrap().verify, None);
        assert_eq!(p.get("t2").unwrap().files.as_deref(), Some("b"));
        assert_eq!(p.get("t2").unwrap().verify.as_deref(), Some("false"));
    }

    /// Ticking is how a `--plan-file` run records a merge: the file it was
    /// handed is the only copy of that plan, so the run writes the tick back
    /// there rather than leaving the merge recorded nowhere (friction
    /// #2213VV3P).
    ///
    /// The box this writes is lowercase, and t1's `[X]` is left as its author
    /// spelled it: ticking one task is not a licence to restyle the rest of
    /// someone's document.
    #[test]
    fn ticking_marks_one_task_and_leaves_the_rest_of_the_file_alone() {
        let text = "# plan: p\n\nprose about the plan\n\n\
                    - [X] t1 First\n      Files: a\n      Verify: true\n\
                    - [ ] t2 Second [after: t1]\n      Files: b\n      Verify: true\n";
        let out = tick(text, "t2").expect("t2 is in this plan");
        assert!(out.contains("- [x] t2 Second [after: t1]"), "{out}");
        assert!(out.contains("- [X] t1 First"), "t1 must not move: {out}");
        assert!(out.contains("prose about the plan"));
        assert_eq!(out.len(), text.len());
        // Ticking twice says the same thing, and a task the file does not
        // carry says so instead of rewriting it.
        let again = tick(&out, "t2").expect("an already ticked task is still found");
        assert_eq!(again, out);
        assert!(tick(text, "t9").is_none());
    }

    /// A roadmap is the same grammar under a different first line. Its
    /// milestones name stored plans rather than work, so `require_files` does
    /// not reach them, and an id is a plan slug rather than a task id.
    #[test]
    fn a_roadmap_carries_milestones_without_files_or_verify() {
        let text = "# roadmap: workflow-2026\n\n\
                    - [x] mem-stores-the-plans Store the plans in mem\n\
                    - [ ] the-roadmap-header Teach the parser the header [after: mem-stores-the-plans]\n";
        let p = parse(text, true).expect("a roadmap parses");
        assert_eq!(p.kind, PlanKind::Roadmap);
        assert_eq!(p.plan_id, "workflow-2026");
        assert_eq!(p.ids(), vec!["mem-stores-the-plans", "the-roadmap-header"]);
        assert!(p.get("mem-stores-the-plans").unwrap().checked);
        assert_eq!(
            p.get("the-roadmap-header").unwrap().deps,
            vec!["mem-stores-the-plans"]
        );
        assert_eq!(
            p.waves,
            vec![vec!["mem-stores-the-plans"], vec!["the-roadmap-header"]]
        );
        // Ticking reads the ids of the document it was handed, so a milestone
        // id longer than a task's is still found.
        let out = tick(text, "the-roadmap-header").expect("the milestone is in this roadmap");
        assert!(out.contains("- [x] the-roadmap-header"), "{out}");
    }

    #[test]
    fn a_roadmap_refuses_what_a_plan_refuses() {
        for (name, text) in [
            (
                "unknown dependency",
                "# roadmap: r\n\n- [ ] a Waits for nothing that is here [after: nope]\n",
            ),
            (
                "cycle",
                "# roadmap: r\n\n- [ ] a One [after: b]\n- [ ] b Two [after: a]\n",
            ),
            (
                "duplicate id",
                "# roadmap: r\n\n- [ ] a One\n- [ ] a Again\n",
            ),
            ("uppercase id", "# roadmap: r\n\n- [ ] Mem-Stores An id\n"),
        ] {
            assert!(parse(text, true).is_none(), "{name} should be refused");
        }
    }

    /// The id run is the one rule the two kinds spell differently: a task id
    /// names a branch and a worktree, a milestone id is the slug of a stored
    /// plan.
    #[test]
    fn the_id_run_is_sixty_four_in_a_roadmap_and_sixteen_in_a_plan() {
        let id64 = "a".repeat(64);
        let p = parse(
            &format!("# roadmap: r\n\n- [ ] {id64} At the limit\n"),
            true,
        )
        .expect("a 64 character milestone id parses");
        assert_eq!(p.ids(), vec![id64.clone()]);
        assert!(
            parse(&format!("# roadmap: r\n\n- [ ] {id64}a Past it\n"), true).is_none(),
            "65 characters should be refused"
        );

        let id17 = "b".repeat(17);
        assert!(
            parse(
                &format!("# plan: p\n\n- [ ] {id17} Past it\n      Files: x\n      Verify: true\n"),
                true
            )
            .is_none(),
            "a 17 character task id should still be refused in a plan"
        );
    }

    /// Every other way of opening a checkbox is refused outright, so no
    /// continuation of one can reach the task above it either.
    #[test]
    fn a_checkbox_spelling_the_grammar_does_not_accept_is_a_hard_error() {
        for line in [
            "- [-] t2 A dash is not a tick",
            "- [o] t2 Nor is an o",
            "- [X] T2 An uppercase id is still an uppercase id",
            "- [X]t2 The space after the box is not optional",
        ] {
            let text = format!(
                "# plan: p\n\n- [ ] t1 First\n      Files: a\n      Verify: true\n\
                 {line}\n      Files: b\n      Verify: false\n"
            );
            assert!(parse(&text, true).is_none(), "{line} should be refused");
        }
    }
}
