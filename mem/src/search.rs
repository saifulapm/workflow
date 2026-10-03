//! Search and ranking (spec §6).
//!
//! `bm25()` returns a negative number and more negative is better, so the
//! displayed score is `bm25_i / bm25_best`, which lands in (0, 1] with the best
//! hit at 1.00. Recency decay and the kind boost are applied on top and the
//! result is re-normalised, so a score only ever means "relative to the other
//! hits for this query".

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use rusqlite::{ErrorCode, params_from_iter};

use crate::index::{Index, ROW_COLUMNS, Row, WIKI_KIND, section_row_columns};

/// BM25 column weights: title outranks tags, tags outrank body.
const W_TITLE: f64 = 10.0;
const W_BODY: f64 = 1.0;
const W_TAGS: f64 = 5.0;

/// Half the weight every 90 days (spec §6).
const DECAY_HALF_LIFE_DAYS: f64 = 90.0;

/// Global items are served alongside project items but never outrank them:
/// "project ∪ global, global ranked lower" needs a number, and this is it.
const GLOBAL_PENALTY: f64 = 0.8;

/// Items tagged this are exempt from recency decay.
pub const PINNED_TAG: &str = "pinned";

/// A search line is capped at 80 bytes so a digest can afford one per hit.
pub const LINE_BUDGET: usize = 80;

/// Each snippet line under a section row is cut here.
pub const SNIPPET_BUDGET: usize = 120;

/// A page keeps this many of its sections in the ranking, so one long page
/// that matches everywhere cannot crowd out every other hit.
const SECTIONS_PER_PAGE: usize = 3;

fn kind_boost(kind: &str) -> f64 {
    match kind {
        "fact" => 1.2,
        "ruling" => 1.1,
        "answer" => 0.9,
        "log" => 0.8,
        // handoff and question are pinned sections of the digest, not ranked
        // material; they carry no boost either way.
        _ => 1.0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The named project plus global. `None` means the working directory has no
    /// project, so this is global alone.
    Project(Option<String>),
    Global,
    All,
}

#[derive(Debug, Clone)]
pub struct Query<'a> {
    pub text: &'a str,
    pub kind: Option<&'a str>,
    pub r#type: Option<&'a str>,
    pub limit: usize,
    pub min_score: Option<f64>,
    pub include_archived: bool,
    pub scope: Scope,
}

impl<'a> Query<'a> {
    pub fn new(text: &'a str) -> Query<'a> {
        Query {
            text,
            kind: None,
            r#type: None,
            limit: 20,
            min_score: None,
            include_archived: false,
            scope: Scope::Project(None),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub row: Row,
    pub score: f64,
    /// A section's heading as written, `top` for the preamble; None for an item.
    pub heading: Option<String>,
    /// A section's line with the most query-term hits and the line after it,
    /// newline-separated and each cut at 120 bytes; None for an item.
    pub snippet: Option<String>,
    /// A section's size in bytes, heading line included; 0 for an item.
    pub bytes: u64,
}

impl Hit {
    /// `#<short8>  1.00  fact  2026-08-12  <title>` for an item, capped at 80
    /// bytes with the title as the part that gives way first.
    ///
    /// A section is `wiki:<slug>#<hslug>  1.00  <bytes>` and its snippet lines
    /// under it, indented two spaces: a section has no id to show, the label is
    /// what keeps it from reading as an item, and the size is what a reader
    /// weighs before pulling the section in.
    pub fn line(&self) -> String {
        if self.row.kind == WIKI_KIND {
            let mut out = format!(
                "wiki:{}  {:.2}  {}",
                self.row.short_id, self.score, self.bytes
            );
            for line in self.snippet.iter().flat_map(|s| s.lines()) {
                out.push_str("\n  ");
                out.push_str(line);
            }
            return out;
        }
        let date = crate::timefmt::date(self.row.modified_epoch);
        let head = format!("#{}", self.row.short_id);
        let prefix = format!("{head}  {:.2}  {}  {}  ", self.score, self.row.kind, date);
        let room = LINE_BUDGET.saturating_sub(prefix.len());
        let line = format!("{prefix}{}", truncate_bytes(&self.row.title, room));
        truncate_bytes(&line, LINE_BUDGET)
    }
}

/// Truncates on a character boundary, leaving an ellipsis when anything is cut.
pub fn truncate_bytes(text: &str, budget: usize) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    if flat.len() <= budget {
        return flat;
    }
    // The ellipsis is three bytes, and the budget is a byte budget.
    const ELLIPSIS: usize = '…'.len_utf8();
    if budget <= ELLIPSIS {
        return String::new();
    }
    let mut end = budget - ELLIPSIS;
    while end > 0 && !flat.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &flat[..end])
}

pub fn search(index: &Index, query: &Query<'_>) -> Result<Vec<Hit>> {
    // Decay and the kind boost can reorder hits, so rank a wider pool than the
    // caller asked for and cut to the limit afterwards.
    let pool = query.limit.saturating_mul(5).max(50);
    let mut rows: Vec<Matched> =
        with_ladder(query.text, |text| match_rows(index, text, query, pool))?
            .into_iter()
            .map(|(row, bm25)| Matched {
                row,
                bm25,
                heading: None,
                bytes: 0,
            })
            .collect();
    // Items and sections are two corpora, so they are two matches and one
    // ranking: BM25 is relative to its own table, and a score here only ever
    // means "relative to the other hits for this query" anyway.
    if wants_pages(query) {
        rows.extend(with_ladder(query.text, |text| {
            match_sections(index, text, query, pool)
        })?);
    }

    let best = rows.iter().map(|m| m.bm25).fold(f64::INFINITY, f64::min);
    let now = jiff::Timestamp::now().as_second();

    let mut hits: Vec<Hit> = rows
        .into_iter()
        .map(|m| {
            // Both bm25 values are negative, so the ratio is positive and the
            // best hit is exactly 1. A zero best means BM25 could not tell the
            // hits apart at all.
            let relevance = if best == 0.0 { 1.0 } else { m.bm25 / best };
            let row = m.row;
            let score = relevance * decay(&row, now) * kind_boost(&row.kind) * scope_factor(&row);
            Hit {
                row,
                score,
                heading: m.heading,
                snippet: None,
                bytes: m.bytes,
            }
        })
        .collect();

    let top = hits.iter().map(|h| h.score).fold(0.0_f64, f64::max);
    if top > 0.0 {
        for hit in &mut hits {
            hit.score /= top;
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.row.modified_epoch.cmp(&a.row.modified_epoch))
            .then_with(|| a.row.id.cmp(&b.row.id))
    });
    if let Some(min) = query.min_score {
        hits.retain(|h| h.score >= min);
    }
    hits.truncate(query.limit);
    // The snippet is read from the page file and only for the hits that are
    // shown: the section index is contentless, so it holds no text to give.
    let terms = snippet_terms(query.text);
    for hit in &mut hits {
        hit.snippet = section_snippet(&hit.row, &terms);
    }
    Ok(hits)
}

/// One match before ranking. `heading` and `bytes` are a section's.
struct Matched {
    row: Row,
    bm25: f64,
    heading: Option<String>,
    bytes: u64,
}

/// The words a query looks for, lowercased, without FTS5's operators, column
/// filters or quoting.
fn snippet_terms(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|t| t.rsplit(':').next().unwrap_or(t))
        .filter(|t| !matches!(*t, "AND" | "OR" | "NOT" | "NEAR"))
        .flat_map(|t| t.split(|c: char| !c.is_alphanumeric()))
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// The section's line with the most query-term hits and the next line with
/// anything on it, each cut at 120 bytes. A blank line is skipped because it
/// would print as nothing. None when the page has changed under the index
/// and the section is no longer there.
fn section_snippet(row: &Row, terms: &[String]) -> Option<String> {
    let hslug = row.wiki_hslug()?;
    let text = std::fs::read_to_string(&row.path).ok()?;
    let section = crate::sections::split(&text)
        .into_iter()
        .find(|s| s.hslug == hslug)?;
    let lines: Vec<&str> = text[section.start..section.end]
        .lines()
        .map(str::trim)
        .collect();
    let hits = |line: &str| {
        let line = line.to_lowercase();
        terms
            .iter()
            .map(|t| line.matches(t.as_str()).count())
            .sum::<usize>()
    };
    // The first line with the most hits; with none at all, the first line that
    // has anything on it, which is the heading.
    let mut best: Option<(usize, usize)> = None;
    for (n, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let count = hits(line);
        if best.is_none_or(|(_, most)| count > most) {
            best = Some((n, count));
        }
    }
    let (n, _) = best?;
    let mut out = truncate_bytes(lines[n], SNIPPET_BUDGET);
    if let Some(next) = lines[n + 1..].iter().find(|l| !l.is_empty()) {
        out.push('\n');
        out.push_str(&truncate_bytes(next, SNIPPET_BUDGET));
    }
    Some(out)
}

fn decay(row: &Row, now: i64) -> f64 {
    if row.tags.iter().any(|t| t == PINNED_TAG) {
        return 1.0;
    }
    let age_days = ((now - row.modified_epoch).max(0) as f64) / 86_400.0;
    0.5_f64.powf(age_days / DECAY_HALF_LIFE_DAYS)
}

fn scope_factor(row: &Row) -> f64 {
    if row.project_id.is_none() {
        GLOBAL_PENALTY
    } else {
        1.0
    }
}

/// The query ladder: the text as written, and if FTS5 calls that a syntax
/// error, every term quoted. A raw SQLite failure is never the user's business.
fn with_ladder<T>(text: &str, run: impl Fn(&str) -> rusqlite::Result<Vec<T>>) -> Result<Vec<T>> {
    match run(text) {
        Ok(rows) => Ok(rows),
        Err(e) if is_query_syntax_error(&e) => {
            run(&quote_terms(text)).map_err(|_| anyhow!("could not run that search"))
        }
        Err(_) => Err(anyhow!("could not run that search")),
    }
}

/// A page has no type, no kind but `wiki`, and no existence outside a project,
/// so a query that asks for anything else is asking for no pages at all.
fn wants_pages(query: &Query<'_>) -> bool {
    query.r#type.is_none()
        && query.kind.is_none_or(|kind| kind == WIKI_KIND)
        && !matches!(query.scope, Scope::Global | Scope::Project(None))
}

fn match_rows(
    index: &Index,
    text: &str,
    query: &Query<'_>,
    pool: usize,
) -> rusqlite::Result<Vec<(Row, f64)>> {
    let mut sql = format!(
        "SELECT {ROW_COLUMNS}, bm25(items_fts, {W_TITLE}, {W_BODY}, {W_TAGS}) AS bm25
         FROM items_fts JOIN items ON items.rowid = items_fts.rowid
         WHERE items_fts MATCH ?1"
    );
    let mut args: Vec<String> = vec![text.to_string()];
    if !query.include_archived {
        sql.push_str(" AND items.active = 1");
    }
    if let Some(kind) = query.kind {
        args.push(kind.to_string());
        sql.push_str(&format!(" AND items.kind = ?{}", args.len()));
    }
    if let Some(ty) = query.r#type {
        args.push(ty.to_string());
        sql.push_str(&format!(" AND items.type = ?{}", args.len()));
    }
    match &query.scope {
        Scope::Project(Some(id)) => {
            args.push(id.clone());
            sql.push_str(&format!(
                " AND (items.project_id = ?{} OR items.project_id IS NULL)",
                args.len()
            ));
        }
        Scope::Project(None) | Scope::Global => sql.push_str(" AND items.project_id IS NULL"),
        Scope::All => {}
    }
    sql.push_str(&format!(" ORDER BY bm25 ASC LIMIT {pool}"));

    let mut stmt = index.conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((Row::from_sql(r)?, r.get::<_, f64>("bm25")?))
    })?;
    rows.collect()
}

/// The page side of the same query, one row per section. Pages carry no tags
/// and are never archived or superseded, so the only filter they take is the
/// scope. A page keeps its best three sections, so a page that matches as a
/// whole prints those rather than every part of itself.
fn match_sections(
    index: &Index,
    text: &str,
    query: &Query<'_>,
    pool: usize,
) -> rusqlite::Result<Vec<Matched>> {
    let mut sql = format!(
        "SELECT {}, sections.page_rowid, sections.heading, sections.bytes,
                bm25(sections_fts, {W_TITLE}, {W_BODY}) AS bm25
         FROM sections_fts
         JOIN sections ON sections.rowid = sections_fts.rowid
         JOIN pages ON pages.rowid = sections.page_rowid
         WHERE sections_fts MATCH ?1",
        section_row_columns()
    );
    let mut args: Vec<String> = vec![text.to_string()];
    if let Scope::Project(Some(id)) = &query.scope {
        args.push(id.clone());
        sql.push_str(&format!(" AND pages.project_id = ?{}", args.len()));
    }
    sql.push_str(" ORDER BY bm25 ASC");

    let mut stmt = index.conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((
            r.get::<_, i64>("page_rowid")?,
            Matched {
                row: Row::from_sql(r)?,
                bm25: r.get("bm25")?,
                heading: Some(r.get("heading")?),
                bytes: r.get::<_, i64>("bytes")?.max(0) as u64,
            },
        ))
    })?;
    let mut kept: HashMap<i64, usize> = HashMap::new();
    let mut out = Vec::new();
    for row in rows {
        let (page, matched) = row?;
        let count = kept.entry(page).or_default();
        if *count == SECTIONS_PER_PAGE {
            continue;
        }
        *count += 1;
        out.push(matched);
        if out.len() == pool {
            break;
        }
    }
    Ok(out)
}

/// FTS5 reports a bad query as a plain SQLITE_ERROR; anything else (corruption,
/// a missing table) is not something re-quoting the terms would fix.
fn is_query_syntax_error(err: &rusqlite::Error) -> bool {
    matches!(err.sqlite_error_code(), Some(ErrorCode::Unknown))
}

/// The second rung of the ladder: every term quoted as a phrase, joined by
/// FTS5's implicit AND. Nothing here can be an operator.
pub fn quote_terms(text: &str) -> String {
    let terms: Vec<String> = text
        .split_whitespace()
        .map(|t| t.replace('"', " "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    terms.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_neutralises_every_operator() {
        assert_eq!(quote_terms("redis AND"), "\"redis\" \"AND\"");
        assert_eq!(quote_terms("\"unbalanced"), "\"unbalanced\"");
        assert_eq!(quote_terms("(redis"), "\"(redis\"");
        assert_eq!(quote_terms("   "), "");
    }

    #[test]
    fn truncation_respects_char_boundaries_and_the_budget() {
        assert_eq!(truncate_bytes("short", 20), "short");
        assert_eq!(truncate_bytes("one\ntwo", 20), "one two");
        let cut = truncate_bytes("\u{1F600}\u{1F600}\u{1F600}", 6);
        assert!(cut.len() <= 6, "{cut:?}");
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_bytes("abc", 1), "");
        assert_eq!(truncate_bytes("abcdef", 4), "a…");
        assert!(truncate_bytes("abcdef", 5).len() <= 5);
    }
}
