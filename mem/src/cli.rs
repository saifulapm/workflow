//! Command-line surface (spec §7).

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "mem",
    version,
    about = "The system of record for AI workflow state",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Act on this project instead of the one inferred from the working directory.
    #[arg(long, global = true, value_name = "NAME")]
    pub project: Option<String>,

    /// Scope of a read: project (project ∪ global), global, or all.
    #[arg(long, global = true, value_name = "SCOPE")]
    pub scope: Option<Scope>,

    /// Include archived items.
    #[arg(long, global = true)]
    pub include_archived: bool,

    /// Machine-readable output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress non-essential output.
    #[arg(long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// The session digest for this project (spec §8).
    Context {
        /// Which project, when it is not the working directory's.
        project: Option<String>,
        /// Override the assembly target in bytes.
        #[arg(long)]
        budget: Option<usize>,
        /// A hook-sized summary instead of the digest.
        #[arg(long)]
        brief: bool,
        /// Wrap the output in the runtime's hook envelope.
        #[arg(long)]
        hook_json: bool,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Nudge a session that recorded nothing (the Stop hook, spec §9).
    SessionCheck {
        #[arg(long)]
        session_id: Option<String>,
        /// Wrap the nudge in the runtime's hook envelope.
        #[arg(long)]
        hook_json: bool,
    },
    /// The instruction a compaction summarizer must follow (the PreCompact hook).
    Precompact {
        /// Accepted for symmetry: PreCompact's channel is plain stdout, so the
        /// output is the same either way (spec §9).
        #[arg(long)]
        hook_json: bool,
    },
    /// Full-text search across this project and global scope.
    Search {
        /// The query. FTS5 syntax, including title:, body: and tags: filters.
        query: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long = "type")]
        r#type: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        min_score: Option<f64>,
    },
    /// Print items by full ULID or exact 8-character suffix.
    Show {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// List the projects the store knows about.
    Projects,
    /// Questions about one project.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Record a durable fact or ruling.
    Save {
        text: String,
        #[arg(long, default_value = "fact")]
        kind: String,
        #[arg(long = "type")]
        r#type: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long)]
        supersedes: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// With text, append a log entry; without, list recent entries.
    Log {
        text: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long = "type")]
        r#type: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// The session handoff: print the latest, or set a new one.
    Handoff {
        #[arg(long)]
        set: Option<String>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Print status.md verbatim, or replace it.
    Status {
        #[arg(long)]
        set: Option<String>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Rebuild the index from the store.
    Reindex {
        #[arg(long)]
        full: bool,
    },
    /// Write a dated tarball of the store.
    Snapshot,
    /// List stale items, or archive the named ones in place.
    Prune {
        /// Short ids or full ULIDs to archive in place; `all` takes every
        /// candidate. An id that is not a candidate is exit 1.
        #[arg(long, num_args = 1.., value_name = "ID")]
        apply: Vec<String>,
    },
    /// Ask qshell-sync for a round and verify that one happened.
    Sync,
    /// Health checks over the store, the index and the sync unit.
    Doctor {
        #[arg(long)]
        fix: bool,
    },
    /// Ask a blocking question. Never waits: it writes, notifies and returns.
    Ask {
        question: String,
        #[arg(long, value_delimiter = ',')]
        options: Vec<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// List questions, or wait for one to be answered.
    Questions {
        #[arg(long)]
        pending: bool,
        #[arg(long)]
        all_projects: bool,
        /// Wait for this question to be answered.
        #[arg(long, value_name = "ID")]
        wait: Option<String>,
        /// At most 5m, which is also the default.
        #[arg(long, default_value = "5m")]
        timeout: String,
    },
    /// Answer a question, from any machine.
    Answer {
        id: String,
        text: Option<String>,
        #[arg(long)]
        option: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// The project's wiki: list pages, print one, or replace one.
    Wiki {
        /// The page slug. Without one, list every page this project has.
        slug: Option<String>,
        /// Replace the page with what is on stdin.
        #[arg(long)]
        stdin: bool,
        /// What changed and why. Mandatory on a write: the note becomes the
        /// log line that is the page's history.
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Print plan.md verbatim, or replace or clear it.
    Plan {
        #[arg(long)]
        set_file: Option<std::path::PathBuf>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        clear: bool,
        /// Check off one task by its plan id, in place.
        #[arg(long, value_name = "TASK-ID", conflicts_with_all = ["set_file", "stdin", "clear"])]
        tick: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// Print the working directory's project — id, name and checkout root.
    /// Exit 1 when this directory belongs to no project mem knows.
    Current,
    /// Register a subdirectory of this checkout as its own project, so a
    /// monorepo holds one project per app beside the root project. Sessions
    /// in that subdirectory then resolve to the child; everything else in
    /// the checkout stays with the root.
    Add {
        /// The directory, relative to the checkout toplevel.
        subdir: String,
        /// The project name; the subdirectory's basename when absent.
        #[arg(long)]
        name: Option<String>,
    },
    /// Record something about this project in the store.
    Set {
        #[command(subcommand)]
        command: ProjectSetCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProjectSetCommand {
    /// The command that verifies this project. It is tier one of the
    /// verification ladder: whatever it says beats every detected verifier.
    Verify { cmd: String },
    /// Globs this project wants a cold review of, whitespace separated.
    /// Merged with `workflow review-needed`'s global table, never replacing
    /// it: these are the paths that are load-bearing in THIS repository.
    #[command(name = "review-paths")]
    ReviewPaths { globs: String },
    /// The worker this project's tasks are dispatched onto. Absent means
    /// claude, which is what every project ran on before there was a choice.
    Backend { backend: Backend },
    /// This project's origin remote, for one registered before the remote
    /// existed. Normalized exactly as registration normalizes `origin`.
    Remote { url: String },
}

/// The workers a task can be dispatched onto. A closed list: an unknown name
/// is a typo, and a typo must not leave a project pointing at nothing.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum Backend {
    Claude,
    Amx,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::Amx => "amx",
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[value(rename_all = "lower")]
pub enum Scope {
    /// The current project plus global, with global ranked lower.
    #[default]
    Project,
    Global,
    All,
}
