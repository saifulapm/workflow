//! The performance harness of AC2: 1,000 items across 10 projects, warm search
//! p95 under 50 ms end to end (reindex included) and a full rebuild under 1 s.
//!
//! Every number here is measured on the **release binary**, one process per
//! measurement. An earlier version of this harness called `search()` in process
//! and passed with 4 ms of headroom while the real `mem search` took twice the
//! ceiling (code review 1, finding 1): everything the CLI does around the query
//! — resolving the project, opening the index, the incremental reindex — is
//! exactly what the acceptance criterion measures, so the harness pays for it
//! too. `CARGO_BIN_EXE_mem` is the binary cargo built for this profile, which
//! is what makes `--release` the whole build step.
//!
//! Ignored in a debug build — the numbers in the spec are for an optimised
//! binary, and in release this is an ordinary test. Run it with
//! `cargo test --release --test perf_corpus -- --nocapture`.

mod common;

use std::time::{Duration, Instant};

use common::{Rng, World, code, item, mem, put, stderr};
use mem::item::Kind;

const PROJECTS: usize = 10;
const ITEMS: usize = 1_000;

fn build_corpus(w: &World) -> Vec<String> {
    let mut rng = Rng::new(0x5EED);
    let words = [
        "redis",
        "sessions",
        "database",
        "driver",
        "deadlock",
        "cache",
        "queue",
        "worker",
        "migration",
        "index",
        "token",
        "handoff",
        "plan",
        "budget",
    ];
    let store = w.store();
    let names: Vec<String> = (0..PROJECTS)
        .map(|n| {
            let id = format!("01K2{n:022}");
            let name = format!("project-{n}");
            w.project(&id, &name);
            name
        })
        .collect();
    let ids: Vec<String> = (0..PROJECTS).map(|n| format!("01K2{n:022}")).collect();
    for n in 0..ITEMS {
        // Bodies between 100 and 2,000 bytes, as the acceptance criterion says.
        let target = 100 + rng.below(1_900);
        let mut body = String::with_capacity(target + 16);
        while body.len() < target {
            body.push_str(rng.pick(&words));
            body.push(' ');
        }
        let kind = *rng.pick(&[Kind::Fact, Kind::Log, Kind::Ruling, Kind::Handoff]);
        let title = format!("{} {} {n}", rng.pick(&words), rng.pick(&words));
        put(&store, Some(&ids[n % PROJECTS]), &item(kind, &title, &body));
    }
    names
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((sorted.len() as f64 * p).ceil() as usize).max(1);
    sorted[rank - 1]
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release-only performance harness")]
fn a_thousand_items_search_warm_under_fifty_milliseconds() {
    let w = World::new("perf");
    let names = build_corpus(&w);
    // Not a checkout: `--project` carries the scope, so no `git rev-parse` runs
    // and the measurement is the store's cost rather than git's.
    let cwd = w.plain_dir("cwd");

    let rebuild = Instant::now();
    let out = mem(&w, &cwd, &["reindex", "--full", "--json"]);
    let rebuild = rebuild.elapsed();
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["indexed"], serde_json::json!(ITEMS));
    println!("full rebuild of {ITEMS} items: {rebuild:?}");
    assert!(rebuild.as_secs_f64() < 1.0, "full rebuild took {rebuild:?}");

    let terms = [
        "redis",
        "sessions redis",
        "title:token",
        "deadlock",
        "queue worker",
    ];
    let mut timings = Vec::new();
    for n in 0..100 {
        let term = terms[n % terms.len()];
        let project = &names[n % PROJECTS];
        let started = Instant::now();
        let out = mem(&w, &cwd, &["search", term, "--project", project]);
        timings.push(started.elapsed());
        assert_eq!(code(&out), 0, "'{term}' found nothing: {}", stderr(&out));
    }
    timings.sort();
    println!(
        "warm search p50 {:?}, p95 {:?}",
        percentile(&timings, 0.50),
        percentile(&timings, 0.95)
    );
    let p95 = percentile(&timings, 0.95);
    assert!(p95.as_millis() < 50, "p95 was {p95:?}");
}
