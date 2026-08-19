//! Search, the ranking formula and the query ladder (spec §6).

mod common;

use common::{World, item, put};
use mem::index::{Index, Purpose};
use mem::item::Kind;
use mem::search::{Query, Scope, search};

const P: &str = "01K2AAAAAAAAAAAAAAAAAAAAAA";

fn ready(w: &World) -> Index {
    let index = Index::open(&w.index_path(), Purpose::Read).unwrap();
    index.reindex(&w.store(), false).unwrap();
    index
}

fn q<'a>(text: &'a str) -> Query<'a> {
    Query {
        text,
        scope: Scope::Project(Some(P.to_string())),
        ..Query::new(text)
    }
}

#[test]
fn the_top_hit_scores_one_and_the_rest_are_relative_to_it() {
    let w = World::new("search-scores");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(Kind::Fact, "redis sessions", "redis redis redis sessions"),
    );
    put(
        &store,
        Some(P),
        &item(Kind::Fact, "something else", "a passing mention of redis"),
    );

    let hits = search(&ready(&w), &q("redis")).unwrap();
    assert_eq!(hits.len(), 2);
    assert!((hits[0].score - 1.0).abs() < 1e-9, "top hit must be 1.00");
    assert!(hits[0].score >= hits[1].score);
    assert!(hits[1].score > 0.0, "scores are in (0, 1]");
}

#[test]
fn a_recent_item_outranks_an_identical_older_one() {
    let w = World::new("search-decay");
    w.project(P, "thing");
    let store = w.store();
    let mut old = item(Kind::Fact, "decaytoken here", "decaytoken body");
    old.meta.modified = "2025-08-18T00:00:00Z".parse().unwrap();
    let mut fresh = item(Kind::Fact, "decaytoken here", "decaytoken body");
    fresh.meta.modified = jiff::Timestamp::now();
    put(&store, Some(P), &old);
    put(&store, Some(P), &fresh);

    let hits = search(&ready(&w), &q("decaytoken")).unwrap();
    assert_eq!(
        hits[0].row.id, fresh.meta.id,
        "recency decay must order these"
    );
    assert!(hits[1].score < 0.2, "a year old is ~4 half-lives: {hits:?}");
}

#[test]
fn a_pinned_item_is_exempt_from_decay() {
    let w = World::new("search-pinned");
    w.project(P, "thing");
    let store = w.store();
    let mut old = item(Kind::Fact, "pinnedtoken here", "pinnedtoken body");
    old.meta.modified = "2025-08-18T00:00:00Z".parse().unwrap();
    old.meta.tags = vec!["pinned".to_string()];
    let mut fresh = item(Kind::Fact, "pinnedtoken here", "pinnedtoken body");
    fresh.meta.modified = jiff::Timestamp::now();
    put(&store, Some(P), &old);
    put(&store, Some(P), &fresh);

    let hits = search(&ready(&w), &q("pinnedtoken")).unwrap();
    let pinned = hits.iter().find(|h| h.row.id == old.meta.id).unwrap();
    assert!(
        pinned.score > 0.9,
        "a pinned item does not decay: {}",
        pinned.score
    );
}

#[test]
fn kind_boosts_break_ties_between_equal_matches() {
    let w = World::new("search-kinds");
    w.project(P, "thing");
    let store = w.store();
    let now = jiff::Timestamp::now();
    for kind in [Kind::Log, Kind::Fact, Kind::Answer, Kind::Ruling] {
        let mut it = item(kind, "boosttoken", "boosttoken body");
        it.meta.modified = now;
        it.meta.created = now;
        put(&store, Some(P), &it);
    }
    let hits = search(&ready(&w), &q("boosttoken")).unwrap();
    let order: Vec<&str> = hits.iter().map(|h| h.row.kind.as_str()).collect();
    assert_eq!(order, vec!["fact", "ruling", "answer", "log"], "{hits:?}");
}

#[test]
fn inactive_items_are_out_of_the_default_search_and_back_with_the_flag() {
    let w = World::new("search-archived");
    w.project(P, "thing");
    let store = w.store();
    let mut archived = item(Kind::Fact, "hiddentoken", "hiddentoken body");
    archived.meta.archived = Some(true);
    put(&store, Some(P), &archived);
    let superseded = item(Kind::Fact, "hiddentoken two", "hiddentoken body");
    put(&store, Some(P), &superseded);
    let mut replacement = item(Kind::Fact, "visibletoken", "replacement body");
    replacement.meta.supersedes = Some(superseded.meta.id.clone());
    put(&store, Some(P), &replacement);

    let index = ready(&w);
    assert_eq!(search(&index, &q("hiddentoken")).unwrap().len(), 0);
    let all = search(
        &index,
        &Query {
            include_archived: true,
            ..q("hiddentoken")
        },
    )
    .unwrap();
    assert_eq!(all.len(), 2, "--include-archived brings back both");
}

#[test]
fn kind_and_type_filters_narrow_the_result() {
    let w = World::new("search-filters");
    w.project(P, "thing");
    let store = w.store();
    let mut decision = item(Kind::Fact, "filtertoken a", "body");
    decision.meta.r#type = Some("decision".to_string());
    put(&store, Some(P), &decision);
    let mut gotcha = item(Kind::Fact, "filtertoken b", "body");
    gotcha.meta.r#type = Some("gotcha".to_string());
    put(&store, Some(P), &gotcha);
    put(&store, Some(P), &item(Kind::Log, "filtertoken c", "body"));

    let index = ready(&w);
    assert_eq!(search(&index, &q("filtertoken")).unwrap().len(), 3);
    assert_eq!(
        search(
            &index,
            &Query {
                kind: Some("fact"),
                ..q("filtertoken")
            }
        )
        .unwrap()
        .len(),
        2
    );
    let typed = search(
        &index,
        &Query {
            r#type: Some("decision"),
            ..q("filtertoken")
        },
    )
    .unwrap();
    assert_eq!(typed.len(), 1);
    assert_eq!(typed[0].row.id, decision.meta.id);
}

#[test]
fn global_items_rank_below_project_items() {
    let w = World::new("search-scope");
    w.project(P, "thing");
    let store = w.store();
    let now = jiff::Timestamp::now();
    let mut mine = item(Kind::Fact, "scopetoken", "scopetoken body");
    mine.meta.modified = now;
    let mut theirs = item(Kind::Fact, "scopetoken", "scopetoken body");
    theirs.meta.modified = now;
    put(&store, Some(P), &mine);
    put(&store, None, &theirs);

    let index = ready(&w);
    let hits = search(&index, &q("scopetoken")).unwrap();
    assert_eq!(hits.len(), 2, "project scope also serves global");
    assert_eq!(hits[0].row.id, mine.meta.id);
    assert!(hits[1].score < hits[0].score);

    let only_global = search(
        &index,
        &Query {
            scope: Scope::Global,
            ..q("scopetoken")
        },
    )
    .unwrap();
    assert_eq!(only_global.len(), 1);
    assert_eq!(only_global[0].row.id, theirs.meta.id);
}

#[test]
fn min_score_cuts_on_the_displayed_score() {
    let w = World::new("search-minscore");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(
            Kind::Fact,
            "cuttoken cuttoken",
            "cuttoken cuttoken cuttoken",
        ),
    );
    put(
        &store,
        Some(P),
        &item(
            Kind::Fact,
            "unrelated",
            "one cuttoken here among many other words",
        ),
    );

    let index = ready(&w);
    let all = search(&index, &q("cuttoken")).unwrap();
    assert_eq!(all.len(), 2);
    let cut = search(
        &index,
        &Query {
            min_score: Some(all[0].score - 1e-9),
            ..q("cuttoken")
        },
    )
    .unwrap();
    assert_eq!(cut.len(), 1, "only the top hit clears its own score");
}

#[test]
fn a_malformed_query_falls_back_to_quoted_terms() {
    let w = World::new("search-ladder");
    w.project(P, "thing");
    let store = w.store();
    put(
        &store,
        Some(P),
        &item(Kind::Fact, "ladder", "sessions use redis not the database"),
    );
    let index = ready(&w);

    // Each of these is an FTS5 syntax error raw; none may surface as SQL, and
    // the retry is every term quoted and ANDed — so a query whose terms are all
    // in the item still finds it, and one that carries a stray operator word
    // simply does not match.
    for text in [
        "redis AND",
        "\"unbalanced",
        "redis OR OR sessions",
        "(redis",
    ] {
        search(&index, &q(text)).expect("a malformed query is not an error");
    }
    assert_eq!(search(&index, &q("(redis")).unwrap().len(), 1);
    // An unbalanced quote is a phrase search raw; quoted term by term it is an
    // AND of two words the item has, so the retry finds it.
    assert_eq!(search(&index, &q("\"sessions redis")).unwrap().len(), 1);
    // A column filter is real FTS5 syntax and must keep working.
    assert_eq!(search(&index, &q("title:ladder")).unwrap().len(), 1);
    assert_eq!(search(&index, &q("body:redis")).unwrap().len(), 1);
    assert_eq!(search(&index, &q("title:redis")).unwrap().len(), 0);
}

#[test]
fn tags_are_searchable_and_a_no_hit_query_is_empty() {
    let w = World::new("search-tags");
    w.project(P, "thing");
    let store = w.store();
    let mut tagged = item(Kind::Fact, "tagged item", "body");
    tagged.meta.tags = vec!["redis".into(), "sessions".into()];
    put(&store, Some(P), &tagged);

    let index = ready(&w);
    assert_eq!(search(&index, &q("tags:redis")).unwrap().len(), 1);
    assert!(search(&index, &q("nothingmatchesthis")).unwrap().is_empty());
}

#[test]
fn a_search_line_never_exceeds_eighty_bytes() {
    let w = World::new("search-line");
    w.project(P, "thing");
    let store = w.store();
    let long = "a very long title ".repeat(20);
    put(&store, Some(P), &item(Kind::Fact, &long, "linetoken body"));
    put(
        &store,
        Some(P),
        &item(
            Kind::Fact,
            "\u{1F600}\u{1F600}\u{1F600} emoji title that runs on and on and on and on",
            "linetoken",
        ),
    );

    for hit in search(&ready(&w), &q("linetoken")).unwrap() {
        let line = hit.line();
        assert!(line.len() <= 80, "{} bytes: {line}", line.len());
        assert!(line.starts_with('#'));
    }
}

#[test]
fn the_limit_is_applied_after_ranking() {
    let w = World::new("search-limit");
    w.project(P, "thing");
    let store = w.store();
    for n in 0..10 {
        put(
            &store,
            Some(P),
            &item(Kind::Fact, &format!("limittoken {n}"), "limittoken body"),
        );
    }
    let hits = search(
        &ready(&w),
        &Query {
            limit: 3,
            ..q("limittoken")
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 3);
    assert!((hits[0].score - 1.0).abs() < 1e-9);
}
