//! `workflow` -- the verification gate and the interim orchestrator.
//!
//!   workflow verify          run this repo's suite over what is staged
//!   workflow lint-msg        check a commit message, branch name or PR body
//!   workflow review-needed   does this change set want a cold review?
//!   workflow plan-check      read a plan and report its tasks and waves
//!   workflow run             run a plan's tasks in worktrees
//!   workflow reap            collect finished or stalled workers
//!   workflow status          report this project's runs, --json for machines
//!   workflow doctor          check this machine's wiring
//!   workflow hook            the body of a git hook stub
//!   workflow enable/disable  this repo's skills, on in a project or off
//!   workflow settings-merge  the install's settings edit
//!
//! Exit codes are a contract; `workflow help` prints them.
//!
//! Process state lives in mem, never in the repo. Everything here is driven by
//! git and mem, so it works the same under any agent runtime.

pub mod backend;
pub mod brief;
pub mod cli;
pub mod doctor;
pub mod exit;
pub mod gitcmd;
pub mod hook;
pub mod lint;
pub mod memcli;
pub mod ownership;
pub mod paths;
pub mod plan;
pub mod plancheck;
pub mod repo;
pub mod review;
pub mod run;
pub mod settings;
pub mod status;
pub mod sys;
pub mod testdecl;
pub mod verify;

use clap::Parser;

use cli::{Cli, Command};

/// Everything the commands say to a human goes to stderr, prefixed, so stdout
/// stays the answer.
pub fn warn(msg: impl AsRef<str>) {
    eprintln!("workflow: {}", msg.as_ref());
}

pub fn warn_as(prefix: &str, msg: impl AsRef<str>) {
    eprintln!("{prefix}: {}", msg.as_ref());
}

pub fn have(program: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| gitcmd::exists_x(&dir.join(program)))
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
        Command::Verify { hook, gate } => verify::cmd_verify(if hook {
            verify::Mode::Hook
        } else if gate {
            verify::Mode::Gate
        } else {
            verify::Mode::Direct
        }),
        Command::LintMsg { msgfile, string } => {
            lint::cmd_lint_msg(msgfile.as_deref(), string.as_deref())
        }
        Command::ReviewNeeded { diff } => review::cmd_review_needed(diff.as_deref()),
        Command::Hook { name, stub, args } => hook::cmd_hook(&name, stub.as_deref(), &args),
        Command::Run { plan_file } => run::cmd_run(plan_file.as_deref()),
        Command::Reap => run::cmd_reap(),
        Command::Redispatch { task } => run::cmd_redispatch(&task),
        Command::Status { json } => status::cmd_status(json),
        Command::PlanCheck { file, json } => cmd_plan_check(&file, json),
        Command::Ownership {
            repo,
            base,
            branch,
            patterns,
        } => {
            for line in ownership::show(&ownership::violations(&repo, &base, &branch, &patterns)) {
                println!("{line}");
            }
            exit::OK
        }
        Command::SplitPatterns { line } => {
            for p in ownership::split_patterns(&line) {
                println!("{p}");
            }
            exit::OK
        }
        Command::Stalled {
            rundir,
            wtroot,
            deadline,
            task,
        } => run::cmd_stalled(&rundir, &wtroot, &task, deadline),
        Command::Doctor => doctor::cmd_doctor(),
        Command::SettingsMerge { file, dry_run } => {
            settings::cmd_settings_merge(file.as_deref(), dry_run)
        }
        Command::Enable { global, dry_run } => settings::cmd_skills("on", global, dry_run),
        Command::Disable { global, dry_run } => settings::cmd_skills("off", global, dry_run),
    }
}

/// The plan grammar, run over a file and reported. This is what a planner runs
/// before asking for approval, and it is also the harness's way of checking the
/// parser against written-down expectations (AC12). It touches nothing: no
/// worktree, no branch, no worker.
fn cmd_plan_check(file: &std::path::Path, json: bool) -> i32 {
    let Ok(text) = std::fs::read_to_string(file) else {
        warn(format!("plan-check: cannot read {}", file.display()));
        return exit::USAGE;
    };
    let Some(parsed) = plan::parse(&text, true) else {
        return exit::FAILED;
    };
    // The grammar is half the check: the plan must also hold in the checkout
    // it will run in (frictions #DBHZBFY1, #6485CNC0). Warnings inform the
    // planner; a Verify that cannot pass here is refused, after the report so
    // the author still sees what parsed.
    let mut refusals = Vec::new();
    if let Some(top) = gitcmd::Git::here().toplevel() {
        let found = plancheck::findings(&parsed, &top);
        for w in &found.warnings {
            warn(w);
        }
        for r in &found.refusals {
            warn(r);
        }
        refusals = found.refusals;
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&parsed).unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!("plan: {}", parsed.plan_id);
        for t in &parsed.tasks {
            println!("  {} {}", t.id, t.title);
        }
        for (i, w) in parsed.waves.iter().enumerate() {
            println!("  wave {}: {}", i + 1, w.join(" "));
        }
    }
    if !refusals.is_empty() {
        return exit::FAILED;
    }
    exit::OK
}
