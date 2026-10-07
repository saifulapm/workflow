//! `GET /p/<project>/wiki` with and without `q`, and the contents list of
//! `GET /wiki/<project>/<slug>`.
//!
//! The fake `mem` prints recorded JSON and logs every argv whole, so a query
//! full of shell characters can be checked as the one argument it must stay.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::{Hub, TempDir, body_of, fixture_bin, invocations, status_of, wait_for};
use hub::form::encode_component;
use serde_json::json;

const PROJECT: &str = "gamma";

/// The page every test reads: a preamble, then four `## ` sections, two of
/// them with the same heading and one with inline code in it.
const STORAGE: &str = "# How storage works\n\nIntro.\n\n\
     ## How it flushes\n\nThe flush is atomic.\n\n\
     ## Notes\n\none\n\n\
     ## Notes\n\ntwo\n\n\
     ## `fsync` and you\n\nthree\n";

/// The sections mem gives `STORAGE`, `top` first.
const SECTIONS: [(&str, &str); 5] = [
    ("top", "top"),
    ("How it flushes", "how-it-flushes"),
    ("Notes", "notes"),
    ("Notes", "notes-2"),
    ("`fsync` and you", "fsync-and-you"),
];

struct World {
    _dir: TempDir,
    hub: Hub,
    log: PathBuf,
}

impl World {
    /// A hub over a `mem` that answers `projects`, the three `wiki` reads and
    /// `search`: the word `flush` finds one section and one item, and every
    /// other query finds nothing, printed the way mem prints it, with exit 1.
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        let bins = dir.join("bin");
        let log = dir.join("argv.log");
        let projects = json!({"projects": [{
            "id": "p-gamma", "name": PROJECT, "remote": null, "aliases": [],
            "created": "2026-01-01", "items": 3, "current": false, "checkouts": [],
        }]});
        let pages = json!({"pages": [
            {"slug": "storage", "title": "How storage works", "bytes": 120, "modified": "2026-10-01"},
            {"slug": "index", "title": "Gamma's wiki", "bytes": 40, "modified": "2026-10-01"},
        ]});
        let page = json!({"slug": "storage", "text": STORAGE});
        let sections = json!({"sections": SECTIONS
            .iter()
            .map(|(heading, hslug)| json!({"heading": heading, "hslug": hslug, "bytes": 10}))
            .collect::<Vec<_>>()});
        let hits = json!({"hits": [
            {"kind": "wiki", "id": "wiki:p-gamma/storage#how-it-flushes",
             "short_id": "storage#how-it-flushes", "title": "How storage works",
             "heading": "How it flushes", "snippet": "The [flush] is <atomic>",
             "project": PROJECT},
            {"kind": "fact", "id": "01M40JS4E731V1CG1P8H1JYAN3", "short_id": "8H1JYAN3",
             "title": "Flush on every write", "project": PROJECT},
        ]});
        fixture_bin(
            &bins,
            "mem",
            &format!(
                "printf '\\1' >> '{log}'\n\
                 for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
                 for last; do :; done\n\
                 case \"$*\" in\n\
                 projects*) printf '%s\\n' '{projects}' ;;\n\
                 questions*) printf '%s\\n' '{{\"questions\":[]}}' ;;\n\
                 *--sections*) printf '%s\\n' '{sections}' ;;\n\
                 wiki*' -- '*) printf '%s\\n' '{page}' ;;\n\
                 wiki*) printf '%s\\n' '{pages}' ;;\n\
                 search*)\n\
                   if [ \"$last\" = flush ]; then printf '%s\\n' '{hits}'\n\
                   else printf '%s\\n' '{{\"hits\":[]}}'; exit 1; fi ;;\n\
                 *) exit 1 ;;\n\
                 esac",
                log = log.display(),
                page = page.to_string().replace('\'', "'\\''"),
                pages = pages.to_string().replace('\'', "'\\''"),
            ),
        );
        let hub = Hub::spawn(&home, &[&bins], &["--port", "0"]);
        // The doorbell's first round reads mem too; the spawns counted are
        // the request's own.
        wait_for(
            "the doorbell's first round",
            Duration::from_secs(10),
            || {
                invocations(&log)
                    .iter()
                    .any(|argv| argv.first().map(String::as_str) == Some("projects"))
            },
        );
        // One page through mem's gate, so the round's projects read has finished
        // and filled the cache before the counted request.
        hub.get("/");
        World {
            _dir: dir,
            hub,
            log,
        }
    }

    /// The body of `path`, and the argv of every spawn the request made
    /// besides the doorbell's question poll.
    fn get(&self, path: &str) -> (String, Vec<Vec<String>>) {
        let before = invocations(&self.log).len();
        let response = self.hub.get(path);
        assert_eq!(status_of(&response), 200, "{path}: {response}");
        let spawns = invocations(&self.log)[before..]
            .iter()
            .filter(|argv| argv.first().map(String::as_str) != Some("questions"))
            .cloned()
            .collect();
        (body_of(&response).to_string(), spawns)
    }
}

fn search_path(q: &str) -> String {
    format!("/p/{PROJECT}/wiki?q={}", encode_component(q))
}

#[test]
fn the_page_has_a_search_form_and_lists_pages_index_first() {
    let world = World::new("wiki-search-list");

    let (body, spawns) = world.get(&format!("/p/{PROJECT}/wiki"));

    assert!(
        body.contains("<form class=\"search\" method=\"get\""),
        "{body}"
    );
    assert!(body.contains("name=\"q\""), "{body}");
    let index = body.find("href=\"/wiki/gamma/index\"").expect(&body);
    let storage = body.find("href=\"/wiki/gamma/storage\"").expect(&body);
    assert!(index < storage, "index comes first: {body}");
    assert!(body.contains("Gamma&#39;s wiki"), "{body}");
    assert!(body.contains("How storage works"), "{body}");
    assert!(spawns.len() <= 3, "a page costs three spawns: {spawns:?}");
}

#[test]
fn a_section_hit_links_to_its_heading_on_the_page() {
    let world = World::new("wiki-search-section");

    let (body, spawns) = world.get(&search_path("flush"));

    let link = "href=\"/wiki/gamma/storage#how-it-flushes\"";
    assert!(body.contains(link), "{body}");
    assert!(body.contains("How storage works"), "{body}");
    assert!(body.contains("How it flushes"), "{body}");
    // The snippet is text mem cut from a page, so it is escaped like one.
    assert!(body.contains("The [flush] is &lt;atomic&gt;"), "{body}");
    assert!(spawns.len() <= 3, "a search costs three spawns: {spawns:?}");

    // The link lands: the page it names gives that heading the id.
    let (page, _) = world.get("/wiki/gamma/storage");
    assert!(
        page.contains("<h2 id=\"how-it-flushes\">How it flushes</h2>"),
        "{page}"
    );
}

#[test]
fn an_item_hit_links_to_the_item() {
    let world = World::new("wiki-search-item");

    let (body, _) = world.get(&search_path("flush"));

    assert!(
        body.contains(
            "<a href=\"/p/gamma/item/01M40JS4E731V1CG1P8H1JYAN3\">Flush on every write</a>"
        ),
        "{body}"
    );
}

#[test]
fn a_search_with_no_hit_says_so() {
    let world = World::new("wiki-search-none");

    let (body, _) = world.get(&search_path("nothing-matches"));

    assert!(body.contains("Nothing matches"), "{body}");
    assert!(body.contains("nothing-matches"), "{body}");
    assert!(!body.contains("#how-it-flushes"), "{body}");
}

#[test]
fn a_query_of_shell_characters_is_one_argument_after_the_double_dash() {
    let world = World::new("wiki-search-shell");
    let q = "a\"; $(touch x) `id` | && 'b' $HOME * <c>";

    let (body, spawns) = world.get(&search_path(&format!("  {q}  ")));

    let search: Vec<&Vec<String>> = spawns
        .iter()
        .filter(|argv| argv.first().map(String::as_str) == Some("search"))
        .collect();
    assert_eq!(
        search,
        [&vec![
            "search".to_string(),
            "--project=gamma".to_string(),
            "--limit".to_string(),
            "20".to_string(),
            "--json".to_string(),
            "--".to_string(),
            q.to_string(),
        ]],
        "trimmed, whole and last: {spawns:?}"
    );
    // Echoed back into the field and the no-hit line, escaped both times.
    assert!(!body.contains("<c>"), "{body}");
    assert!(body.contains("&lt;c&gt;"), "{body}");
}

#[test]
fn a_long_query_is_cut_to_200_bytes_on_a_character_boundary() {
    let world = World::new("wiki-search-long");
    // 199 bytes of `a`, then a two-byte character that would straddle 200.
    let q = format!("{}é{}", "a".repeat(199), "b".repeat(100));

    let (_, spawns) = world.get(&search_path(&q));

    let last = spawns
        .iter()
        .find(|argv| argv.first().map(String::as_str) == Some("search"))
        .and_then(|argv| argv.last())
        .expect("a search ran");
    assert_eq!(last, &"a".repeat(199));
}

#[test]
fn the_contents_list_links_every_heading_to_its_anchor() {
    let world = World::new("wiki-search-contents");

    let (body, spawns) = world.get("/wiki/gamma/storage");

    for (heading, hslug) in &SECTIONS[1..] {
        let heading = heading.replace('\'', "&#39;");
        assert!(
            body.contains(&format!("<a href=\"#{hslug}\">{heading}</a>")),
            "{hslug} in the contents: {body}"
        );
        assert!(
            body.contains(&format!("<h2 id=\"{hslug}\">")),
            "{hslug} on a heading: {body}"
        );
    }
    // A repeated heading gets each id once, in order.
    let first = body
        .find("<h2 id=\"notes\">Notes</h2>\n<p>one")
        .expect(&body);
    let second = body
        .find("<h2 id=\"notes-2\">Notes</h2>\n<p>two")
        .expect(&body);
    assert!(first < second, "{body}");
    // The preamble is no heading, so it is no row.
    assert!(!body.contains("href=\"#top\""), "{body}");
    assert!(spawns.len() <= 3, "a page costs three spawns: {spawns:?}");
}

#[test]
fn a_page_sits_beside_its_contents_with_a_search_box_above() {
    let world = World::new("wiki-search-docs");

    let (body, spawns) = world.get("/wiki/gamma/storage");

    let search = body
        .find(
            "<form class=\"search\" method=\"get\" action=\"/p/gamma/wiki\">\n\
             <input type=\"search\" name=\"q\" value=\"\" placeholder=\"search gamma\" \
             aria-label=\"search\">",
        )
        .expect(&body);
    let docs = body
        .find("<div class=\"docs\">\n<nav class=\"contents\">")
        .expect(&body);
    let article = body.find("<article class=\"md\">").expect(&body);
    assert!(search < docs && docs < article, "{body}");
    assert!(body.contains("</article>\n</div>\n"), "{body}");
    assert!(spawns.len() <= 3, "the box costs no spawn: {spawns:?}");
}

#[test]
fn the_wiki_list_uses_the_same_search_box_and_keeps_the_query() {
    let world = World::new("wiki-search-box");

    let (body, _) = world.get(&search_path("flush"));

    assert!(
        body.contains(
            "<form class=\"search\" method=\"get\" action=\"/p/gamma/wiki\">\n\
             <input type=\"search\" name=\"q\" value=\"flush\" placeholder=\"search gamma\" \
             aria-label=\"search\">"
        ),
        "{body}"
    );
}

#[test]
fn a_page_marks_wiki_as_the_section_being_read() {
    let world = World::new("wiki-search-current");

    let (body, _) = world.get("/wiki/gamma/storage");

    assert!(
        body.contains("<a href=\"/p/gamma/wiki\" aria-current=\"page\">wiki</a>"),
        "{body}"
    );
    assert!(
        body.contains("<title>hub — gamma / storage</title>"),
        "{body}"
    );
}
