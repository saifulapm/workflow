//! The write tools: each turns into the store files mem would have written,
//! handed back for the Worker to write. A new item is a file of its own; a
//! tick, a page and a closed finding rewrite the file they change.

use serde_json::Value;
use toml::Value as Toml;

use super::reads::{no_such_section, page_slug, page_text, split_slug};
use super::tools::{AUDIENCES, DECIDERS};
use super::{
    Call, ID, MACHINE, Project, SLUG, Text, WIKI_SLUG, checked, one_of, optional, project_arg,
    required,
};
use crate::item;
use crate::memview::{self, sections};

/// What `REVIEW_QUESTION` in hubpages' page_control.rs asks: a roadmap's
/// review reads "Review the <name> roadmap".
const REVIEW_QUESTION: (&str, &str) = ("Review the ", " roadmap");

impl<F: memview::Files> Call<'_, F> {
    /// Writes one new item and returns its id.
    fn new_item(&mut self, p: &Project, kind: &str, text: &str, keys: Vec<(&str, Toml)>) -> String {
        let ms = u64::try_from(self.store.now.as_millisecond()).unwrap_or(0);
        let id = item::ulid(ms, (self.store.random)());
        let new = item::New {
            kind,
            project: &p.name,
            text,
            keys,
        };
        let body = item::file(&new, &id, &self.store.now.to_string(), MACHINE);
        self.writes
            .push((format!("projects/{}/items/{id}.md", p.id), Some(body)));
        id
    }

    /// A new item, reported as `mem save` reports one.
    pub(super) fn save(
        &mut self,
        p: &Project,
        kind: &str,
        text: &str,
        keys: Vec<(&str, Toml)>,
    ) -> Text {
        let id = self.new_item(p, kind, text, keys);
        Ok(format!("#{}  {kind}\n", short(&id)))
    }

    pub(super) fn ask(&mut self, p: &Project, a: &Value) -> Text {
        let question = required(a, "question")?;
        if question.trim().is_empty() {
            return Err("mem: the question is empty \u{2014} say what you are asking".into());
        }
        let mut keys = Vec::new();
        let options: Vec<&str> = optional(a, "options")?
            .filter(|o| !o.is_empty())
            .map(|o| o.split(',').collect())
            .unwrap_or_default();
        if !options.is_empty() {
            let stored = options.iter().map(|o| Toml::String((*o).into())).collect();
            keys.push(("options", Toml::Array(stored)));
        }
        if let Some(recommend) = optional(a, "recommend")? {
            // The phone marks the recommended button by its text, as mem ask
            // checks on a machine.
            if !options.is_empty() && !options.contains(&recommend) {
                return Err(format!(
                    "mem: --recommend \"{recommend}\" is not one of the options: {} \u{2014} pass \
                     it exactly as one of them",
                    options.join(" · ")
                ));
            }
            keys.push(("recommend", Toml::String(recommend.into())));
        }
        // A person's question stores no audience; only the others are named.
        if let Some(audience) = optional(a, "for")? {
            let audience = one_of(audience, "for", &AUDIENCES)?;
            if audience != "human" {
                keys.push(("audience", Toml::String(audience.into())));
            }
        }
        let id = self.new_item(p, "question", question, keys);
        Ok(format!("#{}\n", short(&id)))
    }

    /// Signed `connector`, whatever the caller says: an answer with no author
    /// reads as Saiful's own tap, and one the model wrote is not.
    pub(super) fn answer(&mut self, p: &Project, a: &Value) -> Text {
        let id = checked(required(a, "id")?, "id", ID)?;
        let question = self.not_a_review(p, project_arg(a)?, id)?;
        let body = match (optional(a, "text")?, optional(a, "option")?) {
            (Some(text), _) => text,
            (None, Some(option)) => option,
            (None, None) => return Err("mem: an answer needs text or --option".into()),
        };
        let keys = vec![
            ("answers", Toml::String(question)),
            ("by", Toml::String(MACHINE.into())),
        ];
        self.save(p, "answer", body, keys)
    }

    /// The full id of this project's question `id`. Answering a roadmap's
    /// review approves it, which only Saiful does, on the hub. The question
    /// has to be one of this project's, so naming another project cannot step
    /// around the check, and a question the listing lacks is refused rather
    /// than answered blind.
    fn not_a_review(&self, p: &Project, project: &str, id: &str) -> Result<String, String> {
        let listed = self.view(&["questions", "--project", &p.name])?;
        let question = listed
            .as_ref()
            .and_then(|doc| doc["questions"].as_array())
            .into_iter()
            .flatten()
            .find(|row| row["id"] == id || row["short_id"] == id)
            .ok_or_else(|| format!("{id} is not a question of {project}"))?;
        let body = question["body"].as_str().unwrap_or("").trim();
        let offers_approval = question["options"]
            .as_array()
            .is_some_and(|options| options.iter().any(|o| o == "approve"));
        let is_review =
            body.starts_with(REVIEW_QUESTION.0) && body.trim_end().ends_with(REVIEW_QUESTION.1);
        if is_review || (offers_approval && body.to_lowercase().contains("roadmap")) {
            return Err("a roadmap review is Saiful's to answer, on the hub".into());
        }
        Ok(question["id"].as_str().unwrap_or(id).to_string())
    }

    pub(super) fn decide(&mut self, p: &Project, a: &Value) -> Text {
        let text = required(a, "text")?;
        let by = one_of(required(a, "by")?, "by", &DECIDERS)?;
        self.save(p, "ruling", text, vec![("by", Toml::String(by.into()))])
    }

    /// Ticks the line of `plan.md` or `roadmap.md` whose id is `noun`'s
    /// argument; a line already ticked is left as it is.
    pub(super) fn tick(&mut self, p: &Project, a: &Value, noun: &str, document: &str) -> Text {
        let wanted = checked(required(a, noun)?, noun, SLUG)?;
        let path = format!("projects/{}/{document}.md", p.id);
        let text = self
            .store
            .files
            .read(&path)
            .ok_or_else(|| format!("mem: no {document} recorded for this project"))?;
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        for n in 0..lines.len() {
            match tick_line(&lines[n], wanted) {
                Tick::Flipped(line) => {
                    lines[n] = line;
                    self.writes.push((path, Some(lines.join("\n"))));
                    return Ok(format!("- [x] {wanted}\n"));
                }
                Tick::Already => return Ok(format!("- [x] {wanted} (already)\n")),
                Tick::NoMatch => {}
            }
        }
        Err(format!("mem: no {noun} '{wanted}' in the {document}"))
    }

    /// The page, or one section spliced into it, then a log item typed
    /// `wiki` that carries the note: the log is the page's history.
    pub(super) fn write_wiki(&mut self, p: &Project, a: &Value) -> Text {
        let page = required(a, "page")?;
        let (slug, hslug) = split_slug(checked(required(a, "slug")?, "slug", WIKI_SLUG)?);
        let note = required(a, "note")?.trim();
        if note.is_empty() {
            return Err("mem: a page write needs --note \"<what changed and why>\" \u{2014} the note is the page's history".into());
        }
        let mut text = page.to_string();
        if let Some(hslug) = hslug {
            let current = page_text(self, p, slug)?;
            let all = sections::split(&current);
            let section = all
                .iter()
                .find(|s| s.hslug == hslug)
                .ok_or_else(|| format!("mem: {}", no_such_section(slug, hslug, &all)))?;
            // Only the preamble has no heading of its own: text without one
            // would fold this section into the one above it.
            if section.hslug != sections::TOP && !text.starts_with("## ") {
                return Err(format!(
                    "mem: the text for {slug}#{hslug} must open on its `## ` heading line"
                ));
            }
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text = format!(
                "{}{text}{}",
                &current[..section.start],
                &current[section.end..]
            );
        } else {
            page_slug(slug)?;
        }
        if page.trim().is_empty() {
            return Err("mem: a page needs text \u{2014} there is no delete verb, so a page that is done becomes a one-line stub pointing at what replaced it".into());
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let label = match hslug {
            Some(hslug) => format!("{slug}#{hslug}"),
            None => slug.to_string(),
        };
        self.writes
            .push((format!("projects/{}/wiki/{slug}.md", p.id), Some(text)));
        let log = format!("wiki {label}: {note}");
        let id = self.new_item(p, "log", &log, vec![("type", Toml::String("wiki".into()))]);
        Ok(format!("wiki {label}  #{}\n", short(&id)))
    }

    /// The text goes to the files store under the task's evidence directory,
    /// named for this moment, and the item names it.
    pub(super) fn add_evidence(&mut self, p: &Project, a: &Value) -> Text {
        let task = checked(required(a, "task")?, "task", SLUG)?;
        let note = required(a, "note")?;
        let content = required(a, "content")?;
        let file = format!(
            "evidence/{task}/connector-{}.txt",
            self.store.now.as_nanosecond()
        );
        self.files.push((
            format!("projects/{}/{file}", p.id),
            content.as_bytes().to_vec(),
        ));
        let keys = vec![
            ("task", Toml::String(task.into())),
            ("file", Toml::String(file)),
        ];
        self.save(p, "evidence", note, keys)
    }

    pub(super) fn add_finding(&mut self, p: &Project, a: &Value) -> Text {
        let milestone = checked(required(a, "milestone")?, "milestone", SLUG)?;
        let step = required(a, "step")?;
        let text = required(a, "text")?;
        let keys = vec![
            ("milestone", Toml::String(milestone.into())),
            ("step", Toml::String(step.into())),
            ("status", Toml::String("open".into())),
        ];
        self.save(p, "finding", text, keys)
    }

    /// The finding's own file, rewritten in place as fixed.
    pub(super) fn close_finding(&mut self, a: &Value) -> Text {
        let id = checked(required(a, "id")?, "id", ID)?;
        let by = required(a, "by")?;
        let shown = self.view(&["show", "--", id])?;
        let row = shown
            .as_ref()
            .and_then(|doc| doc["items"].as_array()?.first().cloned())
            .ok_or_else(|| format!("mem: no finding {id}"))?;
        let short_id = row["short_id"].as_str().unwrap_or(id);
        if row["kind"] != "finding" {
            return Err(format!(
                "mem: #{short_id} is a {}, not a finding",
                row["kind"].as_str().unwrap_or("")
            ));
        }
        let path = row["path"].as_str().unwrap_or("").to_string();
        let now = self.store.now.to_string();
        let closed = self
            .store
            .files
            .read(&path)
            .and_then(|text| {
                item::rewrite(
                    &text,
                    &[("status", "fixed"), ("fixed_by", by), ("modified", &now)],
                )
            })
            .ok_or_else(|| format!("mem: no finding {id}"))?;
        self.writes.push((path, Some(closed)));
        Ok(format!("#{short_id}  finding\n"))
    }
}

/// The last 8 characters of a ULID, the id mem prints.
fn short(id: &str) -> &str {
    &id[id.len().saturating_sub(8)..]
}

/// What one plan line has to say about a task id.
enum Tick {
    /// The line carries the id and was open; here it is, ticked.
    Flipped(String),
    Already,
    NoMatch,
}

/// mem's tick of one `- [ ] <id> <title>` line: only the box changes, and
/// only an exact id counts, so `t1` never ticks `t10`.
fn tick_line(line: &str, id: &str) -> Tick {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let Some(rest) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) else {
        return Tick::NoMatch;
    };
    let bullet = &line[indent_len..indent_len + 2];
    let (ticked, tail) = if let Some(tail) = rest.strip_prefix("[ ] ") {
        (false, tail)
    } else if let Some(tail) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        (true, tail)
    } else {
        return Tick::NoMatch;
    };
    if tail.split_whitespace().next() != Some(id) {
        return Tick::NoMatch;
    }
    if ticked {
        return Tick::Already;
    }
    Tick::Flipped(format!("{indent}{bullet}[x] {tail}"))
}
