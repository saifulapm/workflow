//! What the pages are made of (spec §3, §4b).
//!
//! **Time comes from the id, not from `created`.** Every list verb reports
//! `"created":"2026-08-18"` — date only — so a question asked thirty seconds
//! ago and one asked twenty hours ago are the same string (review M-1). The
//! `id` is a ULID whose first ten characters are milliseconds since the epoch,
//! so that is where the age and the ordering come from.

use serde::Serialize;
use serde_json::Value;

use crate::memcli::{MemCli, Outcome};

/// Crockford base32, the ULID alphabet.
const BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// mem's own ceiling on a page slug (`mem/src/store.rs`).
pub const SLUG_MAX: usize = 64;

/// How many log lines a building project's front page shows.
pub const LAST_MOVES: usize = 5;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Activity {
    pub id: String,
    pub short_id: String,
    pub kind: String,
    pub title: String,
    pub project: Option<String>,
    pub machine: String,
    pub at: Option<String>,
    pub age: String,
}

/// One page of a project's wiki, as `mem wiki --json` lists it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WikiPage {
    pub slug: String,
    /// The page's first heading — or the slug, for a page that has none.
    pub title: String,
    pub bytes: u64,
    /// mem's date, `2026-08-25`. `None` where mem could not stat the file.
    pub modified: Option<String>,
}

/// One project that has at least one page. A project with none is not a
/// heading with nothing under it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WikiProject {
    pub name: String,
    pub pages: Vec<WikiPage>,
}

/// One section of a page: its rows, and the reason the section is degraded
/// when it is.
#[derive(Debug, Clone)]
pub struct Section<T> {
    /// Set only when mem itself is broken (§4a's last row): a missing binary,
    /// or output that will not parse. An empty store is not degraded.
    pub degraded: Option<String>,
    pub rows: Vec<T>,
}

/// Why a list verb's answer is not usable, or `None` if it is.
///
/// §4a's "exit 1 + empty stdout = absent, render an em dash" row is about
/// `status`, which is a singleton. A **list** verb that matched nothing still
/// prints its document — that is mem's contract, in its own schema README: *"a
/// filtered read that matches nothing prints this document with an empty array
/// and exits 1"*. So empty stdout from `questions`, `log` or `projects` is mem
/// failing, and an empty section is the wrong way to say so: "Nothing waiting"
/// while a question is in fact waiting is the worst answer this page can give.
pub fn list_fault(outcome: &Outcome, verb: &str) -> Option<String> {
    match outcome {
        Outcome::Broken(why) => Some(why.clone()),
        Outcome::Absent => Some(format!(
            "mem printed nothing for `{verb}`, which is not how it says \"no rows\""
        )),
        Outcome::Json(_) => None,
    }
}

/// Every project that has pages, with its pages: one read per project,
/// because `mem wiki` is a per-project verb.
pub fn wiki(mem: &MemCli) -> Section<WikiProject> {
    let outcome = mem.projects();
    let mut fault = list_fault(&outcome, "projects");
    let mut rows = Vec::new();
    for name in project_names(&outcome) {
        let listing = mem.wiki(&name);
        if let Some(why) = list_fault(&listing, "wiki") {
            // One project's failure costs that project's rows and says so,
            // rather than quietly shortening the list.
            fault.get_or_insert(format!("{name}: {why}"));
            continue;
        }
        let mut pages: Vec<WikiPage> = listing.rows("pages").iter().map(wiki_row).collect();
        pages.sort_by(|a, b| page_order(a).cmp(&page_order(b)));
        if !pages.is_empty() {
            rows.push(WikiProject { name, pages });
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Section {
        degraded: fault,
        rows,
    }
}

/// One page's text, byte for byte as mem printed it, or `None` for a page that
/// is not there — which is `mem wiki <slug>`'s exit 1 with empty stdout, the
/// same shape as a missing status.md.
pub fn wiki_text(mem: &MemCli, project: &str, slug: &str) -> Option<String> {
    let Outcome::Json(value) = &*mem.wiki_page(project, slug) else {
        return None;
    };
    value.get("text")?.as_str().map(str::to_string)
}

/// Whether mem knows this project at all. The name comes out of a URL, so it is
/// checked against the list rather than against a guess about what a project
/// name may contain.
pub fn is_known_project(mem: &MemCli, name: &str) -> bool {
    project_names(&mem.projects()).iter().any(|p| p == name)
}

/// A `/p/<project>` singleton read — a stored plan's text or one item —
/// present, absent, or degraded when mem itself is broken. The two
/// are never collapsed into one `None`: an absent slug or id is a 404, a
/// broken mem is the degraded banner.
#[derive(Debug)]
pub struct Singleton<T> {
    pub degraded: Option<String>,
    pub value: Option<T>,
}

/// The current plan's text — `/p/<project>/plan`.
pub fn plan_text(mem: &MemCli, project: &str) -> Singleton<String> {
    let outcome = mem.plan(project);
    Singleton {
        degraded: singleton_fault(&outcome),
        value: singleton_text(&outcome),
    }
}

/// One stored plan's text, by slug — `/p/<project>/plan/<slug>`.
pub fn plan_slug_text(mem: &MemCli, project: &str, slug: &str) -> Singleton<String> {
    let outcome = mem.plan_slug(project, slug);
    Singleton {
        degraded: singleton_fault(&outcome),
        value: singleton_text(&outcome),
    }
}

/// A building project's last five log lines, for its front page.
pub fn last_moves(mem: &MemCli, project: &str, now_ms: i64) -> Section<Activity> {
    let outcome = mem.log_n(project, LAST_MOVES);
    let rows = outcome
        .rows("items")
        .iter()
        .map(|row| activity_item(row, now_ms))
        .collect();
    Section {
        degraded: list_fault(&outcome, "log"),
        rows,
    }
}

/// `/p/<project>/item/<id>`: one item, whole.
#[derive(Debug)]
pub struct ItemDetail {
    pub kind: String,
    pub title: String,
    pub body: String,
    /// The item's own project, from `mem show`'s row. `mem show` has no
    /// `--project` — it resolves an id against the whole index — so the route
    /// compares this against the project in the URL and 404s on a mismatch
    /// (review 1 of detail).
    pub project: Option<String>,
}

/// `None` for an id mem does not have.
pub fn item_detail(mem: &MemCli, id: &str) -> Singleton<ItemDetail> {
    let outcome = mem.show(id);
    let value = match &*outcome {
        Outcome::Json(json) => json
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .map(|item| ItemDetail {
                kind: string(item, "kind"),
                title: string(item, "title"),
                body: item
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                project: optional(item, "project"),
            }),
        _ => None,
    };
    Singleton {
        degraded: singleton_fault(&outcome),
        value,
    }
}

/// `index` first, then alphabetical: the index page is the one a reader wants
/// first, and it is the page the plan makes every wiki keep.
fn page_order(page: &WikiPage) -> (u8, &str) {
    (u8::from(page.slug != "index"), &page.slug)
}

fn wiki_row(row: &Value) -> WikiPage {
    let slug = string(row, "slug");
    WikiPage {
        title: optional(row, "title").unwrap_or_else(|| slug.clone()),
        bytes: row.get("bytes").and_then(Value::as_u64).unwrap_or_default(),
        modified: optional(row, "modified"),
        slug,
    }
}

/// mem's page-slug rule, `[a-z0-9][a-z0-9-]{0,63}`, applied before a slug off a
/// URL becomes an argument to a child process. It is what keeps `..`, a leading
/// dash and a dot-temp out of both the store and the argv.
pub fn is_slug(slug: &str) -> bool {
    let mut bytes = slug.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    slug.len() <= SLUG_MAX
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An id is a short id (8 base32 characters) or a full ULID (26),
/// checked before it becomes an argument to `mem show`.
pub fn is_item_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (bytes.len() == 8 || bytes.len() == 26) && bytes.iter().all(|b| BASE32.contains(b))
}

fn project_names(outcome: &Outcome) -> Vec<String> {
    outcome
        .rows("projects")
        .iter()
        .filter_map(|p| p["name"].as_str().map(str::to_string))
        .collect()
}

fn activity_item(row: &Value, now_ms: i64) -> Activity {
    let id = string(row, "id");
    let millis = ulid_millis(&id);
    Activity {
        short_id: string(row, "short_id"),
        kind: string(row, "kind"),
        title: string(row, "title"),
        project: optional(row, "project"),
        machine: string(row, "machine"),
        at: millis.map(rfc3339),
        age: age(millis, now_ms),
        id,
    }
}

fn string(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// mem's contract is "absent is `null`, never missing", so a null and a missing
/// key are the same thing to us.
fn optional(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// Milliseconds since the epoch out of a ULID's first ten characters.
///
/// Returns `None` for anything that is not one, rather than a wrong time: a
/// wrong age is worse than a missing one.
pub fn ulid_millis(id: &str) -> Option<i64> {
    let bytes = id.as_bytes();
    if bytes.len() != 26 || !bytes.iter().all(|b| BASE32.contains(b)) {
        return None;
    }
    // Only the first ten characters are the timestamp; the other sixteen are
    // the entropy, and are checked above only to reject a malformed id whole.
    let mut millis: i64 = 0;
    for byte in &bytes[..10] {
        let index = BASE32.iter().position(|c| c == byte)?;
        millis = millis * 32 + index as i64;
    }
    Some(millis)
}

pub fn rfc3339(millis: i64) -> String {
    match jiff::Timestamp::from_millisecond(millis) {
        Ok(timestamp) => format!("{timestamp:.3}"),
        Err(_) => String::new(),
    }
}

/// "4s", "12m", "3h", "6d" — short enough for a phone, and honest about the
/// two cases where there is no answer.
pub fn age(millis: Option<i64>, now_ms: i64) -> String {
    let Some(millis) = millis else {
        return "—".to_string();
    };
    let seconds = (now_ms - millis) / 1000;
    if seconds < 0 {
        // A clock that stepped backwards, or an item from another machine
        // whose clock is ahead. Neither is worth a negative age.
        return "now".to_string();
    }
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// Only a broken read is a fault for a singleton file: an absent one is a
/// project that has simply not written it yet (memcli's own §4a table).
fn singleton_fault(outcome: &Outcome) -> Option<String> {
    match outcome {
        Outcome::Broken(why) => Some(why.clone()),
        _ => None,
    }
}

fn singleton_text(outcome: &Outcome) -> Option<String> {
    let Outcome::Json(value) = outcome else {
        return None;
    };
    value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ulid_carries_its_own_timestamp() {
        // Minted by the real mem during the spec review, on 2026-08-18.
        let millis = ulid_millis("01M0BF1F8BY8FXGZS428J1TSD1").unwrap();
        assert!(
            rfc3339(millis).starts_with("2026-08-18T"),
            "{}",
            rfc3339(millis)
        );

        assert_eq!(ulid_millis("0000000000ZZZZZZZZZZZZZZZZ"), Some(0));
        assert_eq!(ulid_millis("too-short"), None);
        assert_eq!(
            ulid_millis("01M0BF1F8BY8FXGZS428J1TSD!"),
            None,
            "not base32"
        );
        // The `I`, `L`, `O` and `U` a ULID never uses.
        assert_eq!(ulid_millis("01M0BF1F8IY8FXGZS428J1TSD1"), None);
    }

    #[test]
    fn ids_minted_a_minute_apart_still_sort_by_id() {
        // The three the review recorded, which share eight leading characters
        // — and are still correctly ordered by the ninth.
        let mut ids = [
            "01M0BF3B9VWD6PMF213HNZ1Q1D",
            "01M0BF1F8BY8FXGZS428J1TSD1",
            "01M0BF1F8FQ31V756YQHC0DSSK",
        ];
        ids.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids[0], "01M0BF3B9VWD6PMF213HNZ1Q1D", "newest first");

        let times: Vec<i64> = ids.iter().map(|id| ulid_millis(id).unwrap()).collect();
        assert!(times[0] >= times[1] && times[1] >= times[2], "{times:?}");
    }

    #[test]
    fn ages_are_short_and_never_negative() {
        let now = 1_000_000_000_000;
        assert_eq!(age(Some(now), now), "0s");
        assert_eq!(age(Some(now - 30_000), now), "30s");
        assert_eq!(age(Some(now - 90_000), now), "1m");
        assert_eq!(age(Some(now - 7_200_000), now), "2h");
        assert_eq!(age(Some(now - 5 * 86_400_000), now), "5d");
        assert_eq!(age(Some(now + 60_000), now), "now", "a clock that stepped");
        assert_eq!(age(None, now), "—");
    }

    #[test]
    fn an_empty_stdout_from_a_list_verb_is_a_fault_and_from_a_singleton_is_not() {
        // mem's contract: a filtered read that matched nothing still prints its
        // document. Nothing at all is mem failing, and an empty section would
        // report it as "no questions", which is exactly backwards.
        assert!(list_fault(&Outcome::Absent, "questions").is_some());
        assert!(list_fault(&Outcome::Broken("gone".into()), "questions").is_some());
        assert!(
            list_fault(
                &Outcome::Json(serde_json::json!({"questions": []})),
                "questions"
            )
            .is_none(),
            "an empty array is an answer"
        );
        // A singleton's empty stdout genuinely means absent.
        assert_eq!(singleton_text(&Outcome::Absent), None);
    }

    #[test]
    fn the_slug_rule_is_mems_own_so_nothing_else_reaches_an_argv() {
        for slug in ["index", "storage", "a", "0", "a-b-c", "x9"] {
            assert!(is_slug(slug), "{slug}");
        }
        for slug in [
            "",
            "..",
            "../status",
            "-flag",
            ".hidden",
            "Storage",
            "with space",
            "under_score",
            "trailing/",
        ] {
            assert!(!is_slug(slug), "{slug}");
        }
        assert!(is_slug(&"a".repeat(SLUG_MAX)));
        assert!(!is_slug(&"a".repeat(SLUG_MAX + 1)));
    }

    #[test]
    fn an_item_id_is_a_short_id_or_a_full_ulid_in_base32() {
        assert!(is_item_id("28J1TSD1"));
        assert!(is_item_id("01M0BF1F8BY8FXGZS428J1TSD1"));
        for id in ["", "short7", "toolong123", "28J1TSD!", "28j1tsd1"] {
            assert!(!is_item_id(id), "{id}");
        }
        // The `I`, `L`, `O` and `U` a ULID never uses.
        assert!(!is_item_id("01M0BF1F8IY8FXGZS428J1TSD1"));
    }

    #[test]
    fn the_index_page_sorts_first_and_the_rest_alphabetically() {
        let page = |slug: &str| WikiPage {
            slug: slug.to_string(),
            title: slug.to_string(),
            bytes: 0,
            modified: None,
        };
        let mut pages = [page("storage"), page("api"), page("index")];
        pages.sort_by(|a, b| page_order(a).cmp(&page_order(b)));
        let slugs: Vec<&str> = pages.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["index", "api", "storage"]);
    }

    #[test]
    fn a_page_with_no_heading_is_listed_under_its_slug() {
        let row = serde_json::json!({"slug": "storage", "title": "", "bytes": 12});
        let page = wiki_row(&row);
        assert_eq!(page.title, "storage");
        assert_eq!(page.bytes, 12);
        assert_eq!(page.modified, None);
    }
}
