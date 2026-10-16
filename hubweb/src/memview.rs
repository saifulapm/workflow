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
mod search;
mod sections;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use item::{Row, newest_first, row_json};

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
    let view = View {
        files,
        registry: Registry::load(files),
    };
    let served = match args.words.as_slice() {
        ["projects"] => args.only(&[]).then(|| view.projects()),
        ["questions"] => args
            .only(&["pending", "all-projects", "for", "project"])
            .then(|| view.questions(&args)),
        ["brief"] => args.only(&["project"]).then(|| view.latest(&args, "brief")),
        ["handoff"] => args
            .only(&["project"])
            .then(|| view.latest(&args, "handoff")),
        ["roadmap"] => args
            .only(&["project"])
            .then(|| view.singleton(&args, "roadmap.md", true)),
        ["plan"] if args.flag("list") => args
            .only(&["list", "project"])
            .then(|| view.listing(&args, "plans", "plans")),
        ["plan"] => args
            .only(&["project"])
            .then(|| view.singleton(&args, "plan.md", false)),
        ["plan", slug] => args
            .only(&["project"])
            .then(|| view.slug_file(&args, "plans", slug)),
        ["wiki"] if args.flag("sections") => Some(Reply::Absent),
        ["wiki"] => args
            .only(&["project"])
            .then(|| view.listing(&args, "wiki", "pages")),
        ["wiki", slug] if slug.contains('#') => None,
        ["wiki", slug] if args.flag("sections") => args
            .only(&["sections", "project"])
            .then(|| view.sections(&args, slug)),
        ["wiki", slug] => args
            .only(&["project"])
            .then(|| view.slug_file(&args, "wiki", slug)),
        ["log"] => args
            .only(&["kind", "limit", "project"])
            .then(|| view.log(&args, false)),
        ["search"] if args.flag("kind") => args
            .only(&["kind", "limit", "project"])
            .then(|| view.log(&args, true)),
        ["search", query] => args
            .only(&["limit", "project"])
            .then(|| view.search(&args, query, now)),
        ["evidence", "list"] => args.only(&["project"]).then(|| view.evidence(&args)),
        ["finding", "list"] => args
            .only(&["open", "project"])
            .then(|| view.findings(&args)),
        ["show", id] => args.only(&[]).then(|| view.show(id)),
        _ => None,
    };
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
    remote: Option<String>,
    aliases: Vec<String>,
    created: jiff::Timestamp,
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
                let created = text("created")?.parse::<jiff::Timestamp>().ok()?;
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
                    remote: text("remote"),
                    aliases,
                    created,
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

    /// The project `--project` names: Err(Unknown) without one, since mem
    /// would read the working directory, and Err(Absent) for a name mem does
    /// not know.
    fn project(&self, args: &Args) -> Result<&Project, Reply> {
        let name = args.value("project").ok_or(Reply::Unknown)?;
        self.registry.by_name(name).ok_or(Reply::Absent)
    }

    /// `mem projects`: each project's record and what its page shows of it.
    /// The keys mem takes from this machine's paths.toml and working
    /// directory are empty.
    fn projects(&self) -> Reply {
        let rows: Vec<Value> = self
            .registry
            .projects
            .iter()
            .map(|p| {
                let mut row = json!({
                    "id": p.id,
                    "name": p.name,
                    "remote": p.remote,
                    "aliases": [],
                    "created": p.created.to_string(),
                    "current": false,
                    "checkouts": [],
                    "worktree": null,
                });
                for (key, value) in self.summary(p) {
                    row[key] = value;
                }
                row
            })
            .collect();
        Reply::Json(json!({ "projects": rows }))
    }

    /// mem's `project_summary`, from project.toml, the roadmap, the plan,
    /// the wiki and the items.
    fn summary(&self, p: &Project) -> Vec<(&'static str, Value)> {
        let read = |name: &str| {
            self.files
                .read(&format!("projects/{}/{name}", p.id))
                .unwrap_or_default()
        };
        let roadmap_text = read("roadmap.md");
        let roadmap = checkboxes(&roadmap_text, false);
        let plan_text = read("plan.md");
        let plan = checkboxes(&plan_text, true);
        let plan_slug = plan_text
            .lines()
            .next()
            .and_then(|first| header_slug(first, "plan"));
        let mut rows = self.rows(p, &[]);
        let count = rows.len();
        rows.retain(|r| r.active);
        newest_first(&mut rows);
        let stamp = |row: Option<&Row>| {
            row.and_then(|r| jiff::Timestamp::from_second(r.meta().modified).ok())
                .map(|ts| ts.to_string())
        };
        let last_activity = stamp(rows.first());
        let handoff_at = stamp(rows.iter().find(|r| r.meta().kind == "handoff"));
        let pages = self.pages(p, "wiki");
        let has_page = |wanted: fn(&str) -> bool| pages.iter().any(|page| wanted(&page.slug));
        let runner = match p
            .declared("runner")
            .as_deref()
            .and_then(|r| r.split_once(' '))
        {
            Some((machine, run)) => json!({
                "machine": machine,
                "id": run,
                "since": p.declared("runner_since"),
                "milestone": p.declared("runner_milestone"),
            }),
            None => Value::Null,
        };
        vec![
            ("items", json!(count)),
            ("roadmap_status", json!(p.declared("roadmap_status"))),
            ("paused", json!(p.declared("paused"))),
            ("runner", runner),
            (
                "milestone",
                json!(roadmap.iter().find(|(_, done)| !done).map(|(slug, _)| slug)),
            ),
            (
                "milestones_done",
                json!(roadmap.iter().filter(|(_, done)| *done).count()),
            ),
            ("milestones_total", json!(roadmap.len())),
            ("plan_slug", json!(plan_slug)),
            (
                "plan_ticked",
                json!(plan.iter().filter(|(_, done)| *done).count()),
            ),
            ("plan_total", json!(plan.len())),
            ("last_activity", json!(last_activity)),
            ("handoff_at", json!(handoff_at)),
            (
                "has_research",
                json!(has_page(|s| s.starts_with("research"))),
            ),
            (
                "has_research_summary",
                json!(has_page(|s| s == "research-summary")),
            ),
            ("has_spec", json!(has_page(|s| s == "spec"))),
        ]
    }

    /// The `<slug>.md` files of a project's `wiki` or `plans` directory, in
    /// slug order; anything else there is skipped, as mem skips it.
    fn pages(&self, p: &Project, dir: &str) -> Vec<Page> {
        let prefix = format!("projects/{}/{dir}/", p.id);
        self.files
            .list(&prefix)
            .into_iter()
            .filter_map(|(path, text)| {
                let slug = path.strip_prefix(&prefix)?.strip_suffix(".md")?;
                is_valid_slug(slug).then(|| Page {
                    slug: slug.to_string(),
                    path: path.clone(),
                    text,
                })
            })
            .collect()
    }

    /// `mem brief` and `mem handoff`: the newest active item of the kind,
    /// body and all.
    fn latest(&self, args: &Args, kind: &str) -> Reply {
        let rows = match self.records(args, kind) {
            Ok(rows) => rows,
            Err(reply) => return reply,
        };
        let Some(row) = rows.first() else {
            return Reply::Absent;
        };
        let mut v = row_json(row);
        v["body"] = json!(row.item.body);
        Reply::Json(v)
    }

    /// `mem roadmap` and `mem plan`: the file whole, and the roadmap's
    /// status when project.toml declares one.
    fn singleton(&self, args: &Args, name: &str, with_status: bool) -> Reply {
        let p = match self.project(args) {
            Ok(p) => p,
            Err(reply) => return reply,
        };
        let path = format!("projects/{}/{name}", p.id);
        let Some(text) = self.files.read(&path) else {
            return Reply::Absent;
        };
        let mut out = json!({ "text": text, "path": path });
        if let Some(status) = p.declared("roadmap_status").filter(|_| with_status) {
            out["status"] = json!(status);
        }
        Reply::Json(out)
    }

    /// `mem wiki` and `mem plan --list`: one row per page. The date is null:
    /// mem takes it from the file's mtime, which the store does not keep.
    fn listing(&self, args: &Args, dir: &str, key: &str) -> Reply {
        let p = match self.project(args) {
            Ok(p) => p,
            Err(reply) => return reply,
        };
        let rows: Vec<Value> = self
            .pages(p, dir)
            .iter()
            .map(|page| {
                json!({
                    "slug": page.slug,
                    "title": page_title(&page.text),
                    "bytes": page.text.len(),
                    "modified": null,
                    "path": page.path,
                })
            })
            .collect();
        Reply::Json(json!({ key: rows }))
    }

    /// `mem wiki -- <slug>` and `mem plan -- <slug>`: one page whole.
    fn slug_file(&self, args: &Args, dir: &str, slug: &str) -> Reply {
        let path = match self.page_path(args, dir, slug) {
            Ok(path) => path,
            Err(reply) => return reply,
        };
        match self.files.read(&path) {
            Some(text) => Reply::Json(json!({
                "slug": slug,
                "text": text,
                "bytes": text.len(),
                "path": path,
            })),
            None => Reply::Absent,
        }
    }

    /// The project and store path of one page; a slug mem refuses is its
    /// usage error, with nothing on stdout.
    fn page_path(&self, args: &Args, dir: &str, slug: &str) -> Result<String, Reply> {
        if !is_valid_slug(slug) {
            return Err(Reply::Absent);
        }
        let p = self.project(args)?;
        Ok(format!("projects/{}/{dir}/{slug}.md", p.id))
    }

    /// `mem wiki --sections -- <slug>`: the page's `## ` sections.
    fn sections(&self, args: &Args, slug: &str) -> Reply {
        let path = match self.page_path(args, "wiki", slug) {
            Ok(path) => path,
            Err(reply) => return reply,
        };
        let Some(text) = self.files.read(&path) else {
            return Reply::Absent;
        };
        let rows: Vec<Value> = sections::split(&text)
            .iter()
            .map(|s| json!({ "hslug": s.hslug, "heading": s.heading, "bytes": s.end - s.start }))
            .collect();
        Reply::Json(json!({ "sections": rows }))
    }

    /// `mem log` and `mem search --kind <kind>` with no query: the newest
    /// active items of one kind, `log` by default. The search form carries
    /// each body.
    fn log(&self, args: &Args, bodies: bool) -> Reply {
        let limit = match args.value("limit").map(str::parse::<usize>) {
            None => 20,
            Some(Ok(n)) => n.max(1),
            Some(Err(_)) => return Reply::Absent,
        };
        let mut rows = match self.records(args, args.value("kind").unwrap_or("log")) {
            Ok(rows) => rows,
            Err(reply) => return reply,
        };
        rows.truncate(limit);
        let items: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut v = row_json(row);
                if bodies {
                    v["body"] = json!(row.item.body);
                }
                v
            })
            .collect();
        Reply::Json(json!({ "items": items }))
    }

    /// `mem search -- <query>`: the project's items and page sections that
    /// hold every word, ranked as `search::search` describes. It reads every
    /// item and page in the store, since BM25 weighs a word by the whole
    /// table.
    fn search(&self, args: &Args, query: &str, now: jiff::Timestamp) -> Reply {
        let limit = match args.value("limit").map(str::parse::<usize>) {
            None => 20,
            Some(Ok(n)) => n,
            Some(Err(_)) => return Reply::Absent,
        };
        let p = match self.project(args) {
            Ok(p) => p,
            Err(reply) => return reply,
        };
        let pages: Vec<(&Project, Vec<Page>)> = self
            .registry
            .projects
            .iter()
            .map(|q| (q, self.pages(q, "wiki")))
            .collect();
        let corpus = search::Corpus {
            items: self
                .registry
                .projects
                .iter()
                .flat_map(|q| self.rows(q, &[]).into_iter().map(|r| (q.id.as_str(), r)))
                .collect(),
            sections: pages
                .iter()
                .flat_map(|(q, pages)| {
                    pages.iter().flat_map(|page| {
                        sections::split(&page.text)
                            .into_iter()
                            .map(|section| search::SectionHit {
                                project_id: &q.id,
                                project: &q.name,
                                page,
                                section,
                            })
                    })
                })
                .collect(),
            project_id: &p.id,
        };
        let hits = search::search(&corpus, query, limit, now.as_second());
        Reply::Json(json!({ "hits": hits }))
    }

    /// The active items of one kind in the project `--project` names,
    /// newest first.
    fn records(&self, args: &Args, kind: &str) -> Result<Vec<Row>, Reply> {
        let p = self.project(args)?;
        let mut rows = self.rows(p, &[kind]);
        rows.retain(|r| r.active);
        newest_first(&mut rows);
        Ok(rows)
    }

    /// `mem evidence list`.
    fn evidence(&self, args: &Args) -> Reply {
        let rows = match self.records(args, "evidence") {
            Ok(rows) => rows,
            Err(reply) => return reply,
        };
        let items: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut v = row_json(row);
                v["file"] = json!(row.meta().file);
                v["note"] = json!(row.item.body.trim());
                v
            })
            .collect();
        Reply::Json(json!({ "items": items }))
    }

    /// `mem finding list`, `--open` keeping only the open ones.
    fn findings(&self, args: &Args) -> Reply {
        let mut rows = match self.records(args, "finding") {
            Ok(rows) => rows,
            Err(reply) => return reply,
        };
        if args.flag("open") {
            rows.retain(|r| r.meta().status.as_deref() == Some("open"));
        }
        let items: Vec<Value> = rows
            .iter()
            .map(|row| {
                let m = row.meta();
                let mut v = row_json(row);
                v["milestone"] = json!(m.milestone);
                v["step"] = json!(m.step);
                v["status"] = json!(m.status);
                v["fixed_by"] = json!(m.fixed_by);
                v["file"] = json!(m.file);
                v["body"] = json!(row.item.body);
                v
            })
            .collect();
        Reply::Json(json!({ "items": items }))
    }

    /// `mem show -- <id>`: a full ULID or its last 8 characters, matched
    /// against every project's items. No match, or a short id two items
    /// share, is mem's exit 1 or 3 with nothing on stdout.
    fn show(&self, raw: &str) -> Reply {
        let wanted = raw.trim().trim_start_matches('#').to_ascii_uppercase();
        let full = item::is_ulid(&wanted);
        if !full && !(wanted.len() == 8 && wanted.chars().all(item::is_ulid_char)) {
            return Reply::Absent;
        }
        let mut found: Vec<Row> = Vec::new();
        for p in &self.registry.projects {
            found.extend(self.rows(p, &[]).into_iter().filter(|r| {
                if full {
                    r.meta().id == wanted
                } else {
                    r.short_id() == wanted
                }
            }));
        }
        let [row] = found.as_slice() else {
            return Reply::Absent;
        };
        let mut v = row_json(row);
        v["body"] = json!(row.item.body);
        Reply::Json(json!({ "items": [v] }))
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

/// A page or a stored plan.
struct Page {
    slug: String,
    path: String,
    text: String,
}

/// `[a-z0-9][a-z0-9-]{0,63}`, mem's rule for a page or plan slug.
fn is_valid_slug(slug: &str) -> bool {
    let mut chars = slug.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    slug.len() <= 64 && plain(first) && chars.all(|c| plain(c) || c == '-')
}

/// What a listing calls a page: its first heading, or its first non-empty
/// line when it has none, cut at 100 bytes.
fn page_title(text: &str) -> String {
    let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return String::new();
    };
    let stripped = line.trim_start_matches('#');
    let title = if stripped.len() < line.len() {
        stripped.trim_start()
    } else {
        line
    };
    truncate_bytes(title, 100)
}

/// mem's `truncate_bytes`: newlines flattened, then cut on a character
/// boundary with an ellipsis when anything is cut.
fn truncate_bytes(text: &str, budget: usize) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    if flat.len() <= budget {
        return flat;
    }
    let ellipsis = '…'.len_utf8();
    if budget <= ellipsis {
        return String::new();
    }
    let mut end = budget - ellipsis;
    while end > 0 && !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &flat[..end])
}

/// The `- [ ] <id>` and `- [x] <id>` lines of a plan or a roadmap, as the
/// id and whether it is ticked. A plan's tasks may sit indented; a
/// roadmap's milestones start at the margin.
fn checkboxes(text: &str, indented: bool) -> Vec<(&str, bool)> {
    text.lines()
        .filter_map(|l| {
            let l = if indented { l.trim_start() } else { l };
            let (rest, done) = if let Some(rest) = l.strip_prefix("- [ ] ") {
                (rest, false)
            } else {
                (l.strip_prefix("- [x] ").or(l.strip_prefix("- [X] "))?, true)
            };
            Some((rest.split_whitespace().next()?, done))
        })
        .collect()
}

/// The slug of a `# <noun>: <slug>` header line.
fn header_slug<'a>(line: &'a str, noun: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let slug = rest.strip_prefix(noun)?.strip_prefix(':')?.trim();
    (!slug.is_empty()).then_some(slug)
}

#[cfg(test)]
mod tests;
