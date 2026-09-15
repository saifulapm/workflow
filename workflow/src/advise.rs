//! `workflow advise` -- a worker asks a stronger model at a decision point,
//! without ending its turn (spec: Anthropic's Sonnet + Opus advisor). The
//! run resolves who answers the same way it resolves the fix model: an
//! override, a record, a project key, then the reader's own model by the
//! same rungs. A decision -- scope, taste, a broken plan -- stays `mem ask`;
//! this never stops the task, and three consults in one attempt is the
//! limit before the fourth is one.

use std::path::{Path, PathBuf};

use crate::backend::{Dispatch, Handle, WorkerBackend};
use crate::{exit, gitcmd::Git, memcli, plan, repo, reviewer, run, sys, warn};

/// Past this many bytes a named file is named but not inlined, the same cap
/// `read.rs` holds an untracked file to.
const FILE_CAP: usize = 24 * 1024;

/// Consults an attempt may spend before the fourth is turned back to a
/// `mem ask` (ruling 1).
const CONSULT_LIMIT: u64 = 3;

/// One task's field in a run dir, written the way `run.rs`'s own
/// `write_field` does: `<dir>/<task>.<key>`, trimmed to nothing readable
/// but its value plus a newline.
fn write_field(dir: &Path, task: &str, key: &str, value: &str) {
    let _ = std::fs::write(dir.join(format!("{task}.{key}")), format!("{value}\n"));
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

/// `WORKFLOW_ADVISOR`, else the run dir's `advisor` record, else `mem
/// project set advisor`, else the reader's own model by the same rungs
/// (ruling 1).
fn advisor_model(run_dir: Option<&Path>) -> Option<String> {
    dial(
        "WORKFLOW_ADVISOR",
        run_dir,
        "advisor",
        memcli::project_advisor,
    )
    .or_else(|| reader_model(run_dir))
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
                out.push_str(&format!("```\n{}\n```\n\n", text.trim_end_matches('\n')));
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

/// Dispatch off a written prompt and wait for it: polls `alive` every
/// second, prints the answer when the session ends and it exists (exit 0);
/// past [`reviewer::deadline_s`] the session is stopped, and a session that
/// ended with nothing written reads the same way -- no answer either way
/// (exit 1). `log` is `(task, n, question)`, present only for a consult
/// inside a run (ruling 1).
fn dispatch_and_wait(
    backend: &dyn WorkerBackend,
    d: &Dispatch,
    answer: &Path,
    log: Option<(&str, u64, &str)>,
) -> i32 {
    let dispatched = backend.dispatch(d);
    if dispatched.is_empty() {
        let said = std::fs::read_to_string(&d.err).unwrap_or_default();
        let line = said
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("nothing on stderr");
        warn(format!("advise: the launch was refused: {line}"));
        return exit::FAILED;
    }
    let h = Handle {
        session: dispatched,
        pidfile: d.pidfile.clone(),
        worktree: d.worktree.clone(),
    };
    let deadline_s = reviewer::deadline_s();
    let started = sys::now();
    let mut timed_out = false;
    while backend.alive(&h) {
        if sys::now() - started >= deadline_s {
            timed_out = true;
            break;
        }
        sys::sleep(1.0);
    }
    if timed_out {
        let grace_s = (deadline_s / 2).clamp(1, 30);
        backend.stop(&h, grace_s);
        return exit::FAILED;
    }
    if !answer.exists() {
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
        "name one with `mem project set advisor <model>`, or point WORKFLOW_ADVISOR at one for this call.",
    );
    exit::USAGE
}

/// Inside a run: `WORKFLOW_TASK=<plan>/<task>`, the run dir
/// `paths::runs_root()/<project>/<plan>`, the plan's prose and this task's
/// block, the pages its `Read:` names, and the worker's own status file
/// (ruling 1).
fn cmd_advise_in_run(
    dir: &Path,
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
    let parsed = plan::parse(&plan_text, false);
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
    let body = plan_and_task_section(&plan::prose(&plan_text), task.map_or("", |t| &t.block));

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
    let _ = std::fs::write(&brief, &prompt_text);

    let backend = run::backend_for();
    let session = backend.mint_session();
    let stem = format!("{task_id}.advice-{n}");
    let d = base_dispatch(
        format!("{task_id}-advice-{n}"),
        top.to_path_buf(),
        brief,
        dir,
        &stem,
        session,
        model,
        reader_effort(Some(dir)),
    );
    dispatch_and_wait(backend.as_ref(), &d, &answer, Some((task_id, n, question)))
}

/// Outside a run: `--against <text>` stands in for the plan and the task
/// block, and is required; the answer goes to
/// `paths::runs_root()/<project>/_advice/<unix seconds>/advice` (ruling 1).
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
    let _ = std::fs::write(&brief, &prompt_text);

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
    dispatch_and_wait(backend.as_ref(), &d, &answer, None)
}

pub fn cmd_advise(question: &str, files: &[PathBuf], against: Option<&str>) -> i32 {
    if !Git::here().inside_worktree() {
        warn("advise: not inside a git work tree");
        return exit::USAGE;
    }
    let Some((_, top)) = repo::goto_toplevel() else {
        warn("advise: cannot resolve the repository toplevel");
        return exit::USAGE;
    };

    let project = memcli::project_current();
    let project_dir = project
        .as_ref()
        .map(|p| p.dir_name())
        .unwrap_or_else(|| crate::paths::path_slug(&top));

    let task_env = std::env::var("WORKFLOW_TASK")
        .ok()
        .filter(|v| !v.is_empty());
    match task_env.as_deref().and_then(|v| v.split_once('/')) {
        Some((plan_id, task_id)) => {
            let dir = crate::paths::runs_root().join(&project_dir).join(plan_id);
            cmd_advise_in_run(&dir, task_id, &top, question, files)
        }
        None => cmd_advise_standalone(&project_dir, &top, question, files, against),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
