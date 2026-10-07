//! `workflow` -- hygiene checks and the git hooks that run them.
//!
//!   workflow hygiene         agent files and process references in a repo
//!   workflow lint-msg        check a commit message, branch name or PR body
//!   workflow hook            the body of a git hook stub
//!   workflow install         write the embedded skills, agents and hook stubs
//!   workflow go              start the orchestrator for a project's next milestone
//!
//! Exit codes are a contract; `workflow help` prints them.
//!
//! Process state lives in mem, never in the repo. Everything here is driven by
//! git and mem, so it works the same under any agent runtime.

pub mod cli;
pub mod embedded;
pub mod exit;
pub mod gitcmd;
pub mod go;
pub mod hook;
pub mod hygiene;
pub mod install;
pub mod lint;
pub mod memcli;
pub mod paths;
#[cfg(test)]
mod scratch;

use clap::Parser;

use cli::{Cli, Command};

/// Everything the commands say to a human goes to stderr, prefixed, so stdout
/// stays the answer.
pub fn warn(msg: impl AsRef<str>) {
    eprintln!("workflow: {}", msg.as_ref());
}

/// `help`, no command and an unknown command are answered here rather than by
/// clap: their output and their exit codes are part of the contract.
pub fn main(argv: Vec<String>) -> i32 {
    match argv.get(1).map(String::as_str) {
        None => {
            print!("{}", cli::USAGE);
            return exit::USAGE;
        }
        Some("help") | Some("-h") | Some("--help") => {
            print!("{}", cli::USAGE);
            return exit::OK;
        }
        _ => {}
    }

    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            // Only a command clap does not know is answered here. An option it
            // does not know belongs to a command that was spelled right, and
            // clap's own message names the option; blaming argv[1] would send
            // the reader to check the one thing that was correct.
            let unknown = e.kind() == clap::error::ErrorKind::InvalidSubcommand;
            if unknown && !argv[1].starts_with('-') {
                warn(format!("unknown command: {}", argv[1]));
                eprint!("{}", cli::USAGE);
                return exit::USAGE;
            }
            // Anything else clap has an opinion about -- --help, --version, a
            // missing value -- it prints itself, with its own exit code.
            let _ = e.print();
            return if e.use_stderr() {
                exit::USAGE
            } else {
                exit::OK
            };
        }
    };

    run(cli)
}

pub fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::LintMsg { msgfile, string } => {
            lint::cmd_lint_msg(msgfile.as_deref(), string.as_deref())
        }
        Command::Hygiene {
            staged,
            tree,
            history,
            message,
            string,
            path,
            json,
            fix,
        } => hygiene::cmd_hygiene(
            hygiene::Scope {
                staged,
                tree,
                history,
                message: message.as_deref(),
                string: string.as_deref(),
            },
            path.as_deref(),
            json,
            fix,
        ),
        Command::Hook { name, stub, args } => hook::cmd_hook(&name, stub.as_deref(), &args),
        Command::Install => install::cmd_install(),
        Command::Go {
            project,
            milestone,
            model,
            effort,
            dry_run,
        } => go::cmd_go(
            &go::Options {
                project: project.as_deref(),
                milestone: milestone.as_deref(),
                model: model.as_deref().unwrap_or(go::DEFAULT_MODEL),
                effort: effort.as_deref().unwrap_or(go::DEFAULT_EFFORT),
            },
            dry_run,
        ),
    }
}
