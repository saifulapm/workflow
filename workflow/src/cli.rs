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
        group(ArgGroup::new("scope").args(["staged", "tree", "history", "message", "string", "would_create"])),
        long_about = "Look for agent files and process references in a repository.

The hard tier exits 1: a file on the global ignore list tracked or staged, a
.gitignore line naming one, and in the added lines of non-markdown files or in
a message a numbered ruling, milestone, ticket, issue or ADR, a memory id, a
co-author or generated-with line. A message also fails on a subject over 72
characters and on a bare task or milestone id. The soft tier only warns, reads
messages only, and is cleared per term by a lint-exception ruling. Paths the
project key hygiene-exempt names are not read for content.

--would-create asks about one path before it is written: it exits 1 and names
it only when it does not exist yet, lands in a work tree mem knows, and is an
agent instruction file there.

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
        /// Would writing this path add an agent instruction file?
        #[arg(long, value_name = "PATH")]
        would_create: Option<PathBuf>,
        /// With --staged, read nothing in a checkout mem does not know.
        #[arg(long, requires = "staged")]
        known: bool,
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

  hygiene [--staged [--known]|--tree|--history <n>|--message <file>
      |--string <s>|--would-create <path>] [--path <dir>] [--json] [--fix]
      agent files and process references; no mode reads the tree and the
      last 200 commits; --fix untracks ignore-list files and their lines;
      --would-create refuses a new agent file in a checkout mem knows
      0 clean or warned · 1 hard finding · 2 usage
  lint-msg [<file>] [--string <text>]
      0 clean (warnings included) · 1 hard fail
  hook <name> [--stub <path>] [-- <args>]
      the body of a git hook stub; the stub's exit code is the hook's
";
