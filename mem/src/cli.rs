//! Command-line surface.

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
    /// `MEM_PROJECT` in the environment does the same when the flag is absent.
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
    /// The session digest for this project.
    Context {
        /// Which project, when it is not the working directory's.
        project: Option<String>,
        /// The whole digest: facts, logs and the plan's next task.
        #[arg(long)]
        full: bool,
        /// Override the full digest's assembly target in bytes.
        #[arg(long)]
        budget: Option<usize>,
        /// A hook-sized summary instead of the digest. Text only: it has no
        /// --json form.
        #[arg(long)]
        brief: bool,
        /// Wrap the output in the runtime's hook envelope.
        #[arg(long)]
        hook_json: bool,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// The instruction a compaction summarizer must follow (the PreCompact hook).
    Precompact {
        /// Accepted for symmetry: PreCompact's channel is plain stdout, so the
        /// output is the same either way.
        #[arg(long)]
        hook_json: bool,
    },
    /// Full-text search across this project and global scope.
    Search {
        /// The query. FTS5 syntax, including title:, body: and tags: filters.
        /// Left out with --kind, the kind is listed newest first.
        query: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long = "type")]
        r#type: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        min_score: Option<f64>,
    },
    /// Not a verb: says which ones list.
    #[command(hide = true)]
    List,
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
        /// Narrow to one kind. Log entries when nothing else narrows the
        /// read; with --type and no --kind, every kind.
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
        /// The answer the asker would pick.
        #[arg(long, value_name = "TEXT")]
        recommend: Option<String>,
        /// Who answers. A person unless this says otherwise, and a person's
        /// questions are what the hub and the phone show.
        #[arg(long = "for", value_name = "AUDIENCE")]
        audience: Option<Audience>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// List questions, or wait for one to be answered.
    Questions {
        #[arg(long)]
        pending: bool,
        #[arg(long)]
        all_projects: bool,
        /// Only the questions this audience answers.
        #[arg(long = "for", value_name = "AUDIENCE")]
        audience: Option<Audience>,
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
    /// Record a decision: who made it, and what it replaces.
    Decide {
        text: String,
        #[arg(long)]
        by: DecidedBy,
        /// The decision this one replaces, in words.
        #[arg(long)]
        replaces: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// File a task's evidence, or list what is filed.
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommand,
    },
    /// File what a milestone's check turned up, list findings, or close one.
    Finding {
        #[command(subcommand)]
        command: FindingCommand,
    },
    /// Keep a source as it was taken: a file copied, or a URL fetched.
    Raw {
        #[command(subcommand)]
        command: RawCommand,
    },
    /// The project brief: print the newest, or set a new one.
    Brief {
        #[arg(long)]
        set: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Record an idea for later.
    Idea {
        text: String,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// The project's wiki: list pages, print one, or replace one. `mem wiki
    /// lint` checks the wiki and exits 1 on findings.
    Wiki {
        /// The page slug. Without one, list every page this project has. With
        /// `#<heading-slug>` after it, one section of the page.
        slug: Option<String>,
        /// With `index`: keep the prose above the first `- [` line and rewrite
        /// the rest as one line per page.
        #[arg(long, conflicts_with_all = ["stdin", "note", "sections"])]
        rebuild: bool,
        /// Replace the page, or the section named, with what is on stdin.
        #[arg(long)]
        stdin: bool,
        /// List the page's sections: heading slug, bytes and heading.
        #[arg(long, conflicts_with_all = ["stdin", "note"])]
        sections: bool,
        /// What changed and why. Mandatory on a write: the note becomes the
        /// log line that is the page's history.
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Print plan.md verbatim, or replace or clear it. With a slug, the stored
    /// plan of that milestone instead.
    Plan {
        /// A milestone's stored plan. Without one, the current plan.
        slug: Option<String>,
        #[arg(long)]
        set_file: Option<std::path::PathBuf>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        clear: bool,
        /// Check off one task by its plan id, in place.
        #[arg(long, value_name = "TASK-ID", conflicts_with_all = ["set_file", "stdin", "clear"])]
        tick: Option<String>,
        /// List the stored plans: slug, bytes, date and title.
        #[arg(long, conflicts_with_all = ["slug", "set_file", "stdin", "clear", "tick"])]
        list: bool,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// Print roadmap.md verbatim, or replace, clear or tick it.
    Roadmap {
        #[arg(long)]
        set_file: Option<std::path::PathBuf>,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        clear: bool,
        /// Check off one milestone by its slug, in place.
        #[arg(long, value_name = "SLUG", conflicts_with_all = ["set_file", "stdin", "clear"])]
        tick: Option<String>,
        /// Print the roadmap's status, or set it: draft or approved.
        #[arg(long, value_name = "STATUS", num_args = 0..=1, conflicts_with_all = ["set_file", "stdin", "clear", "tick"])]
        status: Option<Option<String>>,
        #[arg(long)]
        session_id: Option<String>,
    },
}

/// Who made a decision: Saiful, or an agent taking the default.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum DecidedBy {
    Saiful,
    Agent,
}

impl DecidedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            DecidedBy::Saiful => "saiful",
            DecidedBy::Agent => "agent",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum EvidenceCommand {
    /// Copy a file into the project's evidence for a task, with a note.
    Add {
        #[arg(long)]
        task: String,
        file: std::path::PathBuf,
        #[arg(long)]
        note: String,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// One `#<id>  <task>  <file>  <note>` line per item, newest first.
    List,
    /// Write the file an evidence item or a finding names to stdout, as stored.
    Cat { id: String },
}

#[derive(Subcommand, Debug)]
pub enum FindingCommand {
    /// File a finding against a milestone's step. It starts open.
    Add {
        #[arg(long)]
        milestone: String,
        #[arg(long)]
        step: String,
        text: String,
        /// A file that shows it, copied into the project's evidence.
        #[arg(long)]
        evidence: Option<std::path::PathBuf>,
        #[arg(long)]
        session_id: Option<String>,
    },
    /// One `#<id>  <status>  <milestone>  <step>  <title>` line per finding.
    List {
        /// Only the findings nothing has fixed yet.
        #[arg(long)]
        open: bool,
    },
    /// Mark a finding fixed by a commit, in place.
    Close {
        id: String,
        #[arg(long)]
        by: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RawCommand {
    /// Store a file or URL under the project's raw/. A name already there
    /// is refused: a raw source is never replaced.
    Add { source: String },
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
    /// Forget something `set` recorded, so the project is back on the
    /// default. `set` refuses an empty value, so this is the way back to
    /// absent.
    Unset { key: ProjectKey },
}

/// The keys `set` records and `unset` clears. `remote` is not one: it is the
/// project's identity across machines, not a choice to fall back from.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum ProjectKey {
    Verify,
    HygieneExempt,
}

impl ProjectKey {
    /// The key as project.toml spells it.
    pub fn stored(self) -> &'static str {
        match self {
            ProjectKey::Verify => "verify",
            ProjectKey::HygieneExempt => "hygiene_exempt",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ProjectSetCommand {
    /// The command that verifies this project. Whatever it says beats any
    /// verifier a caller would detect on its own.
    Verify { cmd: String },
    /// Globs the hygiene check leaves alone, whitespace separated
    /// (`tests/**`): paths whose fixtures have to carry the ids and words
    /// the check refuses elsewhere.
    #[command(name = "hygiene-exempt")]
    HygieneExempt { globs: String },
    /// This project's origin remote, for one registered before the remote
    /// existed. Normalized exactly as registration normalizes `origin`.
    Remote { url: String },
}

/// Who a question is for. An orchestrator's question never reaches the hub;
/// a person's is what the hub shows.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum Audience {
    Orchestrator,
    Human,
}

impl Audience {
    /// The frontmatter value; a person is the absence of one.
    pub fn stored(self) -> Option<&'static str> {
        match self {
            Audience::Orchestrator => Some("orchestrator"),
            Audience::Human => None,
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
