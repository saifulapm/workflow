//! `workflow advise` -- a worker asks a stronger model at a decision point,
//! without ending its turn (spec: Anthropic's Sonnet + Opus advisor). The
//! run resolves who answers the same way it resolves the fix model: an
//! override, a record, a project key, then the reader's own model by the
//! same rungs. A decision -- scope, taste, a broken plan -- stays `mem ask`;
//! this never stops the task, and three consults in one attempt is the
//! limit before the fourth is one.

use std::path::{Path, PathBuf};

use crate::backend::{Consult, Dispatch, Ending, WorkerBackend};
use crate::{exit, gitcmd::Git, memcli, plan, repo, reviewer, run, sys, warn};

/// Past this many bytes a named file is named but not inlined, the same cap
/// `read.rs` holds an untracked file to.
const FILE_CAP: usize = 24 * 1024;

/// Consults an attempt may spend before the fourth is turned back to a
/// `mem ask` (ruling 1).
const CONSULT_LIMIT: u64 = 3;

/// One task's field in a run dir: `<dir>/<task>.<key>`, its value plus a
/// newline. A failed write means the consult count silently stops
/// advancing, so it warns the way `run.rs`'s own `write_field` does.
fn write_field(dir: &Path, task: &str, key: &str, value: &str) {
    let path = dir.join(format!("{task}.{key}"));
    if let Err(e) = std::fs::write(&path, format!("{value}\n")) {
        warn(format!(
            "task {task}: cannot write {} ({e}) -- this run's account of it is now unreliable",
            path.display()
        ));
    }
}

/// The advisor's prompt, written where the dispatch hands it over. A failed
/// write dispatches a session against a brief that is not there, which comes
/// back as no answer after the whole deadline with nothing saying why, so it
/// refuses instead.
fn write_prompt(path: &Path, text: &str) -> bool {
    if let Err(e) = std::fs::write(path, text) {
        warn(format!(
            "advise: cannot write the prompt {} ({e}) -- nothing was dispatched",
            path.display()
        ));
        return false;
    }
    true
}

/// A run dir field, trimmed; `None` when the file is not there.
fn field(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .map(|v| v.trim().to_string())
}

/// One dial's rungs: the environment variable, even set empty (which means
/// nobody); else the run dir's own record, when there is a run dir; else the
/// project's key. `None` all the way through is nobody having decided.
fn dial(
    var: &str,
    run_dir: Option<&Path>,
    record: &str,
    project: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let clean = |v: &str| {
        let v = v.trim();
        (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then(|| v.to_string())
    };
    match std::env::var(var) {
        Ok(v) => clean(&v),
        Err(_) => match run_dir.and_then(|d| field(d, record)) {
            Some(v) => clean(&v),
            None => project(),
        },
    }
}

/// The reader's own model, by its own rungs: what the advisor falls back to
/// when nobody has named one (ruling 1).
fn reader_model(run_dir: Option<&Path>) -> Option<String> {
    dial(
        "WORKFLOW_REVIEW_MODEL",
        run_dir,
        "review-model",
        memcli::project_review_model,
    )
}

fn reader_effort(run_dir: Option<&Path>) -> Option<String> {
    dial(
        "WORKFLOW_REVIEW_EFFORT",
        run_dir,
        "review-effort",
        memcli::project_review_effort,
    )
}

/// `WORKFLOW_ADVISOR`, else the run dir's `advisor` record, else the
/// reader's own model by its own rungs (ruling 1). There is no project rung
/// of its own: mem's project keys are a closed set and `advisor` is not one
/// of them, so a project names its advisor by naming its reader.
fn advisor_model(run_dir: Option<&Path>) -> Option<String> {
    dial("WORKFLOW_ADVISOR", run_dir, "advisor", || None).or_else(|| reader_model(run_dir))
}

/// The first eighty characters of a question, for the log line (ruling 1):
/// cut on a char boundary, never mid character.
fn clip80(text: &str) -> String {
    let mut end = text.len().min(80);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// A fence one backtick longer than the longest run already in `text`, so a
/// named file that itself contains a ``` line (a README, a SKILL.md) cannot
/// close the fence early (same defect as #22H29SZR).
fn fence_for(text: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// `## Files the worker named`: each path inlined under [`FILE_CAP`] bytes,
/// named with its size past that, and a read error said plainly.
fn files_section(files: &[PathBuf]) -> String {
    let mut out = String::from("## Files the worker named\n\n");
    if files.is_empty() {
        out.push_str("None named.\n\n");
        return out;
    }
    for path in files {
        out.push_str(&format!("### {}\n\n", path.display()));
        match std::fs::read(path) {
            Ok(bytes) if bytes.len() > FILE_CAP => out.push_str(&format!(
                "{} bytes, past what this brief carries inline.\n\n",
                bytes.len()
            )),
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                let text = text.trim_end_matches('\n');
                let fence = fence_for(text);
                out.push_str(&format!("{fence}\n{text}\n{fence}\n\n"));
            }
            Err(e) => out.push_str(&format!("cannot be read ({e}).\n\n")),
        }
    }
    out
}

/// `## The worker's reports`: the task's own status file, verbatim.
fn reports_section(status: &Path) -> String {
    let text = std::fs::read_to_string(status).unwrap_or_default();
    let body = if text.trim().is_empty() {
        "Nothing reported yet.".to_string()
    } else {
        text.trim_end().to_string()
    };
    format!("## The worker's reports\n\n{body}\n\n")
}

/// `## The plan this task belongs to` and `## The task`, verbatim, for a
/// consult from inside a run.
fn plan_and_task_section(prose: &str, block: &str) -> String {
    let mut out = String::new();
    if !prose.trim().is_empty() {
        out.push_str("## The plan this task belongs to\n\n");
        out.push_str(prose.trim());
        out.push_str("\n\n");
    }
    out.push_str("## The task\n\n");
    if block.is_empty() {
        out.push_str("This task is not in the plan of record.\n\n");
    } else {
        out.push_str(block);
        out.push('\n');
    }
    out
}

/// What a consult outside a run is held to, in place of the plan and the
/// task block: `--against <text>`, required there.
fn against_section(against: &str) -> String {
    format!("## What this is held to\n\n{}\n\n", against.trim())
}

/// The advisor's whole prompt: `task_id` names the heading and is absent
/// outside a run, `body` is [`plan_and_task_section`] or
/// [`against_section`], `status` is the task's status file, absent outside a
/// run.
#[allow(clippy::too_many_arguments)]
fn prompt(
    task_id: Option<&str>,
    body: &str,
    pages: &[(String, Option<String>)],
    question: &str,
    files: &[PathBuf],
    status: Option<&Path>,
    answer: &Path,
) -> String {
    let heading = match task_id {
        Some(id) => format!("# Advice for {id}\n\n"),
        None => "# Advice\n\n".to_string(),
    };
    let pages = crate::brief::pages_section(task_id.unwrap_or("advice"), pages);
    let reports = status.map(reports_section).unwrap_or_default();
    format!(
        "\
{heading}\
{body}\
{pages}\
## The question

{question}

{files}\
{reports}\
## How to answer

Answer in under 400 words. Name the files and lines you mean. Write nothing
and run nothing in this tree.

Write your whole answer to exactly this file and then stop:

    Answer file: {answer}
",
        question = question.trim(),
        files = files_section(files),
        answer = answer.display(),
    )
}

/// One consult off a written prompt: the backend starts the advisor, waits
/// for its turn and stops it (`WorkerBackend::consult`), and this reads the
/// ending. An answer file is the answer, printed (exit 0); a launch refused,
/// a session stopped at a question, the deadline, or a session that ended
/// with nothing written are each said on stderr (exit 1), the question in
/// its own words when the backend saw one. `log` is `(task, n, question)`,
/// present only for a consult inside a run (ruling 1).
fn consult(
    backend: &dyn WorkerBackend,
    d: &Dispatch,
    answer: &Path,
    log: Option<(&str, u64, &str)>,
) -> i32 {
    let deadline_s = reviewer::deadline_s();
    let c = backend.consult(d, deadline_s);
    if let Some(line) = ending_line("advise", "the consult", &c, d, deadline_s, answer) {
        warn(line);
        return exit::FAILED;
    }
    if !answer.exists() {
        warn(format!(
            "advise: the session ended with no answer -- read {}",
            answer.display()
        ));
        return exit::FAILED;
    }
    let text = std::fs::read_to_string(answer).unwrap_or_default();
    print!("{text}");
    if let Some((task, n, question)) = log {
        memcli::log_run(&format!(
            "task {task}: advised ({n}) -- {}",
            clip80(question)
        ));
    }
    exit::OK
}

/// The stderr line for a consult that gave no answer to read, or `None` when
/// the ending leaves the answer file to speak. `verb` opens the line; `what`
/// names the turn ("the consult", "the reading"). A launch refused has no
/// session and its reason on `Dispatch::err`; a question is named in its
/// own words when the backend read one, since the folder-trust screen and
/// its kind were "no verdict, for an hour" until it was (friction #HAN4WNAR).
pub(crate) fn ending_line(
    verb: &str,
    what: &str,
    c: &Consult,
    d: &Dispatch,
    deadline_s: i64,
    answer: &Path,
) -> Option<String> {
    match c.ending {
        Ending::Unclean if c.session.is_empty() => {
            let said = std::fs::read_to_string(&d.err).unwrap_or_default();
            let line = said
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("nothing on stderr");
            Some(format!("{verb}: the launch was refused: {line}"))
        }
        Ending::TimedOut => Some(format!(
            "{verb}: {what} ran past its {deadline_s} second deadline and was stopped -- read {}",
            answer.display()
        )),
        Ending::Blocked => {
            let asked = c.question.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(match asked.is_empty() {
                true => format!(
                    "{verb}: the session stopped at a question -- read {}",
                    answer.display()
                ),
                false => format!(
                    "{verb}: the session stopped at a question: \"{asked}\" -- read {}",
                    answer.display()
                ),
            })
        }
        Ending::Answered | Ending::Unclean => None,
    }
}

/// The command line's own dispatch, common to both branches: `turns` off
/// `WORKFLOW_MAX_TURNS`, no extra environment.
#[allow(clippy::too_many_arguments)]
fn base_dispatch(
    task: String,
    worktree: PathBuf,
    brief: PathBuf,
    dir: &Path,
    stem: &str,
    session: String,
    model: String,
    effort: Option<String>,
) -> Dispatch {
    Dispatch {
        task,
        worktree,
        brief,
        out: dir.join(format!("{stem}-out")),
        err: dir.join(format!("{stem}-err")),
        pidfile: dir.join(format!("{stem}-pid")),
        status: dir.join(format!("{stem}-status")),
        rundir: dir.to_path_buf(),
        session,
        parent: None,
        role: "advisor".into(),
        model,
        effort,
        turns: match std::env::var("WORKFLOW_MAX_TURNS") {
            Ok(v) if !v.is_empty() => v,
            _ => "120".into(),
        },
        env: Vec::new(),
    }
}

fn no_advisor_refusal() -> i32 {
    warn("advise: nobody is named to advise.");
    warn(
        "point WORKFLOW_ADVISOR at a model for this call, or name the project's reader with `mem project set review-model <model>`, which is who advises when nothing else says.",
    );
    exit::USAGE
}

/// Inside a run: `WORKFLOW_TASK=<plan>/<task>`, the run dir
/// `paths::runs_root()/<project>/<plan>`, the plan's prose and this task's
/// block, the pages its `Read:` names, and the worker's own status file
/// (ruling 1).
fn cmd_advise_in_run(
    dir: &Path,
    plan_id: &str,
    task_id: &str,
    top: &Path,
    question: &str,
    files: &[PathBuf],
) -> i32 {
    let Some(model) = advisor_model(Some(dir)) else {
        return no_advisor_refusal();
    };

    let n = field(dir, &format!("{task_id}.advised"))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    if n > CONSULT_LIMIT {
        warn("advise: three consults this attempt; the fourth is a mem ask");
        return exit::USAGE;
    }
    write_field(dir, task_id, "advised", &n.to_string());

    let plan_text = memcli::plan().unwrap_or_default();
    // The plan of record moves under a live run (`mem plan --from <slug>`, a
    // roadmap taking up the next milestone) and ids like `t1` repeat across
    // plans, so a plan that is not this run's is no plan at all here rather
    // than another plan's `t1` under this task's heading (run.rs `task_now`).
    let parsed = plan::parse(&plan_text, false).filter(|p| p.plan_id == plan_id);
    let task = parsed.as_ref().and_then(|p| p.get(task_id));
    let pages: Vec<(String, Option<String>)> = task
        .map(|t| t.wiki_slugs())
        .unwrap_or_default()
        .into_iter()
        .map(|slug| {
            let text = memcli::wiki_page(&slug);
            (slug, text)
        })
        .collect();
    let prose = parsed
        .as_ref()
        .map(|_| plan::prose(&plan_text))
        .unwrap_or_default();
    let body = plan_and_task_section(&prose, task.map_or("", |t| &t.block));

    let status = dir.join(format!("{task_id}.status"));
    let answer = dir.join(format!("{task_id}.advice.{n}"));
    let prompt_text = prompt(
        Some(task_id),
        &body,
        &pages,
        question,
        files,
        Some(&status),
        &answer,
    );
    let brief = dir.join(format!("{task_id}.advice-prompt.{n}"));
    if !write_prompt(&brief, &prompt_text) {
        return exit::FAILED;
    }

    let backend = run::backend_for();
    let session = backend.mint_session();
    let stem = format!("{task_id}.advice-{n}");
    let mut d = base_dispatch(
        format!("{task_id}-advice-{n}"),
        top.to_path_buf(),
        brief,
        dir,
        &stem,
        session,
        model,
        reader_effort(Some(dir)),
    );
    // The advisor gets its own task tag, the way the reader's dispatch does
    // (run.rs `read_start`): inheriting the worker's verbatim would file the
    // advisor's `mem ask` under the worker's task and spend the worker's own
    // consults.
    d.env.push((
        "WORKFLOW_TASK".into(),
        format!("{plan_id}/{task_id}-advice-{n}"),
    ));
    consult(backend.as_ref(), &d, &answer, Some((task_id, n, question)))
}

/// Outside a run: `--against <text>` stands in for the plan and the task
/// block, and is required; the answer goes to
/// `paths::runs_root()/<project>/_advice/<unix seconds>-<mint>/advice`
/// (ruling 1 as amended, #WCX5RB28: two consults in the same second are two
/// directories).
fn cmd_advise_standalone(
    project_dir: &str,
    top: &Path,
    question: &str,
    files: &[PathBuf],
    against: Option<&str>,
) -> i32 {
    let Some(against) = against else {
        warn(
            "advise: outside a run, --against <text> says what the question is held to, and is required.",
        );
        return exit::USAGE;
    };
    let Some(model) = advisor_model(None) else {
        return no_advisor_refusal();
    };

    let backend = run::backend_for();
    let mint = backend.mint_session();
    let dir = crate::paths::runs_root()
        .join(project_dir)
        .join("_advice")
        .join(format!("{}-{mint}", sys::now()));
    if std::fs::create_dir_all(&dir).is_err() {
        warn(format!("advise: cannot make {}", dir.display()));
        return exit::USAGE;
    }

    let answer = dir.join("advice");
    let prompt_text = prompt(
        None,
        &against_section(against),
        &[],
        question,
        files,
        None,
        &answer,
    );
    let brief = dir.join("advice-prompt");
    if !write_prompt(&brief, &prompt_text) {
        return exit::FAILED;
    }

    let d = base_dispatch(
        "advice".into(),
        top.to_path_buf(),
        brief,
        &dir,
        "advice",
        mint,
        model,
        reader_effort(None),
    );
    consult(backend.as_ref(), &d, &answer, None)
}

/// The `<project>` component of a task worktree,
/// `worktrees_root()/<project>/<plan>/<task>`. A worker's cwd is its
/// worktree root, and mem resolves a child project by the path relative to
/// the toplevel: at the root that path is empty, so mem answers with the
/// *root* project and a child project's consult would compute a run dir
/// under the monorepo instead of under the child. The worktree and the run
/// dir carry the identical component (run.rs writes both from
/// `project.dir_name()`), so inside one the path is the answer, and the
/// name every mem call is made under; verify.rs `task_verify_cmd` reads it
/// the same way.
fn project_from_worktree(top: &Path) -> Option<String> {
    let root = crate::paths::realpath_m(crate::paths::worktrees_root());
    let real = crate::paths::realpath(top)?;
    let rel = real.strip_prefix(&root).ok()?;
    let mut parts = rel.components();
    let (project, _plan, _task) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some(project.as_os_str().to_string_lossy().to_string())
}

pub fn cmd_advise(question: &str, files: &[PathBuf], against: Option<&str>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("advise: not inside a git work tree");
        return exit::USAGE;
    }
    memcli::resolve_from_here();
    // Resolved against the caller's cwd before goto_toplevel() chdirs the
    // process, or a relative --file would be read against the wrong tree.
    let cwd = crate::paths::cwd();
    let files: Vec<PathBuf> = files
        .iter()
        .map(|f| {
            if f.is_absolute() {
                f.clone()
            } else {
                cwd.join(f)
            }
        })
        .collect();
    let files = files.as_slice();
    let Some((_, top)) = repo::goto_toplevel() else {
        warn("advise: cannot resolve the repository toplevel");
        return exit::USAGE;
    };

    // Inside a task worktree the path names the project, and so does every
    // mem call from here on: mem's own answer at a worktree root is the
    // monorepo's root project, whose plan, pages and log are not this
    // task's (ruling 1 as amended).
    let project_dir = match project_from_worktree(&top) {
        Some(name) => {
            memcli::name_project(&name);
            name
        }
        None => memcli::project_current()
            .map(|p| p.dir_name())
            .unwrap_or_else(|| crate::paths::path_slug(&top)),
    };

    let task_env = std::env::var("WORKFLOW_TASK")
        .ok()
        .filter(|v| !v.is_empty());
    match task_env.as_deref().and_then(|v| v.split_once('/')) {
        Some((plan_id, task_id)) => {
            let dir = crate::paths::runs_root().join(&project_dir).join(plan_id);
            cmd_advise_in_run(&dir, plan_id, task_id, &top, question, files)
        }
        None => cmd_advise_standalone(&project_dir, &top, question, files, against),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consulted(ending: Ending, session: &str, question: &str) -> Consult {
        Consult {
            ending,
            answer: String::new(),
            session: session.into(),
            question: question.into(),
        }
    }

    #[test]
    fn the_ending_line_names_the_question_the_refusal_and_the_deadline() {
        let dir = std::env::temp_dir().join(format!("wf-advise-ending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let answer = dir.join("advice");
        let d = base_dispatch(
            "advice".into(),
            dir.clone(),
            dir.join("prompt"),
            &dir,
            "advice",
            "wf-a".into(),
            "opus".into(),
            None,
        );
        let line = |c: &Consult| ending_line("advise", "the consult", c, &d, 900, &answer);

        // An answer, or a session that ended unclean but has a session: the
        // file decides, so no line.
        assert_eq!(line(&consulted(Ending::Answered, "wf-a", "")), None);
        assert_eq!(line(&consulted(Ending::Unclean, "wf-a", "")), None);

        // Refused: no session, and amx's own line off err.
        std::fs::write(
            &d.err,
            "\namx sub: max_children is 8 and wf-p already has that many\n",
        )
        .unwrap();
        assert_eq!(
            line(&consulted(Ending::Unclean, "", "")).as_deref(),
            Some(
                "advise: the launch was refused: amx sub: max_children is 8 and wf-p already has that many"
            )
        );

        // A question, in its own words, whitespace folded.
        let asked = consulted(
            Ending::Blocked,
            "wf-a",
            "Quick safety check:\n  Is this a project you trust?",
        );
        assert_eq!(
            line(&asked).as_deref(),
            Some(&*format!(
                "advise: the session stopped at a question: \"Quick safety check: Is this a project you trust?\" -- read {}",
                answer.display()
            ))
        );
        assert_eq!(
            line(&consulted(Ending::Blocked, "wf-a", "")).as_deref(),
            Some(&*format!(
                "advise: the session stopped at a question -- read {}",
                answer.display()
            ))
        );
        assert_eq!(
            ending_line(
                "read",
                "the reading",
                &consulted(Ending::TimedOut, "wf-a", ""),
                &d,
                900,
                &answer
            )
            .as_deref(),
            Some(&*format!(
                "read: the reading ran past its 900 second deadline and was stopped -- read {}",
                answer.display()
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dial_reads_the_environment_then_the_run_dir_then_the_project() {
        // No environment, no run dir: the project's word stands.
        assert_eq!(
            dial("WF_ADVISE_TEST_UNSET", None, "advisor", || Some(
                "opus".into()
            )),
            Some("opus".into())
        );
        // A run dir record wins over the project when the environment is
        // silent.
        let dir = std::env::temp_dir().join(format!("wf-advise-dial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t1.advisor"), "sage\n").unwrap();
        assert_eq!(
            dial("WF_ADVISE_TEST_UNSET", Some(&dir), "t1.advisor", || Some(
                "opus".into()
            )),
            Some("sage".into())
        );
        // An empty record is nobody, and the project is never asked.
        std::fs::write(dir.join("t1.advisor"), "\n").unwrap();
        assert_eq!(
            dial("WF_ADVISE_TEST_UNSET", Some(&dir), "t1.advisor", || panic!(
                "the project must not be asked"
            )),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_holding_a_fence_does_not_close_the_advisors_own() {
        let dir = std::env::temp_dir().join(format!("wf-advise-fence-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("SKILL.md");
        std::fs::write(&doc, "before\n```\ncode\n```\nafter\n").unwrap();
        let section = files_section(&[doc]);
        assert!(
            section.contains("````\nbefore\n```\ncode\n```\nafter\n````"),
            "{section}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_question_is_clipped_to_eighty_chars_on_a_boundary() {
        let q = "x".repeat(90);
        assert_eq!(clip80(&q).len(), 80);
        assert_eq!(clip80("short"), "short");
    }

    #[test]
    fn files_past_the_cap_are_named_not_inlined() {
        let dir = std::env::temp_dir().join(format!("wf-advise-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = dir.join("small.txt");
        std::fs::write(&small, "a small file").unwrap();
        let big = dir.join("big.txt");
        std::fs::write(&big, "x".repeat(FILE_CAP + 1)).unwrap();
        let section = files_section(&[small, big]);
        assert!(section.contains("a small file"), "{section}");
        assert!(
            section.contains(&format!(
                "{} bytes, past what this brief carries inline.",
                FILE_CAP + 1
            )),
            "{section}"
        );
        assert!(!section.contains(&"x".repeat(100)), "{section}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_prompt_carries_the_question_and_names_the_answer_file() {
        let text = prompt(
            Some("t1"),
            &plan_and_task_section("Ruling 1. Cents.", "- [ ] t1 Do it\n"),
            &[],
            "which file owns rounding?",
            &[],
            None,
            Path::new("/runs/t1.advice.1"),
        );
        for needle in [
            "# Advice for t1",
            "Ruling 1. Cents.",
            "- [ ] t1 Do it",
            "which file owns rounding?",
            "Answer in under 400 words",
            "Answer file: /runs/t1.advice.1",
        ] {
            assert!(text.contains(needle), "the prompt lost {needle:?}: {text}");
        }
    }
}
