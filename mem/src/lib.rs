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
pub mod lint;
pub mod maint;
pub mod paths;
pub mod project;
pub mod questions;
pub mod records;
pub mod search;
pub mod sections;
pub mod session;
pub mod skills;
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
            full,
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
            verbs::context(&app, *full, *budget, *brief, *hook_json)
        }
        cli::Command::SessionCheck {
            session_id,
            hook_json,
        } => hooks::session_check(&with_session(app, session_id), *hook_json),
        cli::Command::Skill { name } => Ok(skills::cmd_skill(name.as_deref())),
        cli::Command::Precompact { hook_json: _ } => hooks::precompact(&app),
        cli::Command::Search {
            query,
            kind,
            r#type,
            limit,
            min_score,
        } => match (query.as_deref(), kind.as_deref()) {
            (Some(query), kind) => {
                verbs::search_verb(&app, query, kind, r#type.as_deref(), *limit, *min_score)
            }
            // No query and a kind: the kind, newest first, the way `mem log
            // --kind` lists it, so a kind can be listed without inventing a query.
            (None, Some(kind)) => verbs::log(
                &app,
                None,
                *limit,
                None,
                Some(kind),
                r#type.as_deref(),
                true,
            ),
            (None, None) => Err(exit::usage(
                "search takes a query, or --kind <kind> to list that kind newest first",
            )),
        },
        cli::Command::List => Err(exit::usage(
            "mem has no list: `mem log` lists recent entries, `mem search <query>` finds items, \
             `mem search --kind <kind>` lists one kind",
        )),
        cli::Command::Show { ids } => verbs::show(&app, ids),
        cli::Command::Projects => verbs::projects(&app),
        cli::Command::Project { command } => match command {
            cli::ProjectCommand::Current => verbs::project_current(&app),
            cli::ProjectCommand::Add { subdir, name } => {
                verbs::project_add(&app, subdir, name.as_deref())
            }
            cli::ProjectCommand::Set { command } => match command {
                cli::ProjectSetCommand::Verify { cmd } => verbs::project_set(&app, "verify", cmd),
                cli::ProjectSetCommand::HygieneExempt { globs } => {
                    verbs::project_set(&app, "hygiene_exempt", globs)
                }
                cli::ProjectSetCommand::Remote { url } => verbs::project_set(&app, "remote", url),
            },
            cli::ProjectCommand::Unset { key } => verbs::project_unset(&app, key.stored()),
        },
        cli::Command::Save {
            text,
            kind,
            r#type,
            title,
            tags,
            supersedes,
            about,
            session_id,
        } => verbs::save(
            &with_session(app, session_id),
            kind,
            text,
            verbs::SaveMeta {
                title: title.as_deref(),
                r#type: r#type.as_deref(),
                tags,
                supersedes: supersedes.as_deref(),
                about: about.as_deref(),
            },
        ),
        cli::Command::Log {
            text: None,
            about: Some(prefix),
            ..
        } => verbs::about(&app, "log", prefix),
        cli::Command::Log {
            text,
            limit,
            since,
            kind,
            r#type,
            about: _,
            session_id,
        } => verbs::log(
            &with_session(app, session_id),
            text.as_deref(),
            *limit,
            since.as_deref(),
            kind.as_deref(),
            r#type.as_deref(),
            false,
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
        cli::Command::Reindex { full } => verbs::reindex(&app, *full),
        cli::Command::Snapshot => verbs::snapshot(&app),
        cli::Command::Prune { apply } => verbs::prune(&app, apply),
        cli::Command::Sync => verbs::sync(&app),
        cli::Command::Doctor { fix } => verbs::doctor(&app, *fix),
        cli::Command::Ask {
            question,
            options,
            recommend,
            audience,
            about,
            session_id,
        } => verbs::ask(
            &with_session(app, session_id),
            question,
            options,
            recommend.as_deref(),
            *audience,
            about.as_deref(),
        ),
        cli::Command::Questions {
            about: Some(prefix),
            ..
        } => verbs::about(&app, "question", prefix),
        cli::Command::Questions {
            pending,
            all_projects,
            audience,
            asked_by,
            wait,
            timeout,
            about: None,
        } => verbs::questions(
            &app,
            *pending,
            *all_projects,
            *audience,
            asked_by.as_deref(),
            wait.as_deref(),
            timeout,
        ),
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
        cli::Command::Decide {
            text,
            by,
            replaces,
            session_id,
        } => records::decide(
            &with_session(app, session_id),
            text,
            by.as_str(),
            replaces.as_deref(),
        ),
        cli::Command::Evidence { command } => match command {
            cli::EvidenceCommand::Add {
                task,
                file,
                note,
                session_id,
            } => records::evidence_add(&with_session(app, session_id), task, file, note),
            cli::EvidenceCommand::List { task } => records::evidence_list(&app, task.as_deref()),
            cli::EvidenceCommand::Cat { id } => records::evidence_cat(&app, id),
        },
        cli::Command::Finding { command } => match command {
            cli::FindingCommand::Add {
                milestone,
                step,
                text,
                evidence,
                session_id,
            } => records::finding_add(
                &with_session(app, session_id),
                milestone,
                step,
                text,
                evidence.as_deref(),
            ),
            cli::FindingCommand::List { open } => records::finding_list(&app, *open),
            cli::FindingCommand::Close { id, by } => records::finding_close(&app, id, by),
        },
        cli::Command::Raw { command } => match command {
            cli::RawCommand::Add { source } => records::raw_add(&app, source),
        },
        cli::Command::Brief { set, session_id } => {
            records::brief(&with_session(app, session_id), set.as_deref())
        }
        cli::Command::Idea { text, session_id } => {
            records::idea(&with_session(app, session_id), text)
        }
        cli::Command::Wiki {
            slug,
            rebuild,
            stdin,
            sections,
            note,
            session_id,
        } => verbs::wiki(
            &with_session(app, session_id),
            slug.as_deref(),
            *stdin,
            *sections,
            note.as_deref(),
            *rebuild,
        ),
        cli::Command::Plan {
            slug,
            set_file,
            stdin,
            clear,
            tick,
            list,
            session_id,
        } => verbs::plan(
            &with_session(app, session_id),
            verbs::PlanArgs {
                slug: slug.as_deref(),
                set_file: set_file.as_deref(),
                stdin: *stdin,
                clear: *clear,
                tick: tick.as_deref(),
                list: *list,
            },
        ),
        cli::Command::Roadmap {
            set_file,
            stdin,
            clear,
            tick,
            status,
            session_id,
        } => verbs::roadmap(
            &with_session(app, session_id),
            verbs::RoadmapArgs {
                set_file: set_file.as_deref(),
                stdin: *stdin,
                clear: *clear,
                tick: tick.as_deref(),
                status: status.as_ref().map(Option::as_deref),
            },
        ),
    }
}
