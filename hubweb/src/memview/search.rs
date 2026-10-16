//! `mem search <query>` over the store's files: mem's FTS5 BM25 ranking,
//! worked out in memory, with mem's recency decay and kind boost on top.
//!
//! Two things are simpler than SQLite's: a token is a run of letters and
//! digits, lowercased, and the only stemming is the plural step of Porter's
//! (`nags` finds `nag`, `running` does not find `run`); and a query is its
//! words, each of which must appear, with FTS5's operators, phrases and
//! column filters read as plain words. A page section carries no date, since
//! mem dates it by the file's mtime, so it never decays.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::item::{Row, row_json};
use super::sections::{self, TOP};
use super::{Page, page_title, truncate_bytes};

/// BM25's parameters and mem's column weights: title 10, body 1, tags 5
/// for an item; heading 10, body 1 for a section.
const K1: f64 = 1.2;
const B: f64 = 0.75;
const W_TITLE: f64 = 10.0;
const W_BODY: f64 = 1.0;
const W_TAGS: f64 = 5.0;
const DECAY_HALF_LIFE_DAYS: f64 = 90.0;
const SECTIONS_PER_PAGE: usize = 3;
const SNIPPET_BUDGET: usize = 120;

/// One indexed document: its columns' tokens, each with its weight.
struct Doc {
    columns: Vec<(f64, Vec<String>)>,
}

impl Doc {
    fn len(&self) -> usize {
        self.columns.iter().map(|(_, tokens)| tokens.len()).sum()
    }

    fn has(&self, term: &str) -> bool {
        self.columns
            .iter()
            .any(|(_, t)| t.iter().any(|x| x == term))
    }

    fn freq(&self, term: &str) -> f64 {
        self.columns
            .iter()
            .map(|(w, t)| w * t.iter().filter(|x| *x == term).count() as f64)
            .sum()
    }
}

/// FTS5's bm25() for every document of one table that holds every term,
/// negative and lower for a better match, by the document's position.
fn bm25(docs: &[Doc], terms: &[String]) -> Vec<(usize, f64)> {
    if docs.is_empty() || terms.is_empty() {
        return Vec::new();
    }
    let rows = docs.len() as f64;
    let avgdl = docs.iter().map(Doc::len).sum::<usize>() as f64 / rows;
    let idf: Vec<f64> = terms
        .iter()
        .map(|t| {
            let hits = docs.iter().filter(|d| d.has(t)).count() as f64;
            let idf = ((rows - hits + 0.5) / (hits + 0.5)).ln();
            if idf <= 0.0 { 1e-6 } else { idf }
        })
        .collect();
    docs.iter()
        .enumerate()
        .filter(|(_, d)| terms.iter().all(|t| d.has(t)))
        .map(|(n, d)| {
            let dl = d.len() as f64;
            let score: f64 = terms
                .iter()
                .zip(&idf)
                .map(|(t, idf)| {
                    let f = d.freq(t);
                    idf * (f * (K1 + 1.0)) / (f + K1 * (1.0 - B + B * dl / avgdl))
                })
                .sum();
            (n, -score)
        })
        .collect()
}

/// The words of a text as the index keeps them.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| stem(&t.to_lowercase()))
        .collect()
}

/// Step 1a of Porter's stemmer: the plural endings.
fn stem(word: &str) -> String {
    if let Some(base) = word.strip_suffix("sses") {
        format!("{base}ss")
    } else if let Some(base) = word.strip_suffix("ies") {
        format!("{base}i")
    } else if word.ends_with("ss") {
        word.to_string()
    } else if let Some(base) = word.strip_suffix('s') {
        base.to_string()
    } else {
        word.to_string()
    }
}

/// An item's section of the index: title, body, tags.
fn item_doc(row: &Row) -> Doc {
    let m = row.meta();
    Doc {
        columns: vec![
            (W_TITLE, tokens(&m.title)),
            (W_BODY, tokens(&row.item.body)),
            (W_TAGS, tokens(&m.tags.join(" "))),
        ],
    }
}

/// One page section, as its hit reports it.
pub struct SectionHit<'a> {
    pub project_id: &'a str,
    pub project: &'a str,
    pub page: &'a Page,
    pub section: sections::Section,
}

fn section_doc(hit: &SectionHit) -> Doc {
    let body = &hit.page.text[hit.section.start..hit.section.end];
    // The preamble's heading line is the page's title, so the title ranks as
    // a heading does.
    let heading = if hit.section.hslug == TOP && hit.section.start == 0 {
        page_title(body)
    } else {
        hit.section.heading.clone()
    };
    Doc {
        columns: vec![(W_TITLE, tokens(&heading)), (W_BODY, tokens(body))],
    }
}

fn kind_boost(kind: &str) -> f64 {
    match kind {
        "fact" => 1.2,
        "ruling" => 1.1,
        "answer" => 0.9,
        "log" => 0.8,
        _ => 1.0,
    }
}

/// What the search ranks: every item and every page section in the store,
/// since BM25's weights count the whole table, and the project it serves.
pub struct Corpus<'a> {
    pub items: Vec<(&'a str, Row)>,
    pub sections: Vec<SectionHit<'a>>,
    pub project_id: &'a str,
}

struct Hit {
    json: Value,
    score: f64,
    modified: i64,
    id: String,
}

/// The `hits` mem prints for `query`, at most `limit`, best first.
pub fn search(corpus: &Corpus, query: &str, limit: usize, now: i64) -> Vec<Value> {
    let terms = tokens(query);
    let pool = limit.saturating_mul(5).max(50);

    let item_docs: Vec<Doc> = corpus.items.iter().map(|(_, r)| item_doc(r)).collect();
    let mut items: Vec<(usize, f64)> = bm25(&item_docs, &terms)
        .into_iter()
        .filter(|(n, _)| {
            let (project, row) = &corpus.items[*n];
            row.active && *project == corpus.project_id
        })
        .collect();
    items.sort_by(|a, b| a.1.total_cmp(&b.1));
    items.truncate(pool);

    let section_docs: Vec<Doc> = corpus.sections.iter().map(section_doc).collect();
    let mut matched: Vec<(usize, f64)> = bm25(&section_docs, &terms)
        .into_iter()
        .filter(|(n, _)| corpus.sections[*n].project_id == corpus.project_id)
        .collect();
    matched.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut kept: HashMap<&str, usize> = HashMap::new();
    let mut sections = Vec::new();
    for (n, bm25) in matched {
        let count = kept.entry(&corpus.sections[n].page.path).or_default();
        if *count == SECTIONS_PER_PAGE {
            continue;
        }
        *count += 1;
        sections.push((n, bm25));
        if sections.len() == pool {
            break;
        }
    }

    let best = items
        .iter()
        .chain(&sections)
        .map(|(_, s)| *s)
        .fold(f64::INFINITY, f64::min);
    let relevance = |bm25: f64| if best == 0.0 { 1.0 } else { bm25 / best };
    let mut hits: Vec<Hit> = Vec::new();
    for (n, bm25) in items {
        let row = &corpus.items[n].1;
        let m = row.meta();
        let pinned = m.tags.iter().any(|t| t == "pinned");
        let age_days = ((now - m.modified).max(0) as f64) / 86_400.0;
        let decay = if pinned {
            1.0
        } else {
            0.5_f64.powf(age_days / DECAY_HALF_LIFE_DAYS)
        };
        hits.push(Hit {
            json: row_json(row),
            score: relevance(bm25) * decay * kind_boost(&m.kind),
            modified: m.modified,
            id: m.id.clone(),
        });
    }
    for (n, bm25) in sections {
        let hit = &corpus.sections[n];
        let id = format!(
            "wiki:{}/{}#{}",
            hit.project_id, hit.page.slug, hit.section.hslug
        );
        let body = &hit.page.text[hit.section.start..hit.section.end];
        let json = json!({
            "id": id,
            "short_id": format!("{}#{}", hit.page.slug, hit.section.hslug),
            "kind": "wiki",
            "type": null,
            "title": page_title(&hit.page.text),
            "tags": [],
            "project": hit.project,
            "machine": "",
            "created": null,
            "modified": null,
            "active": true,
            "archived": false,
            "supersedes": null,
            "superseded_by": null,
            "answers": null,
            "audience": null,
            "task": null,
            "by": null,
            "replaces": null,
            "path": hit.page.path,
            "heading": hit.section.heading,
            "snippet": snippet(body, query),
            "bytes": body.len(),
        });
        hits.push(Hit {
            json,
            score: relevance(bm25),
            modified: now,
            id,
        });
    }

    let top = hits.iter().map(|h| h.score).fold(0.0_f64, f64::max);
    if top > 0.0 {
        for hit in &mut hits {
            hit.score /= top;
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.modified.cmp(&a.modified))
            .then_with(|| a.id.cmp(&b.id))
    });
    hits.truncate(limit);
    hits.into_iter()
        .map(|mut h| {
            h.json["score"] = json!((h.score * 100.0).round() / 100.0);
            h.json
        })
        .collect()
}

/// The section's line with the most query-word hits and the next line with
/// anything on it, each cut at 120 bytes.
fn snippet(body: &str, query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.rsplit(':').next().unwrap_or(t))
        .filter(|t| !matches!(*t, "AND" | "OR" | "NOT" | "NEAR"))
        .flat_map(|t| t.split(|c: char| !c.is_alphanumeric()))
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect();
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let hits = |line: &str| {
        let line = line.to_lowercase();
        terms
            .iter()
            .map(|t| line.matches(t.as_str()).count())
            .sum::<usize>()
    };
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
