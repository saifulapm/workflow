//! Search and ranking (spec §6).
//!
//! `bm25()` returns a negative number and more negative is better, so the
//! displayed score is `bm25_i / bm25_best`, which lands in (0, 1] with the best
//! hit at 1.00. Recency decay and the kind boost are applied on top and the
//! result is re-normalised, so a score only ever means "relative to the other
//! hits for this query".

use anyhow::{Result, anyhow};
use rusqlite::{ErrorCode, params_from_iter};

use crate::index::{Index, ROW_COLUMNS, Row, WIKI_KIND, page_row_columns};

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
}

impl Hit {
    /// `#<short8>  1.00  fact  2026-08-12  <title>` for an item, and
    /// `wiki:<slug>  1.00  wiki  2026-08-12  <title>` for a page: a page has no
    /// id to show, and the label is what keeps it from reading as an item.
    /// Capped at 80 bytes with the title as the part that gives way first.
    pub fn line(&self) -> String {
        let date = crate::timefmt::date(self.row.modified_epoch);
        let head = match self.row.wiki_slug() {
            Some(slug) => format!("wiki:{slug}"),
            None => format!("#{}", self.row.short_id),
        };
        let prefix = format!("{head}  {:.2}  {}  {}  ", self.score, self.row.kind, date);
        let room = LINE_BUDGET.saturating_sub(prefix.len());
        let line = format!("{prefix}{}", truncate_bytes(&self.row.title, room));
        // A slug may run to 64 characters, which is a prefix with no room left
        // in it. The budget is the promise, so it is kept last as well as first.
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
    let mut rows = with_ladder(query.text, |text| match_rows(index, text, query, pool))?;
    // Items and pages are two corpora, so they are two matches and one ranking:
    // BM25 is relative to its own table, and a score here only ever means
    // "relative to the other hits for this query" anyway.
    if wants_pages(query) {
        rows.extend(with_ladder(query.text, |text| {
            match_pages(index, text, query, pool)
        })?);
    }

    let best = rows
        .iter()
        .map(|(_, bm25)| *bm25)
        .fold(f64::INFINITY, f64::min);
    let now = jiff::Timestamp::now().as_second();

    let mut hits: Vec<Hit> = rows
        .into_iter()
        .map(|(row, bm25)| {
            // Both bm25 values are negative, so the ratio is positive and the
            // best hit is exactly 1. A zero best means BM25 could not tell the
            // hits apart at all.
            let relevance = if best == 0.0 { 1.0 } else { bm25 / best };
            let score = relevance * decay(&row, now) * kind_boost(&row.kind) * scope_factor(&row);
            Hit { row, score }
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
    Ok(hits)
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
fn with_ladder(
    text: &str,
    run: impl Fn(&str) -> rusqlite::Result<Vec<(Row, f64)>>,
) -> Result<Vec<(Row, f64)>> {
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

/// The page side of the same query. Pages carry no tags and are never archived
/// or superseded, so the only filter they take is the scope.
fn match_pages(
    index: &Index,
    text: &str,
    query: &Query<'_>,
    pool: usize,
) -> rusqlite::Result<Vec<(Row, f64)>> {
    let mut sql = format!(
        "SELECT {}, bm25(pages_fts, {W_TITLE}, {W_BODY}) AS bm25
         FROM pages_fts JOIN pages ON pages.rowid = pages_fts.rowid
         WHERE pages_fts MATCH ?1",
        page_row_columns()
    );
    let mut args: Vec<String> = vec![text.to_string()];
    if let Scope::Project(Some(id)) = &query.scope {
        args.push(id.clone());
        sql.push_str(&format!(" AND pages.project_id = ?{}", args.len()));
    }
    sql.push_str(&format!(" ORDER BY bm25 ASC LIMIT {pool}"));

    let mut stmt = index.conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
        Ok((Row::from_sql(r)?, r.get::<_, f64>("bm25")?))
    })?;
    rows.collect()
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
