//! The command surface (spec §7 plus the three scripts §12b folded in).
//!
//! `help`, no command at all and an unknown command are handled in
//! [`crate::main`] rather than by clap, because their output and their exit
//! codes are part of the contract the test suite reads.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "workflow",
    version,
    about = "The verification gate and the interim orchestrator",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run this repo's suite over what is staged.
    Verify {
        /// The pre-commit path: the green cache and the opt-out ruling apply.
        #[arg(long)]
        hook: bool,
        /// The merge gate's path: authoritative, nothing downgrades it.
        #[arg(long)]
        gate: bool,
    },
    /// Check a commit message, a branch name or a PR body.
    #[command(name = "lint-msg")]
    LintMsg {
        /// The message file git hands the commit-msg hook.
        msgfile: Option<PathBuf>,
        /// Lint this text instead of a file.
        #[arg(long)]
        string: Option<String>,
    },
    /// Does this change set want a cold review?
    #[command(name = "review-needed")]
    ReviewNeeded {
        /// Also consider what this range changed.
        #[arg(long, value_name = "RANGE")]
        diff: Option<String>,
    },
    /// Run a plan's tasks in worktrees.
    Run {
        /// A plan file, instead of this project's plan in mem.
        #[arg(long = "plan-file", value_name = "FILE")]
        plan_file: Option<PathBuf>,
    },
    /// Collect finished or stalled workers.
    Reap,
    /// Check this machine's wiring.
    Doctor,
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
    /// Put a branch's work somewhere safe and get out of the way.
    #[command(
        long_about = "Put a branch's work somewhere safe and get out of the way.

Uncommitted work becomes a `wip:` commit, and everything from <base> to the
branch tip goes into a git bundle under the parked directory. `workflow resume`
puts it back, including the uncommitted state -- the wip commit is an envelope,
not a decision to commit that work.

Bundles live in ~/.local/share/workflow/parked/<project>/, which syncs on its
own unit. They never go near mem's store."
    )]
    Park {
        /// The checkout to park. Default: the working directory.
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// The branch to park. Default: the one checked out.
        #[arg(long, value_name = "NAME")]
        branch: Option<String>,
        /// Where the branch left the trunk. Default: worked out from the trunk.
        #[arg(long, value_name = "SHA")]
        base: Option<String>,
        /// Why it was parked, recorded in the meta file.
        #[arg(long, value_name = "TEXT")]
        note: Option<String>,
        /// The bundle's name. Default: the branch, slashes turned into dashes.
        #[arg(long, value_name = "LABEL")]
        name: Option<String>,
    },
    /// Bring a parked branch back, uncommitted state and all.
    #[command(long_about = "Bring a parked branch back, uncommitted state and all.

The branch is recreated from the bundle and checked out, then reset --soft to
the wip commit's parent: whatever was uncommitted when park ran is uncommitted
again.

All of it comes back staged. `workflow park` takes the whole working tree with
`git add -A`, untracked files included, so the wip commit does not record which
of it was in the index and which was not, and `reset --soft` puts the lot back
in the index. Content is restored exactly; the staged/unstaged split is not.")]
    Resume {
        /// The bundle to unpack.
        bundle: PathBuf,
        /// The checkout to restore into. Default: the one the meta file names.
        #[arg(long, value_name = "DIR")]
        repo: Option<PathBuf>,
        /// The branch to restore. Default: the one the meta file names.
        #[arg(long, value_name = "NAME")]
        branch: Option<String>,
        /// Overwrite an existing branch and a dirty working tree.
        #[arg(long)]
        force: bool,
    },
    /// Turn attribution off and set WORKFLOW_AGENT in a Claude Code settings file.
    #[command(name = "settings-merge")]
    SettingsMerge {
        /// Default: ${CLAUDE_CONFIG_DIR:-~/.claude}/settings.json
        file: Option<PathBuf>,
        /// Print what the merge would write, and write nothing.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    // ------------------------------------------------------------- test seams
    //
    // Hidden, and the port's replacement for sourcing the bash with
    // WORKFLOW_LIB=1: the parser, the ownership evaluator and the liveness
    // rule are checked against written-down expectations rather than against
    // themselves (AC12).
    /// Parse a plan file and report what the grammar made of it.
    #[command(name = "plan-check", hide = true)]
    PlanCheck {
        file: PathBuf,
        /// Print the parse as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print every record a task touched that its patterns do not claim.
    #[command(name = "ownership", hide = true)]
    Ownership {
        #[arg(long, value_name = "DIR")]
        repo: PathBuf,
        #[arg(long, value_name = "SHA")]
        base: String,
        #[arg(long, value_name = "REF")]
        branch: String,
        patterns: Vec<String>,
    },
    /// Split a Files: line into patterns, one per line.
    #[command(name = "split-patterns", hide = true)]
    SplitPatterns { line: String },
    /// Exit 0 when a task's three liveness signals are all older than the deadline.
    #[command(name = "stalled", hide = true)]
    Stalled {
        #[arg(long, value_name = "DIR")]
        rundir: PathBuf,
        #[arg(long, value_name = "DIR")]
        wtroot: PathBuf,
        #[arg(long, value_name = "SECONDS")]
        deadline: i64,
        task: String,
    },
}

pub const USAGE: &str = "\
usage: workflow <command> [options]

  verify [--hook|--gate]      run the repo's suite over what is staged
      0 green · 1 failed · 2 no verifier · 3 test removal
  lint-msg [<file>] [--string <text>]
      0 clean (warnings included) · 1 hard fail
  review-needed [--diff <range>]
      0 a cold review is wanted · 1 it is not
  run [--plan-file <f>]
      0 every task complete · 1 parked or failed tasks · 2 config or plan error
  reap
      0 nothing to do · 1 reaped something
  doctor
      0 healthy · 1 findings
  hook <name> [--stub <path>] [-- <args>]
      the body of a git hook stub; the stub's exit code is the hook's
  park [--repo <d>] [--branch <b>] [--base <sha>] [--name <l>] [--note <t>]
  resume <bundle> [--repo <d>] [--branch <b>] [--force]
  settings-merge [<file>] [--dry-run]
";
