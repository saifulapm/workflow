//! `GET /p/<project>/file/<id>`, an evidence file's bytes through the real
//! `mem`, and `GET /p/<project>/evidence`, the gallery and the findings over
//! a fake `mem` printing recorded rows.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{
    Hub, TempDir, body_of, fixture_bin, invocations, mem_in, real_mem, recording_mem, seed_project,
    status_of, wait_for,
};
use hub::page_evidence::image_kind;
use serde_json::{Value, json};

const PROJECT: &str = "gamma";

/// A PNG signature followed by bytes that are not UTF-8, so only a body sent
/// as bytes comes back equal.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\xff\xfe\x00\x80\xc3\x28";

#[test]
fn image_kind_reads_the_leading_bytes() {
    assert_eq!(image_kind(PNG), Some("image/png"));
    assert_eq!(
        image_kind(b"\xff\xd8\xff\xe0\0\x10JFIF"),
        Some("image/jpeg")
    );
    assert_eq!(image_kind(b"RIFF\x24\0\0\0WEBPVP8 "), Some("image/webp"));
    assert_eq!(image_kind(b"RIFF\x24\0\0\0WAVEfmt "), None);
    assert_eq!(image_kind(b"hello\n"), None);
    assert_eq!(image_kind(b""), None);
}

/// A throwaway store with one project, a PNG and a text file filed as
/// evidence, and a ruling, and the hub over it.
struct Store {
    _dir: TempDir,
    log: PathBuf,
    hub: Hub,
    png: String,
    text: String,
    ruling: String,
}

fn store() -> Store {
    let dir = TempDir::new("evidence-file");
    let home = dir.join("home");
    let (bin, log) = recording_mem(dir.path(), &home);
    let mem = real_mem().unwrap();
    seed_project(&mem, &home, PROJECT, "gamma began");
    let repo = home.join(PROJECT);
    std::fs::write(repo.join("week.png"), PNG).unwrap();
    std::fs::write(repo.join("notes.txt"), "the nag fires twice\n").unwrap();
    let filed = |args: &[&str]| -> String {
        let out = mem_in(&mem, &home, &repo, args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        value["id"].as_str().unwrap().to_string()
    };
    let png = filed(&[
        "evidence", "add", "--task", "g2-t1", "week.png", "--note", "the week", "--json",
    ]);
    let text = filed(&[
        "evidence",
        "add",
        "--task",
        "g2-t1",
        "notes.txt",
        "--note",
        "notes",
        "--json",
    ]);
    let ruling = filed(&["decide", "keep sqlite", "--by", "saiful", "--json"]);
    let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
    Store {
        _dir: dir,
        log,
        hub,
        png,
        text,
        ruling,
    }
}

/// The status, the headers and the body exactly as sent, which `Hub::get`
/// would pass through a lossy UTF-8 decode.
fn get_bytes(hub: &Hub, path: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        hub.port
    )
    .unwrap();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out);
    let at = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("end of headers");
    let head = String::from_utf8_lossy(&out[..at]).into_owned();
    (status_of(&head), head, out[at + 4..].to_vec())
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn cats(log: &Path) -> usize {
    invocations(log)
        .iter()
        .filter(|argv| argv.first().map(String::as_str) == Some("evidence") && argv[1] == "cat")
        .count()
}

#[test]
fn a_png_comes_back_byte_for_byte_as_an_image() {
    let store = store();
    let (status, head, body) = get_bytes(&store.hub, &format!("/p/{PROJECT}/file/{}", store.png));
    assert_eq!(status, 200, "{head}");
    assert_eq!(body, PNG);
    assert_eq!(header(&head, "Content-Type"), Some("image/png"), "{head}");
    assert_eq!(
        header(&head, "Cache-Control"),
        Some("private, max-age=3600"),
        "{head}"
    );
    assert_eq!(
        header(&head, "X-Content-Type-Options"),
        Some("nosniff"),
        "{head}"
    );
    let argv = invocations(&store.log)
        .into_iter()
        .find(|argv| argv.get(1).map(String::as_str) == Some("cat"))
        .unwrap();
    assert_eq!(
        argv,
        [
            "evidence".to_string(),
            "cat".to_string(),
            format!("--project={PROJECT}"),
            "--".to_string(),
            store.png.clone(),
        ]
    );
}

#[test]
fn a_text_file_comes_back_as_text_and_is_not_cached_in_the_hub() {
    let store = store();
    let path = format!("/p/{PROJECT}/file/{}", store.text);
    let (status, head, body) = get_bytes(&store.hub, &path);
    assert_eq!(status, 200, "{head}");
    assert_eq!(body, b"the nag fires twice\n");
    assert_eq!(
        header(&head, "Content-Type"),
        Some("text/plain; charset=utf-8"),
        "{head}"
    );
    get_bytes(&store.hub, &path);
    assert_eq!(cats(&store.log), 2, "each request runs mem");
}

#[test]
fn a_ruling_is_no_evidence_and_gets_404() {
    let store = store();
    let (status, head, _) = get_bytes(&store.hub, &format!("/p/{PROJECT}/file/{}", store.ruling));
    assert_eq!(status, 404, "{head}");
    assert_eq!(cats(&store.log), 1);
}

#[test]
fn a_malformed_id_gets_404_before_mem_runs() {
    let store = store();
    for id in ["..%2F..%2Fetc%2Fpasswd", "not-an-id", "ABC", "--help"] {
        let (status, head, _) = get_bytes(&store.hub, &format!("/p/{PROJECT}/file/{id}"));
        assert_eq!(status, 404, "{id}: {head}");
    }
    assert_eq!(cats(&store.log), 0);
}

/// Thirty evidence rows, newest first as mem lists them: ten for each of
/// three tasks, every even row a PNG and every odd one a text file.
fn evidence_doc() -> Value {
    let rows: Vec<Value> = (0..30)
        .map(|i| {
            let task = ["g2-t3", "g2-t2", "g2-t1"][i / 10];
            let ext = if i % 2 == 0 { "png" } else { "txt" };
            json!({
                "id": row_id(i), "short_id": format!("SHORT{i:03}"), "kind": "evidence",
                "task": task, "file": format!("evidence/{task}/shot-{i:02}.{ext}"),
                "note": format!("note {i:02}"), "title": format!("note {i:02}"),
                "created": "2026-10-04",
            })
        })
        .collect();
    json!({ "items": rows })
}

fn row_id(i: usize) -> String {
    format!("01M4560000000000000000{i:04}")
}

/// Newest first: the fixed finding was filed after the open one.
fn finding_doc() -> Value {
    json!({"items": [
        {"id": "01M45600000000000000000F1X", "kind": "finding", "status": "fixed",
         "milestone": "g1-log", "step": "1", "fixed_by": "abc1234",
         "file": "evidence/g1-log/midnight.txt",
         "body": "\na habit ticked at midnight lands on the wrong day\n",
         "title": "a habit ticked at midnight lands on the wrong day"},
        {"id": "01M45600000000000000000PEN", "kind": "finding", "status": "open",
         "milestone": "g2-nag", "step": "2", "fixed_by": null,
         "file": "evidence/g2-nag/twice.png",
         "body": "\nthe nag fires twice at nine\n",
         "title": "the nag fires twice at nine"},
    ]})
}

/// The hub over a fake `mem` that logs its argv: the body of `path`, and
/// the spawns that request made besides the doorbell's question poll.
fn page(path: &str) -> (String, Vec<String>) {
    page_over(path, &evidence_doc(), &finding_doc())
}

/// `page` over the given evidence and finding documents.
fn page_over(path: &str, evidence: &Value, findings: &Value) -> (String, Vec<String>) {
    let dir = TempDir::new("evidence-page");
    let home = dir.join("home");
    let bins = dir.join("bin");
    let log = dir.join("mem.log");
    let projects = json!({"projects": [{
        "id": "p-gamma", "name": PROJECT, "remote": null, "aliases": [],
        "created": "2026-01-01", "items": 3, "current": false, "checkouts": [],
    }]});
    fixture_bin(
        &bins,
        "mem",
        &format!(
            "printf '%s\\n' \"$*\" >> '{log}'\n\
             case \"$1\" in\n\
             projects) printf '%s\\n' '{projects}' ;;\n\
             log) printf '%s\\n' '{{\"items\":[]}}' ;;\n\
             questions) printf '%s\\n' '{{\"questions\":[]}}' ;;\n\
             evidence) printf '%s\\n' '{evidence}' ;;\n\
             finding) printf '%s\\n' '{findings}' ;;\n\
             *) exit 1 ;;\n\
             esac",
            log = log.display(),
        ),
    );
    let hub = Hub::spawn(&home, &[&bins], &["--port", "0"]);
    wait_for(
        "the doorbell's first round",
        Duration::from_secs(10),
        || lines(&log).iter().any(|argv| argv.starts_with("projects ")),
    );
    // One page through mem's gate, so the round's projects read has finished
    // and filled the cache before the counted request.
    hub.get("/");
    let before = lines(&log).len();
    let response = hub.get(path);
    assert_eq!(status_of(&response), 200, "{response}");
    let spawns = lines(&log)[before..]
        .iter()
        .filter(|argv| !argv.starts_with("questions"))
        .cloned()
        .collect();
    (body_of(&response).to_string(), spawns)
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn position(body: &str, needle: &str) -> usize {
    body.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} missing from {body}"))
}

#[test]
fn the_first_page_holds_the_newest_24_rows_grouped_by_task() {
    let (body, spawns) = page(&format!("/p/{PROJECT}/evidence"));

    let groups =
        ["g2-t3", "g2-t2", "g2-t1"].map(|task| position(&body, &format!("<h3>{task}</h3>")));
    assert!(
        groups.is_sorted(),
        "groups out of order: {groups:?} in {body}"
    );
    for i in 0..24 {
        assert!(body.contains(&format!("note {i:02}")), "row {i}: {body}");
    }
    assert!(!body.contains("note 24"), "row 24 is on page 2: {body}");
    assert!(body.contains("?page=2"), "a link to page 2: {body}");

    assert!(
        body.contains(&format!(
            "<a href=\"#view-{id}\"><img src=\"/p/{PROJECT}/file/{id}\" alt=\"evidence/g2-t3/shot-00.png\" loading=\"lazy\"></a>",
            id = row_id(0)
        )),
        "an image row's tile: {body}"
    );
    assert!(
        body.contains(&format!(
            "<a href=\"/p/{PROJECT}/file/{}\">evidence/g2-t3/shot-01.txt</a>",
            row_id(1)
        )),
        "a text row: {body}"
    );

    let open = position(&body, "the nag fires twice at nine");
    let fixed = position(&body, "a habit ticked at midnight lands on the wrong day");
    assert!(open < fixed, "open before fixed: {body}");
    for needle in [
        "g2-nag", "step 2", "g1-log", "step 1", "abc1234", "open", "fixed",
    ] {
        assert!(body.contains(needle), "{needle}: {body}");
    }
    assert!(
        body.contains(&format!(
            "<a href=\"#view-01M45600000000000000000PEN\"><img src=\"/p/{PROJECT}/file/01M45600000000000000000PEN\" alt=\"evidence/g2-nag/twice.png\" loading=\"lazy\"></a>"
        )),
        "the open finding's tile: {body}"
    );
    assert!(
        body.contains("evidence/g1-log/midnight.txt</a>"),
        "the fixed finding's file: {body}"
    );

    let evidence = format!("evidence list --project={PROJECT} --json");
    let findings = format!("finding list --project={PROJECT} --json");
    assert_eq!(
        spawns.iter().filter(|a| **a == evidence).count(),
        1,
        "{spawns:?}"
    );
    assert_eq!(
        spawns.iter().filter(|a| **a == findings).count(),
        1,
        "{spawns:?}"
    );
    assert!(spawns.len() <= 3, "three spawns at most: {spawns:?}");
}

#[test]
fn the_second_page_holds_the_last_six_rows() {
    let (body, _) = page(&format!("/p/{PROJECT}/evidence?page=2"));
    for i in 24..30 {
        assert!(body.contains(&format!("note {i:02}")), "row {i}: {body}");
    }
    assert!(!body.contains("note 23"), "row 23 is on page 1: {body}");
    assert!(!body.contains("<h3>g2-t3</h3>"), "{body}");
    assert!(body.contains("<h3>g2-t1</h3>"), "{body}");
    assert!(body.contains("?page=1"), "a link back: {body}");
    assert!(
        body.contains("the nag fires twice at nine"),
        "findings on every page: {body}"
    );
}

/// One image row and one image finding, each carrying markup in its text.
fn hostile_docs() -> (Value, Value) {
    let evidence = json!({"items": [{
        "id": "01M4560000000000000000EVID", "kind": "evidence", "task": "g3-t1",
        "file": "evidence/g3-t1/shot.png", "note": "<script>alert(1)</script> seen",
    }]});
    let findings = json!({"items": [{
        "id": "01M45600000000000000000FND", "kind": "finding", "status": "open",
        "milestone": "g3-view", "step": "4", "fixed_by": null,
        "file": "evidence/g3-view/bad.png", "body": "\n<script>alert(2)</script> shown\n",
    }]});
    (evidence, findings)
}

#[test]
fn an_image_row_is_a_tile_that_opens_full_size_in_a_lightbox() {
    let (evidence, findings) = hostile_docs();
    let (body, _) = page_over(&format!("/p/{PROJECT}/evidence"), &evidence, &findings);
    let id = "01M4560000000000000000EVID";
    let src = format!("/p/{PROJECT}/file/{id}");

    let task = position(&body, "<h3>g3-t1</h3>\n<ul class=\"gallery\">");
    let tile = position(
        &body,
        &format!(
            "<li><figure><a href=\"#view-{id}\"><img src=\"{src}\" alt=\"evidence/g3-t1/shot.png\" loading=\"lazy\"></a><figcaption>&lt;script&gt;alert(1)&lt;/script&gt; seen</figcaption></figure>"
        ),
    );
    let lightbox = position(
        &body,
        &format!(
            "<div class=\"lightbox\" id=\"view-{id}\"><a class=\"close\" href=\"#\">close</a><img src=\"{src}\" alt=\"evidence/g3-t1/shot.png\" loading=\"lazy\"><p>g3-t1 · &lt;script&gt;alert(1)&lt;/script&gt; seen</p><p><a href=\"{src}\">open the file</a></p></div></li>"
        ),
    );
    assert!(task < tile && tile < lightbox, "{body}");
}

#[test]
fn an_image_finding_is_a_tile_with_its_milestone_step_and_body() {
    let (evidence, findings) = hostile_docs();
    let (body, _) = page_over(&format!("/p/{PROJECT}/evidence"), &evidence, &findings);
    let id = "01M45600000000000000000FND";
    let src = format!("/p/{PROJECT}/file/{id}");

    let article = position(&body, "<article>");
    let tile = position(
        &body,
        &format!(
            "<ul class=\"gallery\"><li><figure><a href=\"#view-{id}\"><img src=\"{src}\" alt=\"evidence/g3-view/bad.png\" loading=\"lazy\"></a><figcaption>evidence/g3-view/bad.png</figcaption></figure>"
        ),
    );
    let lightbox = position(
        &body,
        &format!(
            "<div class=\"lightbox\" id=\"view-{id}\"><a class=\"close\" href=\"#\">close</a><img src=\"{src}\" alt=\"evidence/g3-view/bad.png\" loading=\"lazy\"><p>g3-view · step 4</p><p>&lt;script&gt;alert(2)&lt;/script&gt; shown</p><p><a href=\"{src}\">open the file</a></p></div></li></ul>"
        ),
    );
    assert!(article < tile && tile < lightbox, "{body}");
    assert!(!body.contains("<script>"), "markup stays escaped: {body}");
}
