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
        /// Who answers. Asked from an orchestrated task's worktree (or with
        /// WORKFLOW_TASK set) the default is the orchestrator; anywhere else
        /// it is a person, which is what the hub and the phone show.
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
    /// Print plan.md verbatim, or replace or clear it. With a slug, the stored
    /// plan of that milestone instead.
    Plan {
        /// A milestone's stored plan. Without one, the plan of record.
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
        /// Make a stored plan the plan of record. Refused while the current one
        /// still holds an unchecked task.
        #[arg(long, value_name = "SLUG", conflicts_with_all = ["slug", "set_file", "stdin", "clear", "tick", "list"])]
        from: Option<String>,
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
    /// Forget something `set` recorded, so the project is back on the
    /// default: the workflow's model, the detected verifier. `set` refuses an empty value, so this is the way back to
    /// absent. An absent `review-model` is not the same as no reader: a run
    /// stops and asks for one. `mem project set review-model none` is how a
    /// project records that nobody reads.
    Unset { key: ProjectKey },
}

/// The keys `set` records and `unset` clears. `remote` is not one: it is the
/// project's identity across machines, not a choice to fall back from.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum ProjectKey {
    Verify,
    ReviewPaths,
    Model,
    ReviewModel,
    Effort,
    ReviewEffort,
}

impl ProjectKey {
    /// The key as project.toml spells it.
    pub fn stored(self) -> &'static str {
        match self {
            ProjectKey::Verify => "verify",
            ProjectKey::ReviewPaths => "review_paths",
            ProjectKey::Model => "model",
            ProjectKey::ReviewModel => "review_model",
            ProjectKey::Effort => "effort",
            ProjectKey::ReviewEffort => "review_effort",
        }
    }
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
    /// The model a run's workers are started on (`opus`, `sonnet`, ...).
    /// Absent means the workflow's default; WORKFLOW_MODEL overrides per run.
    Model { model: String },
    /// The model that reviews each task's diff at the merge gate, after its
    /// Verify goes green (`fable`, `opus`, ...). `none` says nobody reads and
    /// the run goes ahead unread; absent means the project has not decided,
    /// and a run stops to ask. WORKFLOW_REVIEW_MODEL overrides per run.
    #[command(name = "review-model")]
    ReviewModel { model: String },
    /// How much reasoning a run's workers spend, on whatever model they run
    /// (`max` buys a cheaper model more thinking). Absent means the CLI's
    /// own default; WORKFLOW_EFFORT overrides per run, and set empty it
    /// means no dial for that run.
    Effort { level: Effort },
    /// The same dial for the reader at the merge gate. Absent means the
    /// CLI's own default; WORKFLOW_REVIEW_EFFORT overrides per run, and set
    /// empty it means no dial for that run.
    #[command(name = "review-effort")]
    ReviewEffort { level: Effort },
    /// This project's origin remote, for one registered before the remote
    /// existed. Normalized exactly as registration normalizes `origin`.
    Remote { url: String },
}

/// The reasoning dial (`amx new --effort`). A closed list: a level the
/// worker would refuse at launch must not be stored.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "lower")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// Who a question is for. A worker's question is the orchestrator's to
/// settle and never reaches the hub; a person's is what the hub shows.
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

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    fn long_help(path: &[&str]) -> String {
        let mut cmd = Cli::command();
        cmd.build();
        let mut node = &mut cmd;
        for name in path {
            node = node
                .find_subcommand_mut(name)
                .unwrap_or_else(|| panic!("no such subcommand: {name}"));
        }
        node.render_long_help().to_string()
    }

    /// Clearing the reader is not the same as saying nobody reads: a run with
    /// no `review-model` stops and asks for one, so the help has to send a
    /// project that means it to `none` rather than to `unset`.
    #[test]
    fn unset_help_tells_an_absent_reader_apart_from_none() {
        let help = long_help(&["project", "unset"]);
        assert!(help.contains("ask"), "{help}");
        assert!(help.contains("review-model none"), "{help}");
    }

    /// And the key's own help says the same, since that is where a project
    /// setting a reader for the first time reads what the values mean.
    /// The effort dials name their run override the way the model keys do,
    /// since that is where a project reads how to turn one up for one run.
    #[test]
    fn effort_help_names_the_run_override_for_each_dial() {
        let help = long_help(&["project", "set", "effort"]);
        assert!(help.contains("WORKFLOW_EFFORT"), "{help}");
        let help = long_help(&["project", "set", "review-effort"]);
        assert!(help.contains("WORKFLOW_REVIEW_EFFORT"), "{help}");
    }

    #[test]
    fn review_model_help_names_none_and_what_absent_costs() {
        let help = long_help(&["project", "set", "review-model"]);
        assert!(help.contains("none"), "{help}");
        assert!(help.contains("WORKFLOW_REVIEW_MODEL"), "{help}");
    }
}
