//! `mem` — the system of record for AI workflow state.
//!
//! Files in `~/.local/share/mem/store` are the source of truth; the SQLite
//! index in `~/.cache/mem` is disposable and rebuildable from them.

pub mod app;
pub mod atomic;
pub mod cli;
pub mod digest;
pub mod exit;
pub mod git;
pub mod hooks;
pub mod ids;
pub mod index;
pub mod item;
pub mod maint;
pub mod paths;
pub mod project;
pub mod questions;
pub mod search;
pub mod session;
pub mod store;
pub mod sync;
pub mod timefmt;
pub mod verbs;
pub mod write;

/// Runs one invocation and returns its exit code. Errors are printed once, on
/// stderr, and turned into the exit code they carry (spec §7).
pub fn run(cli: cli::Cli) -> i32 {
    match dispatch(&cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("mem: {err}");
            exit::code_of(&err)
        }
    }
}

/// `--session-id` overrides `MEM_SESSION_ID`.
fn with_session(app: app::App, flag: &Option<String>) -> app::App {
    match session::id_from(flag.as_deref()) {
        Some(id) => app::App {
            session_id: Some(id),
            ..app
        },
        None => app,
    }
}

fn dispatch(cli: &cli::Cli) -> anyhow::Result<i32> {
    let Some(command) = &cli.command else {
        eprintln!("mem: no command given — try `mem --help`");
        return Ok(exit::USAGE);
    };
    let app = app::App::new(cli)?;
    match command {
        cli::Command::Context {
            project,
            budget,
            brief,
            hook_json,
            session_id,
        } => {
            let app = with_session(app, session_id);
            let app = if project.is_some() {
                app::App {
                    project: project.clone(),
                    ..app
                }
            } else {
                app
            };
            verbs::context(&app, *budget, *brief, *hook_json)
        }
        cli::Command::SessionCheck {
            session_id,
            hook_json,
        } => hooks::session_check(&with_session(app, session_id), *hook_json),
        cli::Command::Precompact { hook_json: _ } => hooks::precompact(&app),
        cli::Command::Search {
            query,
            kind,
            r#type,
            limit,
            min_score,
        } => verbs::search_verb(
            &app,
            query,
            kind.as_deref(),
            r#type.as_deref(),
            *limit,
            *min_score,
        ),
        cli::Command::Show { ids } => verbs::show(&app, ids),
        cli::Command::Projects => verbs::projects(&app),
        cli::Command::Project { command } => match command {
            cli::ProjectCommand::Current => verbs::project_current(&app),
            cli::ProjectCommand::Add { subdir, name } => {
                verbs::project_add(&app, subdir, name.as_deref())
            }
            cli::ProjectCommand::Set { command } => match command {
                cli::ProjectSetCommand::Verify { cmd } => {
                    verbs::project_set(&app, "verify", cmd)
                }
                cli::ProjectSetCommand::ReviewPaths { globs } => {
                    verbs::project_set(&app, "review_paths", globs)
                }
                cli::ProjectSetCommand::Backend { backend } => {
                    verbs::project_set(&app, "backend", backend.as_str())
                }
                cli::ProjectSetCommand::Remote { url } => verbs::project_set(&app, "remote", url),
            },
        },
        cli::Command::Save {
            text,
            kind,
            r#type,
            title,
            tags,
            supersedes,
            session_id,
        } => verbs::save(
            &with_session(app, session_id),
            kind,
            text,
            title.as_deref(),
            r#type.as_deref(),
            tags,
            supersedes.as_deref(),
        ),
        cli::Command::Log {
            text,
            limit,
            since,
            kind,
            r#type,
            session_id,
        } => verbs::log(
            &with_session(app, session_id),
            text.as_deref(),
            *limit,
            since.as_deref(),
            kind.as_deref(),
            r#type.as_deref(),
        ),
        cli::Command::Handoff {
            set,
            stdin,
            title,
            session_id,
        } => verbs::handoff(
            &with_session(app, session_id),
            set.as_deref(),
            *stdin,
            title.as_deref(),
        ),
        cli::Command::Status {
            set,
            stdin,
            session_id,
        } => verbs::status(&with_session(app, session_id), set.as_deref(), *stdin),
        cli::Command::Reindex { full } => verbs::reindex(&app, *full),
        cli::Command::Snapshot => verbs::snapshot(&app),
        cli::Command::Prune { apply } => verbs::prune(&app, apply),
        cli::Command::Sync => verbs::sync(&app),
        cli::Command::Doctor { fix } => verbs::doctor(&app, *fix),
        cli::Command::Ask {
            question,
            options,
            session_id,
        } => verbs::ask(&with_session(app, session_id), question, options),
        cli::Command::Questions {
            pending,
            all_projects,
            wait,
            timeout,
        } => verbs::questions(&app, *pending, *all_projects, wait.as_deref(), timeout),
        cli::Command::Answer {
            id,
            text,
            option,
            session_id,
        } => verbs::answer(
            &with_session(app, session_id),
            id,
            text.as_deref(),
            option.as_deref(),
        ),
        cli::Command::Wiki {
            slug,
            stdin,
            note,
            session_id,
        } => verbs::wiki(
            &with_session(app, session_id),
            slug.as_deref(),
            *stdin,
            note.as_deref(),
        ),
        cli::Command::Plan {
            set_file,
            stdin,
            clear,
            tick,
            session_id,
        } => verbs::plan(
            &with_session(app, session_id),
            set_file.as_deref(),
            *stdin,
            *clear,
            tick.as_deref(),
        ),
    }
}
