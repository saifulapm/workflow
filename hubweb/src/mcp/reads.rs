//! The read tools: memview's JSON for each, printed as mem prints its text.
//! A page, a plan, a roadmap, a handoff and a shown item are file text, so
//! those match mem byte for byte; the listings carry the same facts in mem's
//! line shapes.

use serde_json::Value;

use super::{Call, ID, Project, SLUG, Text, WIKI_SLUG, checked, flag, limit, optional};
use crate::memview::{self, sections, truncate_bytes};

/// mem's width for one search hit's line.
const LINE_BUDGET: usize = 80;
/// The lines of the wiki's index page the digest carries.
const INDEX_HEAD_LINES: usize = 5;

impl<F: memview::Files> Call<'_, F> {
    /// The project's digest: where it stands, then what it has recorded.
    /// Global rules live outside the store, so the digest says it has none.
    pub(super) fn context(&self, p: &Project, a: &Value) -> Text {
        let full = flag(a, "full")?;
        let mut lines = vec![format!("project: {}", p.name)];
        if let Some(h) = self.view(&["handoff", "--project", &p.name])? {
            lines.push(format!(
                "handoff ({}): {}",
                s(&h["modified"]),
                s(&h["title"])
            ));
            let sentence = first_sentence(s(&h["body"]));
            if !sentence.is_empty() {
                lines.push(format!("  {sentence}"));
            }
        }
        for (noun, file) in [("roadmap", "roadmap.md"), ("plan", "plan.md")] {
            let text = self.read(p, file).unwrap_or_default();
            for line in plan_head(&text) {
                lines.push(format!("{noun}: {line}"));
            }
        }
        let pending = self.view(&["questions", "--pending", "--project", &p.name])?;
        for q in rows(&pending, "questions") {
            lines.push(format!(
                "? #{}  {}{}",
                s(&q["short_id"]),
                who(q),
                s(&q["title"])
            ));
        }
        let pages = self.view(&["wiki", "--project", &p.name])?;
        let n = rows(&pages, "pages").len();
        if n > 0 {
            let plural = if n == 1 { "" } else { "s" };
            lines.push(format!("wiki: {n} page{plural} \u{2014} mem wiki"));
            let index = self.read(p, "wiki/index.md").unwrap_or_default();
            lines.extend(
                index
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(INDEX_HEAD_LINES)
                    .map(|l| format!("  {}", truncate_bytes(l, 100))),
            );
        }
        let mut kinds = vec![("ruling", "5")];
        if full {
            kinds.extend([("fact", "10"), ("log", "10")]);
        }
        for (kind, n) in kinds {
            let recent = self.view(&["log", "--kind", kind, "--limit", n, "--project", &p.name])?;
            for row in rows(&recent, "items") {
                lines.push(format!(
                    "{kind} #{}  {}",
                    s(&row["short_id"]),
                    truncate_bytes(s(&row["title"]), 70)
                ));
            }
        }
        lines.push("rules: none here; global rules are not in the hub's store".into());
        Ok(lines.join("\n") + "\n")
    }

    pub(super) fn search(&self, p: &Project, a: &Value) -> Text {
        let query = super::required(a, "query")?;
        let n = limit(a)?.unwrap_or_else(|| "20".into());
        let found = self.view(&["search", "--project", &p.name, "--limit", &n, "--", query])?;
        let hits = rows(&found, "hits");
        if hits.is_empty() {
            return Err(format!("mem: nothing matched '{query}'"));
        }
        Ok(hits.iter().map(|h| hit_line(h) + "\n").collect())
    }

    /// Each item's file as it sits in the store, a blank line between.
    pub(super) fn show(&self, a: &Value) -> Text {
        let ids = a
            .get("ids")
            .and_then(Value::as_array)
            .filter(|ids| !ids.is_empty())
            .ok_or("ids is required")?;
        let mut out = Vec::new();
        for id in ids {
            let id = checked(id.as_str().ok_or("each id must be a string")?, "id", ID)?;
            let shown = self.view(&["show", "--", id])?;
            let path = rows(&shown, "items")
                .first()
                .and_then(|row| row["path"].as_str().map(str::to_string));
            let text = path.and_then(|path| self.store.files.read(&path));
            out.push(text.ok_or_else(|| format!("mem: no item {id}"))?);
        }
        Ok(out.join("\n"))
    }

    /// The page list, a page, or one `#section` of a page. The store keeps
    /// no file dates, so the list leaves out mem's date column.
    pub(super) fn wiki(&self, p: &Project, a: &Value) -> Text {
        let Some(slug) = optional(a, "slug")? else {
            let listed = self.view(&["wiki", "--project", &p.name])?;
            let pages = rows(&listed, "pages");
            if pages.is_empty() {
                return Err("mem: no pages yet \u{2014} write one with `mem wiki <slug> --stdin --note \"why\"`".into());
            }
            return Ok(pages
                .iter()
                .map(|page| {
                    format!(
                        "{:<24} {:>6}  {}\n",
                        s(&page["slug"]),
                        page["bytes"],
                        s(&page["title"])
                    )
                })
                .collect());
        };
        let (slug, hslug) = split_slug(checked(slug, "slug", WIKI_SLUG)?);
        let page = page_text(self, p, slug)?;
        let Some(hslug) = hslug else {
            return Ok(page);
        };
        let all = sections::split(&page);
        let section = all
            .iter()
            .find(|s| s.hslug == hslug)
            .ok_or_else(|| format!("mem: {}", no_such_section(slug, hslug, &all)))?;
        Ok(page[section.start..section.end].to_string())
    }

    pub(super) fn questions(&self, p: &Project, a: &Value) -> Text {
        let mut argv = vec!["questions", "--project", &p.name];
        if flag(a, "pending")? {
            argv.push("--pending");
        }
        let listed = self.view(&argv)?;
        let questions = rows(&listed, "questions");
        if questions.is_empty() {
            return Err("mem: no questions".into());
        }
        let mut out = String::new();
        for q in questions {
            let mark = if q["answered"] == true {
                "\u{2713}"
            } else {
                "?"
            };
            out += &format!(
                "{mark} #{}  {}{}\n",
                s(&q["short_id"]),
                who(q),
                s(&q["title"])
            );
            if let Some(text) = q["recommend"].as_str() {
                out += &format!("    recommended: {text}\n");
            }
        }
        Ok(out)
    }

    pub(super) fn plan(&self, p: &Project, a: &Value) -> Text {
        let Some(slug) = optional(a, "slug")? else {
            return self.read(p, "plan.md").ok_or_else(|| {
                "mem: no plan recorded for this project \u{2014} write one with `mem plan --stdin`"
                    .into()
            });
        };
        let slug = checked(slug, "slug", SLUG)?;
        let stored = self.view(&["plan", slug, "--project", &p.name])?;
        stored
            .as_ref()
            .and_then(|doc| doc["text"].as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                format!("mem: no stored plan '{slug}' \u{2014} `mem plan --list` lists them")
            })
    }

    pub(super) fn roadmap(&self, p: &Project, a: &Value) -> Text {
        if !flag(a, "status")? {
            return self.read(p, "roadmap.md").ok_or_else(|| {
                "mem: no roadmap recorded for this project \u{2014} write one with `mem roadmap --stdin`"
                    .into()
            });
        }
        let toml = self.read(p, "project.toml").unwrap_or_default();
        let status = toml::from_str::<toml::Table>(&toml)
            .ok()
            .and_then(|t| t.get("roadmap_status")?.as_str().map(str::to_string));
        status.map(|s| s + "\n").ok_or_else(|| {
            "mem: no roadmap status set \u{2014} `mem roadmap --status <draft|approved>` sets one"
                .into()
        })
    }

    /// mem prints nothing for a project with no log; this says so instead.
    pub(super) fn log(&self, p: &Project, a: &Value) -> Text {
        let n = limit(a)?.unwrap_or_else(|| "20".into());
        let listed = self.view(&["log", "--project", &p.name, "--limit", &n])?;
        let items = rows(&listed, "items");
        if items.is_empty() {
            return Err("mem: no log entries".into());
        }
        Ok(items
            .iter()
            .map(|row| {
                format!(
                    "{}  #{}  {}\n",
                    s(&row["modified"]),
                    s(&row["short_id"]),
                    s(&row["title"])
                )
            })
            .collect())
    }

    pub(super) fn handoff(&self, p: &Project) -> Text {
        let latest = self.view(&["handoff", "--project", &p.name])?;
        latest
            .as_ref()
            .and_then(|h| h["body"].as_str())
            .map(str::to_string)
            .ok_or_else(|| "mem: no handoff recorded for this project".into())
    }

    /// One of the project's own files, such as `plan.md`.
    pub(super) fn read(&self, p: &Project, name: &str) -> Option<String> {
        self.store.files.read(&format!("projects/{}/{name}", p.id))
    }
}

/// A page's text, or mem's refusal for a slug it cannot be or a page that
/// is not there.
pub(super) fn page_text<F: memview::Files>(call: &Call<F>, p: &Project, slug: &str) -> Text {
    page_slug(slug)?;
    call.read(p, &format!("wiki/{slug}.md"))
        .ok_or_else(|| format!("mem: no page '{slug}' \u{2014} `mem wiki` lists them"))
}

/// mem's refusal of a slug no page can have.
pub(super) fn page_slug(slug: &str) -> Result<(), String> {
    if memview::is_valid_slug(slug) {
        return Ok(());
    }
    Err(format!(
        "mem: '{slug}' is not a page slug \u{2014} lower case letters, digits and dashes, \
         starting with a letter or a digit, at most 64 characters"
    ))
}

/// `<slug>#<hslug>` as the page and the section; the slug alone is the page.
pub(super) fn split_slug(slug: &str) -> (&str, Option<&str>) {
    let (slug, hslug) = slug.split_once('#').unwrap_or((slug, ""));
    (slug, (!hslug.is_empty()).then_some(hslug))
}

/// The refusal for a section the page lacks names the ones it has.
pub(super) fn no_such_section(slug: &str, hslug: &str, all: &[sections::Section]) -> String {
    let have: Vec<&str> = all.iter().map(|s| s.hslug.as_str()).collect();
    let have = if have.is_empty() {
        "none".to_string()
    } else {
        have.join(", ")
    };
    format!("no section '{hslug}' in {slug} \u{2014} its sections: {have}")
}

/// One search hit as mem prints it: an item on one line, cut at 80 bytes; a
/// page section with its snippet lines under it.
fn hit_line(hit: &Value) -> String {
    let score = hit["score"].as_f64().unwrap_or(0.0);
    if hit["kind"] == "wiki" {
        let mut out = format!("wiki:{}  {score:.2}  {}", s(&hit["short_id"]), hit["bytes"]);
        for line in hit["snippet"].as_str().unwrap_or("").lines() {
            out.push_str("\n  ");
            out.push_str(line);
        }
        return out;
    }
    let prefix = format!(
        "#{}  {score:.2}  {}  {}  ",
        s(&hit["short_id"]),
        s(&hit["kind"]),
        s(&hit["modified"])
    );
    let room = LINE_BUDGET.saturating_sub(prefix.len());
    let line = format!("{prefix}{}", truncate_bytes(s(&hit["title"]), room));
    truncate_bytes(&line, LINE_BUDGET)
}

/// The `[audience] ` an orchestrator's question carries in a listing.
fn who(question: &Value) -> String {
    match question["audience"].as_str() {
        Some(a) => format!("[{a}] "),
        None => String::new(),
    }
}

/// The rows under `key` of a document, none when mem printed nothing.
fn rows<'v>(doc: &'v Option<Value>, key: &str) -> &'v [Value] {
    doc.as_ref()
        .and_then(|d| d[key].as_array())
        .map_or(&[], Vec::as_slice)
}

fn s(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

/// A body's first sentence, or its first line when it has none.
fn first_sentence(body: &str) -> &str {
    let text = body.trim();
    let end = text
        .find(". ")
        .map(|i| i + 1)
        .or_else(|| text.find('\n'))
        .unwrap_or(text.len());
    text[..end].trim()
}

/// A plan's or a roadmap's first heading and first open box.
fn plan_head(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    if let Some(heading) = text.lines().find(|l| l.trim_start().starts_with('#')) {
        out.push(heading.trim());
    }
    out.extend(first_open_task(text));
    out
}

/// The first open box, skipping boxes in a fenced block or indented four
/// spaces, which are examples of the grammar rather than tasks.
fn first_open_task(text: &str) -> Option<&str> {
    let mut fenced = false;
    for line in text.lines() {
        let body = line.trim_start();
        if body.starts_with("```") || body.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.starts_with("    ") {
            continue;
        }
        let line = line.trim();
        if line.starts_with("- [ ]") || line.starts_with("* [ ]") {
            return Some(line);
        }
    }
    None
}
