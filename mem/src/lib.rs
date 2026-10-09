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
pub mod store;
pub mod sync;
pub mod timefmt;
pub mod verbs;
pub mod write;

/// Runs one invocation and returns its exit code. Errors are printed once, on
/// stderr, and turned into the exit code they carry.
pub fn run(cli: cli::Cli) -> i32 {
    match dispatch(&cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("mem: {err}");
            exit::code_of(&err)
        }
    }
}

/// The id `context --brief` counts batches under; an empty flag is none.
fn with_session(app: app::App, flag: &Option<String>) -> app::App {
    match flag.as_deref().and_then(session::nonempty) {
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
        } => verbs::save(
            &app,
            kind,
            text,
            verbs::SaveMeta {
                title: title.as_deref(),
                r#type: r#type.as_deref(),
                tags,
                supersedes: supersedes.as_deref(),
            },
        ),
        cli::Command::Log {
            text,
            limit,
            since,
            kind,
            r#type,
        } => verbs::log(
            &app,
            text.as_deref(),
            *limit,
            since.as_deref(),
            kind.as_deref(),
            r#type.as_deref(),
            false,
        ),
        cli::Command::Handoff { set, stdin, title } => {
            verbs::handoff(&app, set.as_deref(), *stdin, title.as_deref())
        }
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
        } => verbs::ask(&app, question, options, recommend.as_deref(), *audience),
        cli::Command::Questions {
            pending,
            all_projects,
            audience,
            wait,
            timeout,
        } => verbs::questions(
            &app,
            *pending,
            *all_projects,
            *audience,
            wait.as_deref(),
            timeout,
        ),
        cli::Command::Answer { id, text, option } => {
            verbs::answer(&app, id, text.as_deref(), option.as_deref())
        }
        cli::Command::Decide { text, by, replaces } => {
            records::decide(&app, text, by.as_str(), replaces.as_deref())
        }
        cli::Command::Evidence { command } => match command {
            cli::EvidenceCommand::Add { task, file, note } => {
                records::evidence_add(&app, task, file, note)
            }
            cli::EvidenceCommand::List => records::evidence_list(&app),
            cli::EvidenceCommand::Cat { id } => records::evidence_cat(&app, id),
        },
        cli::Command::Finding { command } => match command {
            cli::FindingCommand::Add {
                milestone,
                step,
                text,
                evidence,
            } => records::finding_add(&app, milestone, step, text, evidence.as_deref()),
            cli::FindingCommand::List { open } => records::finding_list(&app, *open),
            cli::FindingCommand::Close { id, by } => records::finding_close(&app, id, by),
        },
        cli::Command::Raw { command } => match command {
            cli::RawCommand::Add { source } => records::raw_add(&app, source),
        },
        cli::Command::Brief { set } => records::brief(&app, set.as_deref()),
        cli::Command::Idea { text } => records::idea(&app, text),
        cli::Command::Wiki {
            slug,
            rebuild,
            stdin,
            sections,
            note,
        } => verbs::wiki(
            &app,
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
        } => verbs::plan(
            &app,
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
        } => verbs::roadmap(
            &app,
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
