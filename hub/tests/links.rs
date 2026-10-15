//! A question's text is escaped on every page that shows it, with its
//! http(s) URLs as links, and every link that leaves the hub opens in a new
//! tab.

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use common::{Hub, TempDir, body_of, mem_in, real_mem, recording_mem, seed_project, status_of};

const TAGGED: &str = "<b>x</b> https://example.com/a?b=1";
const QUOTED: &str = "https://e.com/\"onmouseover=alert(1)";

/// Writes one wiki page through the real `mem wiki <slug> --stdin`.
fn wiki_page(mem: &Path, home: &Path, project: &str, slug: &str, text: &str) {
    let child = Command::new(mem)
        .args(["wiki", slug, "--stdin", "--note", "seeding a page"])
        .current_dir(home.join(project))
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("MEM_SYNC_CMD", "true")
        .env("MEM_NOTIFY_CMD", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run mem wiki");
    child
        .stdin
        .as_ref()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "writing {slug}: {out:?}");
}

/// A store with project `gamma`, its two questions and a wiki page with an
/// outside link and an in-hub one, and a hub with one sibling.
fn world(tag: &str) -> (TempDir, Hub) {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (bin, _log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, "gamma", "gamma did a thing");
    for body in [TAGGED, QUOTED] {
        let out = mem_in(
            &mem,
            &home,
            &home.join("gamma"),
            &["ask", "--for", "human", body],
        );
        assert!(out.status.success(), "{out:?}");
    }
    wiki_page(
        &mem,
        &home,
        "gamma",
        "notes",
        "# Notes\n\nSee [the docs](https://example.org/docs) and [the index](index.md).\n",
    );
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n\
         siblings = [\"http://nuc:8088\"]\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &home,
        &[&bin],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );
    (dir, hub)
}

fn page(hub: &Hub, path: &str) -> String {
    let response = hub.get(path);
    assert_eq!(status_of(&response), 200, "{path}: {response}");
    body_of(&response).to_string()
}

/// Every tag on the page, the text between its `<` and `>`.
fn tags(body: &str) -> Vec<&str> {
    body.match_indices('<')
        .filter_map(|(at, _)| body[at..].find('>').map(|end| &body[at..at + end + 1]))
        .collect()
}

#[test]
fn a_questions_text_is_escaped_with_its_urls_linked_on_home_and_its_page() {
    let (_dir, hub) = world("links-questions");
    for path in ["/", "/p/gamma/questions"] {
        let body = page(&hub, path);
        assert!(
            body.contains(
                "&lt;b&gt;x&lt;/b&gt; <a href=\"https://example.com/a?b=1\" \
                 target=\"_blank\" rel=\"noopener\">https://example.com/a?b=1</a>"
            ),
            "{path}: {body}"
        );
        // The URL stops at the quote, and the rest is text.
        assert!(
            body.contains(
                "<a href=\"https://e.com/\" target=\"_blank\" rel=\"noopener\">\
                 https://e.com/</a>&quot;onmouseover=alert(1)"
            ),
            "{path}: {body}"
        );
        for tag in tags(&body) {
            assert!(!tag.contains("onmouseover"), "{path}: {tag}");
        }
    }
}

#[test]
fn every_link_that_leaves_the_hub_opens_in_a_new_tab() {
    let (_dir, hub) = world("links-outside");
    for path in ["/", "/p/gamma/questions", "/subscribe", "/wiki/gamma/notes"] {
        let body = page(&hub, path);
        let outside: Vec<&str> = tags(&body)
            .into_iter()
            .filter(|tag| tag.starts_with("<a ") && tag.contains("href=\"http"))
            .collect();
        assert!(!outside.is_empty(), "{path}: no outside link in {body}");
        for tag in outside {
            assert!(
                tag.contains(" target=\"_blank\"") && tag.contains(" rel=\"noopener\""),
                "{path}: {tag}"
            );
        }
    }
    // A link to another page of the wiki stays in the tab.
    let body = page(&hub, "/wiki/gamma/notes");
    assert!(
        body.contains("<a href=\"/wiki/gamma/index\">the index</a>"),
        "{body}"
    );
}
