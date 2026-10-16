//! What `mem <argv> --json` prints for the reads the hub's pages make, built
//! from the store's files alone, with no mem process. The store keeps the
//! latest body of every shared path; this module reads them through `Files`
//! and answers with the same JSON document mem would.
//!
//! Three things mem prints come from the machine rather than the store, so
//! the answer differs there: `path` is store-relative where mem's is
//! absolute; a project's `checkouts`, `current` and `worktree` are empty;
//! and a page's or a stored plan's `modified`, which mem takes from the
//! file's mtime, is null. Global items (`global/items`) are not in the store,
//! so no answer holds one.

mod item;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use item::{Row, date, newest_first, row_json};

/// The store's files, by store-relative path.
pub trait Files {
    /// The body of one store path, e.g. `projects/<id>/roadmap.md`.
    fn read(&self, path: &str) -> Option<String>;
    /// Every (path, body) whose path starts with `prefix`, in path order.
    fn list(&self, prefix: &str) -> Vec<(String, String)>;
    /// The ids of every project in the store (each `<id>` with a
    /// `projects/<id>/project.toml`).
    fn projects(&self) -> Vec<String>;
    /// The items of one project whose `kind` is one of `kinds` (all items
    /// when empty), as (path, body).
    fn items(&self, project: &str, kinds: &[&str]) -> Vec<(String, String)> {
        let mut all = self.list(&format!("projects/{project}/items/"));
        if !kinds.is_empty() {
            all.retain(|(_, body)| item_kind(body).is_some_and(|k| kinds.contains(&k.as_str())));
        }
        all
    }
}

/// What `mem <argv>` prints for one read.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// A document. mem prints one even on exit 1 with an empty list.
    Json(Value),
    /// mem prints nothing and exits non-zero: an unknown id, no handoff.
    Absent,
    /// An argv memview does not serve.
    Unknown,
}

/// The `kind` of an item file's frontmatter.
pub fn item_kind(body: &str) -> Option<String> {
    item::table(body)?.get("kind")?.as_str().map(str::to_string)
}

/// What `mem <argv>` prints for this read.
pub fn read(files: &impl Files, argv: &[&str], now: jiff::Timestamp) -> Reply {
    let Some(args) = Args::parse(argv) else {
        return Reply::Unknown;
    };
    if !args.flag("json") {
        return Reply::Unknown;
    }
    let words: Vec<&str> = args.words.clone();
    let view = View {
        files,
        registry: Registry::load(files),
    };
    let served = match words.as_slice() {
        ["projects"] => args.only(&[]).then(|| view.projects()),
        ["questions"] => args
            .only(&["pending", "all-projects", "for", "project"])
            .then(|| view.questions(&args)),
        _ => None,
    };
    let _ = now;
    served.unwrap_or(Reply::Unknown)
}

/// An argv as mem's parser reads the parts the hub uses: `--flag`,
/// `--flag value` or `--flag=value`, words, and everything after `--` a word.
struct Args<'a> {
    words: Vec<&'a str>,
    flags: BTreeMap<&'a str, Option<&'a str>>,
}

/// The flags that take a value.
const VALUED: [&str; 4] = ["project", "limit", "kind", "for"];

impl<'a> Args<'a> {
    fn parse(argv: &[&'a str]) -> Option<Args<'a>> {
        let mut words = Vec::new();
        let mut flags = BTreeMap::new();
        let mut rest = argv.iter();
        while let Some(&arg) = rest.next() {
            if arg == "--" {
                words.extend(rest.by_ref().copied());
                break;
            }
            let Some(flag) = arg.strip_prefix("--") else {
                words.push(arg);
                continue;
            };
            let (name, value) = match flag.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None if VALUED.contains(&flag) => (flag, Some(*rest.next()?)),
                None => (flag, None),
            };
            if flags.insert(name, value).is_some() {
                return None;
            }
        }
        Some(Args { words, flags })
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    fn value(&self, name: &str) -> Option<&'a str> {
        self.flags.get(name).copied().flatten()
    }

    /// Whether every flag but `--json` is one of `allowed`.
    fn only(&self, allowed: &[&str]) -> bool {
        self.flags
            .keys()
            .all(|name| *name == "json" || allowed.contains(name))
    }
}

/// One `projects/<id>/project.toml`, the keys memview reads.
struct Project {
    id: String,
    name: String,
    aliases: Vec<String>,
    toml: toml::Table,
}

impl Project {
    /// A string key of project.toml, as mem's `declared` reads it.
    fn declared(&self, key: &str) -> Option<String> {
        self.toml.get(key)?.as_str().map(str::to_string)
    }
}

/// Every project, sorted by name as mem's registry is.
struct Registry {
    projects: Vec<Project>,
}

impl Registry {
    fn load(files: &impl Files) -> Registry {
        let mut ids = files.projects();
        ids.sort();
        let mut projects: Vec<Project> = ids
            .iter()
            .filter_map(|id| {
                let toml: toml::Table =
                    toml::from_str(&files.read(&format!("projects/{id}/project.toml"))?).ok()?;
                let text = |key: &str| toml.get(key)?.as_str().map(str::to_string);
                // id, name and created are the keys mem cannot load one without.
                text("created")?.parse::<jiff::Timestamp>().ok()?;
                let aliases = match toml.get("aliases") {
                    None => Vec::new(),
                    Some(value) => value
                        .as_array()?
                        .iter()
                        .map(|a| a.as_str().map(str::to_string))
                        .collect::<Option<Vec<_>>>()?,
                };
                Some(Project {
                    id: text("id")?,
                    name: text("name")?,
                    aliases,
                    toml,
                })
            })
            .collect();
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        Registry { projects }
    }

    /// mem's `by_name`: a name first, then an alias no other project shares.
    fn by_name(&self, name: &str) -> Option<&Project> {
        if let Some(p) = self.projects.iter().find(|p| p.name == name) {
            return Some(p);
        }
        let mut aliased = self
            .projects
            .iter()
            .filter(|p| p.aliases.iter().any(|a| a == name));
        match (aliased.next(), aliased.next()) {
            (Some(p), None) => Some(p),
            _ => None,
        }
    }

    fn by_id(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }
}

struct View<'f, F: Files> {
    files: &'f F,
    registry: Registry,
}

impl<F: Files> View<'_, F> {
    /// The rows of one project's items of `kinds`, every item when empty.
    fn rows(&self, project: &Project, kinds: &[&str]) -> Vec<Row> {
        item::rows(
            self.files.items(&project.id, kinds),
            &project.id,
            Some(&project.name),
        )
    }

    fn projects(&self) -> Reply {
        let _ = &self.registry;
        Reply::Unknown
    }

    /// `mem questions`: with `--all-projects` every project's pending
    /// questions, by project name and then oldest first; with `--pending` one
    /// project's; otherwise the 50 newest of one project's, answered or not.
    fn questions(&self, args: &Args) -> Reply {
        let audience = args.value("for");
        if !matches!(audience, None | Some("human" | "orchestrator" | "saiful")) {
            return Reply::Unknown;
        }
        let projects: Vec<&Project> = if args.flag("all-projects") {
            self.registry.projects.iter().collect()
        } else {
            let Some(name) = args.value("project") else {
                return Reply::Unknown;
            };
            match self.registry.by_name(name) {
                Some(p) => vec![p],
                None => return Reply::Absent,
            }
        };
        let pending = args.flag("all-projects") || args.flag("pending");
        let mut out = Vec::new();
        for project in projects {
            let rows = self.rows(project, &["question", "answer"]);
            let mut questions: Vec<&Row> = rows
                .iter()
                .filter(|r| r.active && r.meta().kind == "question")
                .collect();
            if pending {
                questions.retain(|q| {
                    !rows
                        .iter()
                        .any(|a| a.meta().answers.as_ref() == Some(&q.meta().id))
                });
                questions.sort_by(|a, b| {
                    (a.meta().created, &a.meta().id).cmp(&(b.meta().created, &b.meta().id))
                });
            } else {
                questions.sort_by(|a, b| {
                    (b.meta().modified, &b.meta().id).cmp(&(a.meta().modified, &a.meta().id))
                });
                questions.truncate(50);
            }
            out.extend(
                questions
                    .into_iter()
                    .filter(|q| is_for(audience, q))
                    .map(|q| question_json(q, &rows)),
            );
        }
        Reply::Json(json!({ "questions": out }))
    }
}

/// Whether a question is `--for` this audience: a person's is any not the
/// orchestrator's, as mem's `is_a_persons` has it.
fn is_for(audience: Option<&str>, question: &Row) -> bool {
    let stored = question.meta().audience.as_deref();
    match audience {
        None => true,
        Some("human") => stored != Some("orchestrator"),
        Some(other) => stored == Some(other),
    }
}

/// A question's row with its body, options and answer. The answer is the
/// newest active item that answers it, else the newest replaced one.
fn question_json(question: &Row, rows: &[Row]) -> Value {
    let id = &question.meta().id;
    let answer = rows
        .iter()
        .filter(|r| r.meta().answers.as_ref() == Some(id))
        .max_by(|a, b| (a.active, &a.meta().id).cmp(&(b.active, &b.meta().id)));
    let m = question.meta();
    let mut v = row_json(question);
    v["answered"] = json!(answer.is_some());
    v["body"] = json!(question.item.body.trim());
    v["answer"] = json!(answer.map(|a| a.item.body.trim().to_string()));
    v["answer_id"] = json!(answer.map(|a| a.meta().id.clone()));
    v["answer_by"] = json!(answer.and_then(|a| a.meta().by.clone()));
    v["options"] = json!(m.options.clone().unwrap_or_default());
    v["session"] = json!(m.session);
    v["recommend"] = json!(m.recommend);
    v
}

#[cfg(test)]
mod tests;
