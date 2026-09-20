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
    /// Append one status line for the task this worktree belongs to.
    Report {
        /// One of started, progress, ready, blocked.
        state: String,
        /// What happened, in one line.
        note: Option<String>,
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
    /// Read a plan and report what the grammar made of it. Nothing is run.
    #[command(
        name = "plan-check",
        long_about = "Read a plan and report what the grammar made of it. Nothing is run.

This is how a plan is checked before anyone approves it: it parses the file,
prints the plan id, the tasks and the waves they fall into, and touches
nothing else. Run it from the project checkout and the plan is judged against
the tree too: a Verify that cannot pass here is refused, and a Files line
that does not look like it can hold its task is warned about. `workflow run`
is the other thing -- it creates a worktree and a branch per task and
dispatches the first wave for real."
    )]
    PlanCheck {
        file: PathBuf,
        /// Print the parse as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run a plan's tasks in worktrees. This dispatches real workers.
    #[command(
        long_about = "Run a plan's tasks in worktrees. This dispatches real workers.

The run says at its start what it writes and reads with and where each dial
came from -- the environment, this plan's own record, the project key -- since
a plan picked up again keeps the record it wrote when it began whatever the
project keys say by then. The four flags here rewrite that record before the
run reads it, so `--model opus` changes what a resumed run dispatches on, and
keeps it for every later run of this plan."
    )]
    Run {
        /// A plan file, instead of this project's plan in mem.
        #[arg(long = "plan-file", value_name = "FILE")]
        plan_file: Option<PathBuf>,
        /// Record this as the model this plan's workers write with.
        #[arg(long, value_name = "NAME")]
        model: Option<String>,
        /// Record this as the model that reads this plan's merges.
        #[arg(long = "review-model", value_name = "NAME")]
        review_model: Option<String>,
        /// Record this as the workers' reasoning level.
        #[arg(long, value_name = "LEVEL")]
        effort: Option<String>,
        /// Record this as the reader's reasoning level.
        #[arg(long = "review-effort", value_name = "LEVEL")]
        review_effort: Option<String>,
    },
    /// Start the gate's own reader over a working tree or a range, and print its verdict.
    #[command(
        long_about = "Start the gate's own reader over a working tree or a range, and print its verdict.

The mechanism the merge gate uses, run cold and by hand: with no --range, the
diff is what the working tree carries against HEAD, untracked files inlined;
with one, `git diff <range>`. --against is what the diff is held to -- a
`wiki:<slug>` resolves through mem, plain text stands as written -- and
without it the project's own plan stands in, else a generic requirement.
Exit 0 is a ship verdict, 1 is fix, 2 is nobody named to read, 3 is a reading
that ended with no verdict."
    )]
    Read {
        /// git diff <r> instead of git diff HEAD.
        #[arg(long, value_name = "RANGE")]
        range: Option<String>,
        /// What the diff is held to.
        #[arg(long, value_name = "TEXT")]
        against: Option<String>,
    },
    /// Ask a stronger model at a decision point, without ending your turn.
    #[command(
        long_about = "Ask a stronger model at a decision point, without ending your turn.

Inside a run -- WORKFLOW_TASK set in the environment -- the question goes to
the advisor with the plan's rulings, this task's block and the pages its
Read: names; outside one, --against <text> says what the question is held to
and is required. Either way a --file is inlined under 24 KB, named with its
size past it. The answer prints and the task goes on: a decision -- scope,
taste, a broken plan -- is `mem ask`, never this. Exit 0 is an answer
printed, 1 is a session that ended or was stopped with none, 2 is nobody
named to advise or a fourth consult this attempt."
    )]
    Advise {
        question: String,
        /// A file to inline for the advisor, under 24 KB; repeatable.
        #[arg(long, value_name = "PATH")]
        file: Vec<PathBuf>,
        /// What the question is held to, outside a run.
        #[arg(long, value_name = "TEXT")]
        against: Option<String>,
    },
    /// A library's current documentation, through the Context7 CLI.
    Docs {
        /// The library, by the name its users know it.
        library: String,
        /// What about it, in a few words.
        query: String,
    },
    /// Collect finished or stalled workers.
    Reap,
    /// Ask the live run to dispatch a failed task again.
    #[command(long_about = "Ask the live run to dispatch a failed task again.

A run holds its project's lock for its whole life, so a failed task used to
wait for the run to end before anyone could act on it -- with the worker slot
it freed sitting idle (friction #W0S44DE6). This writes a marker in the live
run's directory; the run picks it up on its next poll and dispatches the task
again. A dispatched task is taken too: its session is stopped and the task
goes again on the commits it already has, so a plan edit reaches it now
rather than after a wasted attempt. --model names a model for this task's
dispatches for the rest of the run. With no live run, just run the plan
again -- a fresh run retries failed tasks by itself.")]
    Redispatch {
        task: String,
        /// The model this task is dispatched with from here on.
        #[arg(long)]
        model: Option<String>,
        /// Minutes the reading in flight may still take. Nothing is
        /// dispatched: the live run reads this every poll.
        #[arg(long = "review-deadline", value_name = "MIN")]
        review_deadline: Option<f64>,
    },
    /// Land a task the run failed, as it stands, with the findings filed.
    #[command(
        long_about = "Land a task the run failed, as it stands, with the findings filed.

Two fix rounds go by themselves and a third fix verdict is the orchestrator's;
so is a task that failed because the reading could not be had at all. With a
run live this writes a marker in its directory and the run merges the task's
branch on its next poll; with no run live it does that merge itself, off the
last run's state -- a run ends in the same pass as the verdict, so there is no
window to hand a marker to. Either way there is no reader this time --
ownership, the words, the rebase and the gate's own suite still stand -- and
every finding of the last reading becomes a mem follow-up (`mem log --type
followup`), so nothing true is lost and nothing minor costs another round. The
other way out is to edit the plan and `workflow redispatch <task>`."
    )]
    Accept { task: String },
    /// Report this project's runs: task states, spend, lock liveness.
    #[command(
        long_about = "Report this project's runs: task states, spend, lock liveness.

The run dir read out loud, and nothing touched: no dispatch, no cleanup, no
state change. --json is the shape a session that owns a run polls."
    )]
    Status {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Block until the live run needs the orchestrator, and say what for.
    #[command(
        long_about = "Block until the live run needs the orchestrator, and say what for.

The run appends one line per event to its run dir -- a worker's question, a
task failed for good, a merge, the end of the run -- and this waits on that
file, printing each new line, and exits the moment there is something to act
on. Exit 2: a question is pending (`mem questions --pending --for
orchestrator`, then `mem answer`). Exit 1: a task failed and the run will not
retry it by itself; `workflow status` says why. Exit 0: the run ended, or no
run is live here. Exit 4: a merge, only under --merges. Exit 3: --timeout
passed with nothing new. A cursor in the run dir remembers what was already
reported, so calling this again after acting picks up where it left off.

Call it in the foreground and act on the exit code; it returns 3 after the
timeout (300 s unless --timeout says otherwise) with one line per live task,
so a session is never held past what it can afford to miss."
    )]
    Wait {
        /// Give up after this many seconds with exit 3. Bounded by default:
        /// a session holding one call for half an hour cannot hear a
        /// message queued behind it.
        #[arg(long, value_name = "SECONDS", default_value_t = 300)]
        timeout: u64,
        /// Return on each merge too, with exit 4.
        #[arg(long)]
        merges: bool,
    },
    /// Check this machine's wiring.
    Doctor {
        /// Write the embedded skills and hook stubs where they are missing,
        /// differ or are a symlink.
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
    /// Print this binary's skills, or one of them whole.
    #[command(long_about = "Print the skills this binary carries.

With no name, one `<name> — <description>` line per skill, in name order: this
is what `mem context` appends to a project's digest, so a session learns which
skills exist where mem knows the project. With a name, that SKILL.md whole.

The skills are embedded in the binary rather than written to a harness's skills
directory, so there is no copy to drift and nothing on disk to gate. mem serves
the mem skill the same way: `mem skill mem`.")]
    Skill {
        /// The skill to print whole. Omit to list them.
        name: Option<String>,
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
    // WORKFLOW_LIB=1: the ownership evaluator and the liveness rule are
    // checked against written-down expectations rather than against
    // themselves (AC12). The parser's seam is `plan-check`, which turned out
    // to be the verb a planner wanted too, and is no longer hidden.
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
  docs <library> <query>      a library's current docs through Context7
      0 printed · 1 nothing came
  report <state> [<note>]     append one status line for this worktree's task
      0 written · 2 not a state, or not a run worktree
  lint-msg [<file>] [--string <text>]
      0 clean (warnings included) · 1 hard fail
  review-needed [--diff <range>]
      0 a cold review is wanted · 1 it is not
  plan-check <file> [--json]
      read a plan and report its tasks and waves; nothing is run
      0 it holds · 1 the grammar or this checkout refused it · 2 no such file
  run [--plan-file <f>] [--model <m>] [--review-model <m>]
      [--effort <l>] [--review-effort <l>]
      run a plan's tasks in worktrees; this dispatches real workers
      the dials rewrite this plan's own record, which a resumed run prefers
      to the project keys; the run says at its start which it took
      0 every task complete · 1 failed tasks · 2 config or plan error
  read [--range <r>] [--against <text>]
      start the gate's own reader over a working tree or a range
      0 ship · 1 fix · 2 no reader named, or nothing to read · 3 no verdict
  advise <question> [--file <path>]... [--against <text>]
      ask a stronger model at a decision point, without ending your turn
      0 answered · 1 no answer · 2 nobody named, or a fourth consult
  reap
      0 nothing to do · 1 reaped something
  redispatch <task> [--model <name>] [--review-deadline <min>]
      ask the live run to dispatch a failed task again, or -- with
      --review-deadline -- give the reading in flight more time and
      dispatch nothing
      0 the run was asked · 1 no live run holds that task failed, or its wave closed
  accept <task>
      land a task the run failed, as it stands, the findings filed
      0 merged, or the live run was asked · 1 it did not merge · 2 nothing to merge
  status [--json]
      report this project's runs: task states, spend, lock liveness
      0 reported · 2 outside a project
  doctor [--fix]
      --fix writes the hook stubs where they are missing, differ or are a
      symlink, and takes the skills an older --fix installed back off disk
      0 healthy · 1 findings
  hook <name> [--stub <path>] [-- <args>]
      the body of a git hook stub; the stub's exit code is the hook's
  skill [<name>]
      the skills this binary carries, one `name — description` line each;
      with a name, that SKILL.md whole
  settings-merge [<file>] [--dry-run]
";
