//! The finding form on `GET /p/<project>/new`, and `POST
//! /p/<project>/new/finding`, which files a finding with its photo through a
//! recording `mem` over the real binary, then reads the store back with that
//! binary.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{
    DEVICE_COOKIE, Hub, TempDir, body_of, header_of, invocations, mem_in, real_mem, recording_mem,
    seed_project, status_of,
};
use serde_json::Value;

const PROJECT: &str = "gamma";

const ROADMAP: &str = "# roadmap: gamma\n\n\
                       - [x] g1-log Log a habit\n\
                       - [ ] g2-nag Nag at nine\n\
                       - [ ] g3-week Show the week\n";

/// A PNG signature followed by bytes that are not UTF-8, and a CRLF and two
/// dashes that are not a boundary, so only a body parsed as bytes comes back
/// equal.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\xff\xfe\r\n--\x00\x80\xc3\x28";

const GIF: &[u8] = b"GIF89a\x01\0\x01\0\x80\0\0\xff\xff\xff";

const BOUNDARY: &str = "----hubphotoQ7MA4YWxkTrZu0gW";

/// A store with one project and, when given, its roadmap, and a hub whose
/// `mem` records every argv before running the real binary. The hub comes
/// first so it is killed before its store is removed.
struct World {
    hub: Hub,
    _dir: TempDir,
    home: PathBuf,
    log: PathBuf,
    mem: PathBuf,
}

impl World {
    fn new(tag: &str, roadmap: Option<&str>) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        let (bin, log) = recording_mem(dir.path(), &home);
        let mem = real_mem().unwrap();
        seed_project(&mem, &home, PROJECT, "gamma began");
        if let Some(text) = roadmap {
            let file = dir.join("roadmap.md");
            std::fs::write(&file, text).unwrap();
            let out = mem_in(
                &mem,
                &home,
                &home.join(PROJECT),
                &["roadmap", "--set-file", file.to_str().unwrap()],
            );
            assert!(out.status.success(), "writing the roadmap: {out:?}");
        }
        let hub = Hub::spawn(&home, &[&bin], &["--port", "0"]);
        World {
            hub,
            _dir: dir,
            home,
            log,
            mem,
        }
    }

    fn page(&self) -> String {
        let response = self.hub.get(&format!("/p/{PROJECT}/new"));
        assert_eq!(status_of(&response), 200, "{response}");
        body_of(&response).to_string()
    }

    /// A same-origin `multipart/form-data` post of `parts`, each a name, a
    /// filename for a file part, and its bytes.
    fn post(&self, parts: &[(&str, Option<&str>, &[u8])]) -> String {
        let mut body = Vec::new();
        for (name, filename, data) in parts {
            body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
            let disposition = match filename {
                Some(filename) => format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                     Content-Type: application/octet-stream\r\n"
                ),
                None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n"),
            };
            body.extend_from_slice(disposition.as_bytes());
            body.extend_from_slice(b"\r\n");
            body.extend_from_slice(data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        let mut request = format!(
            "POST /p/{PROJECT}/new/finding HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             Connection: close\r\nOrigin: {origin}\r\nCookie: {DEVICE_COOKIE}\r\n\
             Content-Type: multipart/form-data; boundary={BOUNDARY}\r\n\
             Content-Length: {length}\r\n\r\n",
            port = self.hub.port,
            origin = self.hub.origin(),
            length = body.len(),
        )
        .into_bytes();
        request.extend_from_slice(&body);
        let response = self.hub.raw_bytes(&request);
        println!("POST finding -> {}", status_of(&response));
        response
    }

    /// Every `finding add` the hub ran, printed so the transcript shows each
    /// argv.
    fn finding_adds(&self) -> Vec<Vec<String>> {
        let adds: Vec<Vec<String>> = invocations(&self.log)
            .into_iter()
            .filter(|argv| argv.len() > 1 && argv[0] == "finding" && argv[1] == "add")
            .collect();
        for argv in &adds {
            println!("  mem {argv:?}");
        }
        adds
    }

    fn uploads_dir(&self) -> PathBuf {
        self.home.join("state/hub/uploads")
    }

    /// What the uploads directory holds; a directory never made holds nothing.
    fn uploads(&self) -> Vec<String> {
        match std::fs::read_dir(self.uploads_dir()) {
            Ok(entries) => entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The project's findings once there are `count` of them. mem serves a
    /// read from the index as it was while another process holds the reindex
    /// lock, so the first read after a write may not show it yet.
    fn findings(&self, count: usize) -> Vec<Value> {
        let flag = format!("--project={PROJECT}");
        let args = ["finding", "list", &flag, "--json"];
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let out = mem_in(&self.mem, &self.home, &self.home, &args);
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
            if let Some(items) = doc["items"].as_array()
                && items.len() == count
            {
                return items.clone();
            }
            assert!(Instant::now() < deadline, "mem {args:?}: {out:?}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn evidence_cat(&self, id: &str) -> Vec<u8> {
        let flag = format!("--project={PROJECT}");
        let out = mem_in(
            &self.mem,
            &self.home,
            &self.home,
            &["evidence", "cat", &flag, "--", id],
        );
        assert!(out.status.success(), "evidence cat {id}: {out:?}");
        out.stdout
    }
}

fn see_evidence(response: &str) {
    assert_eq!(status_of(response), 303, "{response}");
    assert_eq!(
        header_of(response, "Location"),
        Some(format!("/p/{PROJECT}/evidence").as_str()),
        "{response}"
    );
}

#[test]
fn the_form_offers_each_milestone_with_the_first_open_one_chosen() {
    let world = World::new("photo-form", Some(ROADMAP));
    let body = world.page();
    for needle in [
        "<form method=\"post\" action=\"/p/gamma/new/finding\" \
         enctype=\"multipart/form-data\">",
        "<option value=\"g1-log\">g1-log</option>",
        "<option value=\"g2-nag\" selected>g2-nag</option>",
        "<option value=\"g3-week\">g3-week</option>",
        "<input type=\"number\" name=\"step\" value=\"0\" min=\"0\" max=\"999\"",
        "<textarea name=\"text\"",
        "<input type=\"file\" name=\"photo\" accept=\"image/jpeg,image/png,image/webp\" \
         capture=\"environment\">",
    ] {
        assert!(body.contains(needle), "{needle} in {body}");
    }
}

#[test]
fn with_every_milestone_ticked_the_last_is_chosen() {
    let world = World::new("photo-all-ticked", Some(&ROADMAP.replace("- [ ]", "- [x]")));
    let body = world.page();
    assert!(
        body.contains("<option value=\"g1-log\">g1-log</option>"),
        "{body}"
    );
    assert!(
        body.contains("<option value=\"g3-week\" selected>g3-week</option>"),
        "{body}"
    );
}

#[test]
fn with_no_roadmap_the_form_gives_way_to_a_line() {
    let world = World::new("photo-no-roadmap", None);
    let body = world.page();
    assert!(
        body.contains(
            "<p class=\"empty\">No roadmap yet, so no milestone to file a finding against.</p>"
        ),
        "{body}"
    );
    assert!(!body.contains("name=\"photo\""), "{body}");
}

#[test]
fn a_png_is_filed_as_a_finding_whose_file_reads_back_byte_for_byte() {
    let world = World::new("photo-png", Some(ROADMAP));
    let response = world.post(&[
        ("milestone", None, b"g2-nag"),
        ("step", None, b"2"),
        ("text", None, b"the nag fires twice at nine"),
        ("photo", Some("IMG_0042.png"), PNG),
    ]);
    see_evidence(&response);

    let adds = world.finding_adds();
    assert_eq!(adds.len(), 1);
    let argv = &adds[0];
    let upload = PathBuf::from(&argv[8]);
    assert_eq!(upload.parent(), Some(world.uploads_dir().as_path()));
    let name = upload.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("photo-") && name.ends_with(".png"),
        "{name}"
    );
    assert_eq!(
        argv,
        &[
            "finding",
            "add",
            "--project=gamma",
            "--milestone",
            "g2-nag",
            "--step",
            "2",
            "--evidence",
            &argv[8],
            "--",
            "the nag fires twice at nine",
        ]
    );
    assert!(world.uploads().is_empty(), "{:?}", world.uploads());

    let findings = world.findings(1);
    assert_eq!(findings[0]["milestone"], "g2-nag");
    assert_eq!(findings[0]["step"], "2");
    assert_eq!(findings[0]["file"], format!("evidence/g2-nag/{name}"));
    assert_eq!(world.evidence_cat(findings[0]["id"].as_str().unwrap()), PNG);
}

#[test]
fn with_no_photo_the_finding_is_filed_without_evidence() {
    let world = World::new("photo-none", Some(ROADMAP));
    // A browser sends the file part with no filename and no bytes when no
    // photo was chosen.
    let response = world.post(&[
        ("milestone", None, b"g1-log"),
        ("step", None, b"0"),
        ("text", None, b"the streak resets at noon"),
        ("photo", Some(""), b""),
    ]);
    see_evidence(&response);
    assert_eq!(
        world.finding_adds(),
        [[
            "finding",
            "add",
            "--project=gamma",
            "--milestone",
            "g1-log",
            "--step",
            "0",
            "--",
            "the streak resets at noon",
        ]]
    );
    assert!(world.uploads().is_empty(), "{:?}", world.uploads());
    let findings = world.findings(1);
    assert_eq!(findings[0]["file"], Value::Null);
}

#[test]
fn a_gif_is_refused_and_nothing_is_filed() {
    let world = World::new("photo-gif", Some(ROADMAP));
    let response = world.post(&[
        ("milestone", None, b"g2-nag"),
        ("step", None, b"1"),
        ("text", None, b"the nag fires twice"),
        ("photo", Some("nag.gif"), GIF),
    ]);
    assert_eq!(status_of(&response), 400, "{response}");
    assert!(world.finding_adds().is_empty());
    assert!(world.uploads().is_empty(), "{:?}", world.uploads());
}

#[test]
fn the_clients_filename_is_never_read() {
    let world = World::new("photo-name", Some(ROADMAP));
    let response = world.post(&[
        ("milestone", None, b"g3-week"),
        ("step", None, b"3"),
        ("text", None, b"the week starts on sunday"),
        ("photo", Some("../x.png"), PNG),
    ]);
    see_evidence(&response);
    assert!(!world.home.join("state/hub/x.png").exists());
    assert!(world.uploads().is_empty(), "{:?}", world.uploads());

    let findings = world.findings(1);
    let file = findings[0]["file"].as_str().unwrap();
    assert!(file.starts_with("evidence/g3-week/photo-"), "{file}");
    assert_eq!(world.evidence_cat(findings[0]["id"].as_str().unwrap()), PNG);
}

#[test]
fn an_unknown_milestone_a_bad_step_and_an_empty_text_are_refused() {
    let world = World::new("photo-refused", Some(ROADMAP));
    let refused: [(&[u8], &[u8], &[u8]); 5] = [
        (b"g9-none", b"1", b"the nag fires twice"),
        (b"g2-nag", b"", b"the nag fires twice"),
        (b"g2-nag", b"1234", b"the nag fires twice"),
        (b"g2-nag", b"-1", b"the nag fires twice"),
        (b"g2-nag", b"1", b"  \r\n "),
    ];
    for (milestone, step, text) in refused {
        let response = world.post(&[
            ("milestone", None, milestone),
            ("step", None, step),
            ("text", None, text),
            ("photo", Some("nag.png"), PNG),
        ]);
        assert_eq!(status_of(&response), 400, "{response}");
        assert!(world.uploads().is_empty(), "{:?}", world.uploads());
    }
    assert!(world.finding_adds().is_empty());
}
