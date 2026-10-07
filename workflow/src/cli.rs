//! The command surface.
//!
//! `help`, no command at all and an unknown command are handled in
//! [`crate::main`] rather than by clap, because their output and their exit
//! codes are part of the contract the tests read.

use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "workflow",
    version,
    about = "Hygiene checks and the git hooks that run them",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Check a commit message, a branch name or a PR body.
    #[command(name = "lint-msg")]
    LintMsg {
        /// The message file git hands the commit-msg hook.
        msgfile: Option<PathBuf>,
        /// Lint this text instead of a file.
        #[arg(long)]
        string: Option<String>,
    },
    /// Look for agent files and process references in a repository.
    #[command(
        group(ArgGroup::new("scope").args(["staged", "tree", "history", "message", "string"])),
        long_about = "Look for agent files and process references in a repository.

The hard tier exits 1: a file on the global ignore list tracked or staged, a
.gitignore line naming one, and in the added lines of non-markdown files or in
a message a numbered ruling, milestone, ticket, issue or ADR, a memory id, a
co-author or generated-with line. A message also fails on a subject over 72
characters and on a bare task or milestone id. The soft tier only warns, reads
messages only, and is cleared per term by a lint-exception ruling. Paths the
project key hygiene-exempt names are not read for content.

With no mode the whole tracked tree and the last 200 commits are read."
    )]
    Hygiene {
        /// The staged paths and the lines they add.
        #[arg(long)]
        staged: bool,
        /// Every tracked file.
        #[arg(long)]
        tree: bool,
        /// The messages of the last N commits.
        #[arg(long, value_name = "N")]
        history: Option<usize>,
        /// A commit message file, git's commentary left out.
        #[arg(long, value_name = "FILE")]
        message: Option<PathBuf>,
        /// A message given as text.
        #[arg(long, value_name = "TEXT")]
        string: Option<String>,
        /// Read only under this directory.
        #[arg(long, value_name = "DIR")]
        path: Option<PathBuf>,
        /// Print the findings as a JSON array.
        #[arg(long)]
        json: bool,
        /// Untrack ignore-list files and drop their .gitignore lines first.
        #[arg(long)]
        fix: bool,
    },
    /// Write the embedded skills, agents and hook stubs where they are read.
    #[command(
        long_about = "Write the embedded skills, agents and hook stubs where they are read.

The binary carries the repository's skills/, agents/ and hooks/ directories.
Each skill goes to ~/.claude/skills and ~/.agents/skills, each agent to
~/.claude/agents, each hook stub to ~/.config/git/hooks with mode 0755. A
symlink in the way is replaced by the file, never written through, and a file
a shipped skill no longer has is deleted. The skill directories and amx roles
older installs wrote that nothing ships now are removed; every other name in
those directories is left alone. One line is printed per change."
    )]
    Install,
    /// Start the orchestrator for a project's next milestone as an amx agent.
    #[command(
        long_about = "Start the orchestrator for a project's next milestone as an amx agent.

The project is the argument, else the one the current directory belongs to.
The milestone is --milestone, else the first unticked line of the project's
roadmap; with none left it says the roadmap is done and exits 0. Without
--milestone the roadmap's status must be approved. It refuses, exit 2, while
another live amx agent of the project is running an orchestrator; the caller
itself ($AMX_ID) does not count, so an orchestrator can start its successor.
The agent is `amx new --name <project>-<milestone>` in the project's checkout,
on the goal that milestone's landing is the proof of; a name amx has taken by
an agent that ended gets a -2, -3, ... suffix. Prints the agent's name."
    )]
    Go {
        /// The project, by mem's name; the current directory's when omitted.
        project: Option<String>,
        /// The milestone to run, by slug; the roadmap's next open one when omitted.
        #[arg(long, value_name = "SLUG")]
        milestone: Option<String>,
        /// The model the orchestrator runs on.
        #[arg(long, value_name = "NAME")]
        model: Option<String>,
        /// How much reasoning effort it spends.
        #[arg(long, value_name = "LEVEL")]
        effort: Option<String>,
        /// Print the amx command line instead of running it.
        #[arg(long)]
        dry_run: bool,
    },
    /// The body of a git hook stub: fire condition, depth guard, check, chain.
    Hook {
        /// pre-commit, commit-msg or pre-push.
        name: String,
        /// The stub's own path, so step 5 can refuse to chain into itself.
        #[arg(long, value_name = "PATH")]
        stub: Option<PathBuf>,
        /// Whatever git handed the stub.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

pub const USAGE: &str = "\
usage: workflow <command> [options]

  hygiene [--staged|--tree|--history <n>|--message <file>|--string <s>]
      [--path <dir>] [--json] [--fix]
      agent files and process references; no mode reads the tree and the
      last 200 commits; --fix untracks ignore-list files and their lines
      0 clean or warned · 1 hard finding · 2 usage
  lint-msg [<file>] [--string <text>]
      0 clean (warnings included) · 1 hard fail
  hook <name> [--stub <path>] [-- <args>]
      the body of a git hook stub; the stub's exit code is the hook's
  install
      write the embedded skills, agents and hook stubs where they are read,
      replacing symlinks with files and removing what older installs left
      0 installed · 1 something could not be written or removed
  go [<project>] [--milestone <slug>] [--model <m>] [--effort <l>] [--dry-run]
      start the orchestrator for the project's next milestone as an amx agent
      and print its name; --dry-run prints the amx command line instead
      0 started, or the roadmap is done · 1 mem or amx failed · 2 refused:
      the roadmap is not approved, or the project already has an orchestrator
";
