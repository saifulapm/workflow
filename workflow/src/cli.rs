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
        /// Print the amx command line instead of running it; with --cloud,
        /// print the prompt, and record and start nothing, in a thread too.
        #[arg(long)]
        dry_run: bool,
        /// The cloud run of the milestone, on the branch
        /// cloud/<project>-<milestone>. A machine starts none: the project's
        /// Claude Project does, and --cloud alone refuses, naming its start
        /// line.
        #[arg(long)]
        cloud: bool,
        /// With --cloud: print the run's prompt. In a cloud session, a thread
        /// of the Claude Project, it first claims the run in mem as the
        /// session's own, and prints a done: or parked: line instead when
        /// there is nothing to run or another run holds the project.
        #[arg(long, requires = "cloud")]
        print: bool,
    },
    /// Land a cloud run's milestone on its trunk, from a thread of its Claude Project.
    #[command(
        long_about = "Land a cloud run's milestone on its trunk, from a thread of its Claude Project.

Run in the project's checkout on the milestone branch cloud/<project>-<milestone>,
clean. The trunk is origin's app/<project> when origin has it, an app's own
branch in a monorepo, or else main. It rebases the branch on the trunk, runs
the project's test command (mem's verify key) in the project's folder, and
pushes it to the trunk, never forced; a trunk that moved while the tests ran
is rebased onto again, up to three times. Then it ticks the milestone. An app
then moves onto main the same way: rebased on origin/main, tested, main
fast-forwarded, and app/<project> moved to match with a lease; a conflict or
a red run there leaves main untouched and prints `parked: main moved:
<project>: ...`, exit 2. It commits and pushes the writes the store has not
taken, and prints the line the thread ends its report on: `next: Run the next
milestone of <project>.`, `done: <project>'s roadmap is done`, or `parked:
<project> is paused`. A rebase conflict on the trunk pushes nothing and prints
`parked: trunk moved: ...`, exit 2; a red test run pushes nothing, exit 1. The
rebased branch itself goes over origin's copy, leased, so the thread's next
push of it is a fast-forward; a copy with commits the checkout lacks is kept
and land refuses, exit 1.

A project that names a GitHub Actions workflow (`mem project set land-ci ci.yml`)
waits on it before each push of the trunk: after the local tests pass, land
pushes the rebased sha to ci/<project>-<milestone>, leased, prints `land:
waiting on <file> for <sha>`, and reads the workflow's run of that sha on that
ref with `gh api`. A green run lets the push go; otherwise it pushes nothing to
the trunk, exit 2, and prints `parked: CI red on <sha>: <failed jobs>`,
`parked: no <file> run for <sha>` (none in 10 minutes), `parked: <file> still
running on <sha>` (after 90 minutes) or `parked: GitHub refused the Actions API
for <owner/repo>: <why>`. Run again on the same sha, it reads the same run."
    )]
    Land,
    /// Print the text for the Claude Project instructions field of a project's repository.
    #[command(
        name = "project-text",
        long_about = "Print the text for the Claude Project instructions field of a project's repository.

One Claude Project serves a repository: its root project and every app mem
keeps as a project inside it, so any of them prints the same text.
claude.ai has no API for Projects, so the text is pasted once, into Settings >
Memory > Project instructions, which every new thread and the coordinator
read. It names the start line, \"Run the next milestone of <project>.\", tells
the coordinator to start a thread only on that line or a report ending
`next: ...`, and never a second thread of one project, and tells a thread to
cd into its clone, check mem knows the project, run `workflow go <project>
--cloud --print` and follow it."
    )]
    ProjectText {
        /// A project of the repository, by mem's name; the current directory's when omitted.
        project: Option<String>,
    },
    /// Start planning a project in a claude.ai cloud session.
    #[command(long_about = "Start planning a project in a claude.ai cloud session.

The session runs the plan skill and its cloud.md on the branch
cloud/<project>-plan, asks Saiful through mem and waits there for his answers,
and records itself as the project's cloud runner with the milestone `plan`. The
first milestone's thread in the project's Claude Project is cut from that
branch. Only
--cloud exists: a device plans in the chat with the plan skill.")]
    Plan {
        /// The project, by mem's name; the current directory's when omitted.
        project: Option<String>,
        /// Start it in a claude.ai cloud session.
        #[arg(long, required = true)]
        cloud: bool,
        /// Print the session's prompt and start nothing.
        #[arg(long)]
        print: bool,
    },
    /// Print the setup script of the claude.ai cloud environment `workflow`.
    #[command(
        name = "cloud-setup",
        long_about = "Print the setup script of the claude.ai cloud environment `workflow`.

It writes this repository's skills and agents under ~/.claude, and a mem in
/usr/local/bin that takes the x86_64 mem and hub from the public cloud-bins
release, and once a boot their newer builds and the skills, or without the
store's variables from $MEM_REMOTE/dl, where the proxy adds the Access token.

With --bootstrap it prints the short script a Claude Project's environment
holds instead: it fetches that full setup from the release, where CI puts it
with its sum on every push to main, checks the sum and runs it."
    )]
    CloudSetup {
        /// Print the bootstrap that fetches and runs the full setup.
        #[arg(long)]
        bootstrap: bool,
    },
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
    #[command(long_about = "Send questions to the phone or to this machine.

`route phone` has questions ring on the phone. `route device` has them ring
on this machine, as quickshell popups. With no argument it asks where they
ring now. It talks to this machine's local hub, which sends a change on to
every other local hub and writes the file qshell reads. Each form prints one
line, `route: phone` or `route: device (<machine>)`. Exit 1 when the local hub
refuses or does not answer.")]
    Route {
        /// phone or device; leave out to read the current route.
        #[arg(value_parser = ["phone", "device"])]
        to: Option<String>,
    },
    /// Run one call on the routed device, as if the program ran here.
    #[command(
        long_about = "Run one call on the routed device, as if the program ran here.

The first argument is the verb: whoami, run or browser. The rest go to the
device exactly as given, quotes and spaces unchanged, hyphens included. The
call goes to the device's hub, which answers with the program's stdout,
stderr and exit code, and the files it wrote. They land here: stdout and
stderr as given, each file at its relative path under .playwright-cli/ in
the current folder (any other path is skipped and named), then the exit
code. When the machine answering differs from the last one, a line says so.
The client decides four exits: 2 for a wrong verb or no project (set
MEM_PROJECT), 3 when no device is online, 4 when Access refuses the call, and
124 when the device does not answer: Cloudflare's 100 s, or this command's
110 s (WORKFLOW_DEVICE_TIMEOUT_S). The device stops a call at 90 s.
WORKFLOW_DEVICE_URL moves the hub's address. Exit 1 for any other answer."
    )]
    Device {
        /// verb (whoami, run or browser), then its arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run Shopify CLI with a theme's or an app's own variables.
    #[command(
        long_about = "Run Shopify CLI with a theme's or an app's own variables.

One cloud environment holds every theme's and app's secrets, each named after
it: <THEME>_SHOPIFY_CLI_THEME_TOKEN, <THEME>_SHOPIFY_FLAG_STORE and
<THEME>_STORE_PASSWORD, and <APP>_SHOPIFY_SERVICE_ACCOUNT_TOKEN, in capitals
with - as _. They go to the CLI under its own names.

theme-dev <theme> runs shopify theme dev on themes/<theme> (in this folder or
one above it), served at http://127.0.0.1:9292, on the store's unpublished
theme '<theme> (cloud)', made the first time and synced whole from the
checkout at each start, so no VM leaves a development theme behind. A theme
whose assets/ is missing or empty is refused, exit 2: build it first, since
the sync deletes what the checkout lacks.

deploy <app> runs shopify app deploy --config staging --allow-updates on
apps/<app>, never another config: production stays Saiful's go-ahead. It
refuses a staging config with production's client_id, or with no handle or
[build] dev_store_url. Once deployed, its last line is 'walk: <the app's page
in the dev store's admin>', from those two.

Exit 3 when a variable or the staging config is missing, or Shopify rejects
the token, naming it: only Saiful makes those, so the task parks. Exit 2 when the folder is not
there, 1 when the CLI fails otherwise."
    )]
    Shopify {
        /// theme-dev or deploy.
        #[arg(value_parser = ["theme-dev", "deploy"])]
        verb: String,
        /// The theme or app, as its folder under themes/ or apps/ is named.
        name: String,
    },
    /// Join this machine's memory to the Cloudflare store.
    #[command(long_about = "Join this machine's memory to the Cloudflare store.

chezmoi runs it on apply when the hub's local.env has no store token: a store
token minted for this machine, hub share seed, mem share switch, a second
seed, the store keys written to the hub's local.env, and hub-local
restarted. Running sessions do not stop it: the second seed carries what
they write meanwhile. A first seed that runs out of today's import budget
refuses, exit 2, before mem switches. Running it again on a switched machine
moves the store's URL, mints the machine's token again and restarts
hub-local.")]
    Switch {
        /// The store's socket, ws:// or wss://, ending in /store/share.
        #[arg(long, value_name = "URL")]
        store: String,
    },
    /// A check a Claude Code PreToolUse hook runs before a tool call.
    Guard {
        /// pnpm: refuse pnpm in a linked worktree over a linked node_modules.
        /// bash: refuse a command that repeats a lesson's mistake, for every
        /// session.
        #[arg(value_parser = ["pnpm", "bash"])]
        check: String,
    },
    /// Copy stdin to stdout with secret tokens (shpss_, shpat_, cfut_, tskey-)
    /// cut to their prefix and `***`. Pipe a page snapshot through it.
    Mask,
    /// A Claude Code worktree hook, given its JSON on stdin.
    #[command(long_about = "A Claude Code worktree hook, given its JSON on stdin.

create, the WorktreeCreate hook, adds the worktree <main checkout>/.claude/worktrees/<name>
on a new branch worktree-<name> at the HEAD of the session's own checkout, and
prints its path. A git failure exits 1 with git's message and leaves no folder.

remove, the WorktreeRemove hook, removes the worktree at worktree_path and its
branch. A worktree with uncommitted or untracked files stays, exit 1; a branch
that is not merged stays. Each says what it kept on stderr.")]
    Worktree {
        /// create: the WorktreeCreate hook. remove: the WorktreeRemove hook.
        #[arg(value_parser = ["create", "remove"])]
        action: String,
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
      and print its name; --dry-run prints the amx command line instead, or
      for --cloud the prompt; --cloud --print in a thread of a Claude Project
      claims the run first; --cloud alone refuses, naming the start line
      0 started, or the roadmap is done · 1 mem or amx failed · 2 refused:
      the roadmap is not approved, the milestone already has an orchestrator,
      or the project is paused and its handoff does not start parked:
  land
      land a cloud run's milestone branch on origin's main: rebase, test,
      push without force, tick, and print the report's last line
      0 landed · 1 tests red or mem failed · 2 trunk moved, or not on a
      clean milestone branch
  plan [<project>] --cloud [--print]
      start planning the project in a claude.ai cloud session on the branch
      cloud/<project>-plan and print its link; --print prints its prompt
      0 started · 1 mem or claude failed · 2 refused: no remote, main not
      pushed, or its own cloud run is live
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
