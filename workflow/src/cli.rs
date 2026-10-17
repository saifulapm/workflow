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
another live amx agent is running that milestone's orchestrator; the caller
itself ($AMX_ID) does not count, so an orchestrator can start its successor.
The agent is `amx new --name <project>-<milestone>` in the project's checkout,
on the goal that milestone's landing is the proof of; a name amx has taken by
an agent that ended gets a -2, -3, ... suffix. Claude runs under Remote Control
by the same name, so the Claude app's Code tab lists it. Prints the agent's name."
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
        /// Start the milestone in a claude.ai cloud session on the branch
        /// cloud/<project>-<milestone>, and record it as the cloud runner.
        #[arg(long)]
        cloud: bool,
        /// With --cloud: print the session's prompt and start nothing.
        #[arg(long, requires = "cloud")]
        print: bool,
    },
    /// Print the setup script of the claude.ai cloud environment `workflow`.
    #[command(
        name = "cloud-setup",
        long_about = "Print the setup script of the claude.ai cloud environment `workflow`.

It writes this repository's skills and agents under ~/.claude, and a mem in
/usr/local/bin that fetches the x86_64 mem from $MEM_REMOTE/dl/mem the first
time it runs, inside the session, where the proxy adds the Access token."
    )]
    CloudSetup,
    /// Print the steps that set up the claude.ai cloud environment
    /// `workflow` on the Cloudflare store.
    #[command(
        name = "cloud-switch",
        long_about = "Print the steps that set up the claude.ai cloud environment `workflow` on
the Cloudflare store, on a new account or to check one: the logins, GitHub,
the environment's hosts, variables and setup script, the connector and the
default environment. It refuses, exit 2, naming each cloud run that is live
or parked, and with a bad --store. It runs no pass command and prints no
secret, only the pass entry names."
    )]
    CloudSwitch {
        /// The store's socket, ws:// or wss://, ending in /store/share.
        #[arg(long, value_name = "URL")]
        store: String,
    },
    /// Pause a project: `go` refuses it unless its handoff starts `parked:`.
    Pause {
        /// The project, by mem's name.
        project: String,
        /// Who paused it; this machine's name when omitted.
        #[arg(long, value_name = "WHO")]
        by: Option<String>,
    },
    /// Clear a project's pause. Starts nothing.
    Unpause {
        /// The project, by mem's name.
        project: String,
    },
    /// Send questions to the phone or to this machine.
    #[command(
        long_about = "Send questions to the phone or to this machine.

`route phone` has questions ring on the phone. `route device` has them ring
on this machine, as quickshell popups. With no argument it asks where they
ring now. It talks to this machine's local hub, which sends a change on to
every other local hub and writes the file qshell reads. Each form prints one
line, `route: phone` or `route: device (<machine>)`. Exit 1 when the local hub
refuses or does not answer."
    )]
    Route {
        /// phone or device; leave out to read the current route.
        #[arg(value_parser = ["phone", "device"])]
        to: Option<String>,
    },
    /// Join this machine's memory to the Cloudflare store.
    #[command(
        long_about = "Join this machine's memory to the Cloudflare store.

chezmoi runs it on apply when the hub's local.env has no store token: a store
token minted for this machine, hub share seed, mem share switch, a second
seed, the store keys written to the hub's local.env, and hub-local
restarted. Running sessions do not stop it: the second seed carries what
they write meanwhile. A first seed that runs out of today's import budget
refuses, exit 2, before mem switches. Running it again on a switched machine
moves the store's URL, mints the machine's token again and restarts
hub-local."
    )]
    Switch {
        /// The store's socket, ws:// or wss://, ending in /store/share.
        #[arg(long, value_name = "URL")]
        store: String,
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
      the roadmap is not approved, the milestone already has an orchestrator,
      or the project is paused and its handoff does not start parked:
  pause <project> [--by <who>]
      hold the project once its current milestone lands; starts nothing
  unpause <project>
      let go start its next milestone again; starts nothing
      0 done · 1 mem failed · 2 no such project
  route [phone|device]
      send questions to the phone or to this machine; no argument shows
      where they go now
      0 done · 1 the local hub refused or did not answer
";
