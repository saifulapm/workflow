//! H5 — the dashboard, the answer path, and §9 in full.
//!
//! AC9's three tests are `a_foreign_origin_is_refused_and_runs_no_mem_answer`,
//! `a_question_that_is_script_renders_as_text` and
//! `an_answer_that_is_a_shell_command_leaves_no_file`.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use common::{
    Hub, TempDir, body_of, header_of, invocations, mem_in, real_mem, recording_mem, seed_project,
    status_of,
};

/// A store with one project and one pending question, and a `mem` on PATH that
/// records every argv before running the real binary.
struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    mem: PathBuf,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (bin, log) = recording_mem(dir.path(), &home);
        let mem = real_mem().unwrap();
        seed_project(&mem, &home, "proj-alpha", "alpha did a thing");
        World {
            dir,
            home,
            bin,
            log,
            mem,
        }
    }

    fn ask(&self, question: &str) -> String {
        let out = mem_in(
            &self.mem,
            &self.home,
            &self.home.join("proj-alpha"),
            &["ask", question],
        );
        assert!(out.status.success(), "{out:?}");
        let out = mem_in(
            &self.mem,
            &self.home,
            &self.home,
            &["questions", "--pending", "--all-projects", "--json"],
        );
        let document: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let rows = document["questions"].as_array().unwrap();
        rows.iter()
            .find(|q| q["title"].as_str().is_some_and(|t| question.starts_with(t)))
            .unwrap_or_else(|| panic!("{question:?} not in {document:#}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn hub(&self) -> Hub {
        Hub::spawn(&self.home, &[&self.bin], &["--port", "0"])
    }

    /// Every `mem answer` hub ran.
    fn answers(&self) -> Vec<Vec<String>> {
        invocations(&self.log)
            .into_iter()
            .filter(|argv| argv.first().map(String::as_str) == Some("answer"))
            .collect()
    }
}

#[test]
fn the_page_has_the_three_sections_a_header_and_a_form() {
    let world = World::new("page-sections");
    world.ask("Should we use Redis?");
    let hub = world.hub();

    let response = hub.get("/");
    assert_eq!(status_of(&response), 200);
    let body = body_of(&response);

    assert!(body.contains("Pending questions"), "{body}");
    assert!(body.contains("Recent activity"), "{body}");
    assert!(body.contains("Projects"), "{body}");
    assert!(body.contains("Should we use Redis?"));
    assert!(body.contains("alpha did a thing"));
    assert!(body.contains("proj-alpha"));
    // AC4: answering is a form POST, and there is no script on the page at all.
    assert!(
        body.contains("<form method=\"post\" action=\"/answer\">"),
        "{body}"
    );
    assert!(body.contains("<textarea name=\"text\""), "{body}");
    // The reload is guarded: it must never fire while something is focused or
    // typed (a hard meta refresh ate an answer being written, twice). The form
    // above stays a plain POST, so answering still works with JS disabled.
    assert!(!body.contains("http-equiv=\"refresh\""), "{body}");
    assert!(body.contains("location.reload()"), "{body}");
    assert!(
        body.contains("activeElement"),
        "the reload checks focus: {body}"
    );
    // AC7: a project with no status.md is an em dash.
    assert!(body.contains('—'));
}

#[test]
fn the_header_carries_the_machine_name_and_the_sibling_links() {
    let world = World::new("page-header");
    let config = world.dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
         ntfy_base = \"http://127.0.0.1:9\"\n\
         siblings = [\"http://macbook:8787\", \"javascript:alert(1)\"]\n",
    )
    .unwrap();
    let hub = Hub::spawn(
        &world.home,
        &[&world.bin],
        &["--config", config.to_str().unwrap(), "--port", "0"],
    );

    let body = body_of(&hub.get("/")).to_string();

    // The machine-name rule is mem's: the qshell file, else `uname -n`. A
    // synthetic home has no qshell file, so this is `uname -n`.
    let machine =
        String::from_utf8_lossy(&Command::new("uname").arg("-n").output().unwrap().stdout)
            .trim()
            .to_string();
    assert!(body.contains(&format!("<h1>{machine}</h1>")), "{body}");

    assert!(
        body.contains("<a href=\"http://macbook:8787\">macbook:8787</a>"),
        "{body}"
    );
    assert!(body.contains("<a href=\"/subscribe\">"), "{body}");
    // A scheme that is not http(s) never becomes a link.
    assert!(!body.contains("javascript:"), "{body}");
}

#[test]
fn a_project_with_no_status_renders_an_em_dash_and_one_with_a_status_renders_it() {
    let world = World::new("page-status");
    seed_project(&world.mem, &world.home, "proj-beta", "beta did a thing");
    let out = mem_in(
        &world.mem,
        &world.home,
        &world.home.join("proj-beta"),
        &["status", "--set", "Green. Everything builds."],
    );
    assert!(out.status.success());

    let hub = world.hub();
    let body = body_of(&hub.get("/")).to_string();
    assert!(body.contains("Green. Everything builds."), "{body}");
    assert!(body.contains("proj-alpha"), "{body}");
    assert!(body.contains("—"), "{body}");
}

// ---------------------------------------------------------------------------
// AC9, three tests.
// ---------------------------------------------------------------------------

#[test]
fn a_foreign_origin_is_refused_and_runs_no_mem_answer() {
    let world = World::new("page-origin");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    let body = format!("id={id}&text=yes");
    for headers in [
        vec![("Origin", "https://evil.example.com")],
        vec![("Referer", "https://evil.example.com/page")],
        vec![("Sec-Fetch-Site", "cross-site")],
        // No evidence at all is also a no.
        vec![],
    ] {
        let response = hub.post_form_with("/answer", &body, &headers);
        assert_eq!(status_of(&response), 403, "{headers:?} → {response}");
    }
    assert!(
        world.answers().is_empty(),
        "a refused POST still ran mem answer: {:?}",
        world.answers()
    );

    // And the question is still pending.
    assert!(body_of(&hub.get("/")).contains("Should we use Redis?"));
}

#[test]
fn a_question_that_is_script_renders_as_text() {
    let world = World::new("page-xss");
    world.ask("<script>alert(1)</script>");
    let hub = world.hub();

    let body = body_of(&hub.get("/")).to_string();
    assert!(
        body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{body}"
    );
    assert!(
        !body.contains("<script>alert(1)"),
        "the question became script on hub's own origin: {body}"
    );
    // The whole page opens exactly one script tag — the guarded reload the
    // head always carries. A second one anywhere is an injection.
    assert_eq!(body.matches("<script").count(), 1, "{body}");
}

#[test]
fn an_answer_that_is_a_shell_command_leaves_no_file() {
    let world = World::new("page-shell");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    let marker = format!("/tmp/hub-pwned-{}", std::process::id());
    let _ = std::fs::remove_file(&marker);
    let hostile = format!("\"; touch {marker}; #");
    let body = format!("id={id}&text={}", urlencode(&hostile));

    let response = hub.post_form("/answer", &body);
    assert_eq!(status_of(&response), 303, "{response}");

    assert!(
        !Path::new(&marker).exists(),
        "the answer field reached a shell"
    );
    // And it reached mem verbatim, as one argument.
    let answers = world.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0], vec!["answer", "--", &id, &hostile]);
}

// ---------------------------------------------------------------------------
// The answer path itself.
// ---------------------------------------------------------------------------

#[test]
fn ac10_the_decoded_answer_reaches_mem_byte_for_byte() {
    let world = World::new("page-decode");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    // §10 AC10's string, end to end: over the socket, through the decoder, out
    // as one argv element.
    let body = format!("id={id}&text=a+b%26c%3Dd%2B%F0%9F%99%82");
    assert_eq!(status_of(&hub.post_form("/answer", &body)), 303);

    let answers = world.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0][3], "a b&c=d+🙂");
}

#[test]
fn a_successful_answer_redirects_with_a_banner_and_leaves_the_queue_empty() {
    let world = World::new("page-answer");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    let response = hub.post_form("/answer", &format!("id={id}&text=yes+please"));
    assert_eq!(status_of(&response), 303);
    let location = header_of(&response, "Location").unwrap().to_string();
    assert!(location.starts_with("/?answered="), "{location}");

    // The 5 s cache is dropped on the way out, so the very next render is
    // already without the question — no second tap, no second answer.
    let body = body_of(&hub.get(&location)).to_string();
    // mem's own short id, which is what `mem ask` printed and what the page
    // shows — not the 26-character ULID nobody has looked at.
    assert!(body.contains(&format!("Answered #{}", &id[18..])), "{body}");
    assert!(!body.contains("Should we use Redis?"), "{body}");
    assert!(body.contains("Nothing waiting."), "{body}");

    assert_eq!(world.answers().len(), 1);
}

#[test]
fn answering_the_same_question_twice_writes_once() {
    let world = World::new("page-double");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    let body = format!("id={id}&text=yes");
    assert_eq!(status_of(&hub.post_form("/answer", &body)), 303);
    let second = hub.post_form("/answer", &body);
    assert_eq!(status_of(&second), 303, "still a banner, never a 500");
    assert_eq!(
        header_of(&second, "Location"),
        Some("/?unknown=1"),
        "the second tap is told the queue has moved on"
    );

    assert_eq!(
        world.answers().len(),
        1,
        "mem has no CAS on answering, so the second write must not happen"
    );
    assert!(body_of(&hub.get("/?unknown=1")).contains("may already be answered"));
}

/// Six answers to one question, all released off the same barrier.
///
/// The sequential test above passed throughout review B-1 and could not detect
/// it: `invalidate` → `is_pending` → `answer` was check-then-act with the lock
/// dropped in the middle, so every racer read the question as pending before
/// any racer wrote. Twelve rounds out of twelve wrote more than once, all six
/// got the green banner, and `mem questions --wait` unblocked the orchestrator
/// with whichever landed first — not the one the phone was told about.
///
/// Not a contrived race: the page is JavaScript-free by AC4, so the Answer
/// button cannot disable itself; it refreshes itself every fifteen seconds; and
/// a render costs `2P+2+Q` serialised `mem` processes. A double tap on a link
/// that feels slow puts both POSTs on the wire before the first response
/// arrives.
#[test]
fn concurrent_answers_to_one_question_write_exactly_once() {
    const RACERS: usize = 6;

    let world = World::new("page-race");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();
    let port = hub.port;

    let barrier = Arc::new(Barrier::new(RACERS));
    let racers: Vec<_> = (0..RACERS)
        .map(|n| {
            let barrier = Arc::clone(&barrier);
            let id = id.clone();
            std::thread::spawn(move || {
                let body = format!("id={id}&text=r{n}");
                let request = format!(
                    "POST /answer HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Origin: http://127.0.0.1:{port}\r\nConnection: close\r\n\
                     Content-Type: application/x-www-form-urlencoded\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                // Connected before the barrier, so what the release lets go is
                // the write and not a TCP handshake apiece.
                let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                barrier.wait();
                stream.write_all(request.as_bytes()).unwrap();
                stream.flush().unwrap();
                let mut response = String::new();
                let _ = stream.read_to_string(&mut response);
                response
            })
        })
        .collect();
    let responses: Vec<String> = racers.into_iter().map(|r| r.join().unwrap()).collect();

    let answers = world.answers();
    assert_eq!(
        answers.len(),
        1,
        "mem has no CAS on answering, so exactly one racer may reach it: {answers:?}"
    );

    let answered = responses
        .iter()
        .filter(|r| header_of(r, "Location").is_some_and(|l| l.starts_with("/?answered=")))
        .count();
    let unknown = responses
        .iter()
        .filter(|r| header_of(r, "Location") == Some("/?unknown=1"))
        .count();
    assert_eq!(answered, 1, "only one of them may be told it worked");
    assert_eq!(
        unknown,
        RACERS - 1,
        "and the rest are told the queue moved on: {responses:?}"
    );

    // The one that wrote is the one whose text is now the answer, and the
    // question has left the queue.
    assert!(
        answers[0][3].starts_with('r'),
        "the text mem was given: {:?}",
        answers[0]
    );
    assert!(
        !body_of(&hub.get("/")).contains("Should we use Redis?"),
        "the question is still on the page after being answered"
    );
}

/// Two different questions do not queue behind each other: the lock is per id.
#[test]
fn answering_two_questions_at_once_is_not_serialised_into_one() {
    let world = World::new("page-race-two");
    let first = world.ask("Should we use Redis?");
    let second = world.ask("Should we cache the index?");
    let hub = world.hub();
    let port = hub.port;

    let barrier = Arc::new(Barrier::new(2));
    let both: Vec<_> = [first, second]
        .into_iter()
        .map(|id| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let body = format!("id={id}&text=yes");
                let request = format!(
                    "POST /answer HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Origin: http://127.0.0.1:{port}\r\nConnection: close\r\n\
                     Content-Type: application/x-www-form-urlencoded\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                barrier.wait();
                stream.write_all(request.as_bytes()).unwrap();
                stream.flush().unwrap();
                let mut response = String::new();
                let _ = stream.read_to_string(&mut response);
                response
            })
        })
        .collect();

    for response in both.into_iter().map(|t| t.join().unwrap()) {
        assert!(
            header_of(&response, "Location").is_some_and(|l| l.starts_with("/?answered=")),
            "both answers are for different questions and both should land: {response}"
        );
    }
    assert_eq!(world.answers().len(), 2);
}

/// `mem ask` prints `#RK4B2PBW`, and that is what a human copies.
///
/// The page's own form supplies the bare id, so the phone path never met this;
/// a typed or pasted one got `303 → /?unknown=1` and left the question pending.
#[test]
fn the_hash_mem_prints_in_front_of_an_id_is_not_part_of_the_id() {
    let world = World::new("page-hash");
    let id = world.ask("Should we use Redis?");
    let short = &id[18..];
    let hub = world.hub();

    // What a browser sends for a field typed as `#RK4B2PBW`.
    let response = hub.post_form("/answer", &format!("id=%23{short}&text=yes"));
    assert_eq!(status_of(&response), 303);
    assert_eq!(
        header_of(&response, "Location"),
        Some(format!("/?answered={short}").as_str())
    );

    let answers = world.answers();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0][2], short, "mem is given the id without the hash");
}

#[test]
fn a_hash_that_is_the_whole_id_is_still_an_empty_field() {
    let world = World::new("page-hash-only");
    world.ask("Should we use Redis?");
    let hub = world.hub();

    let response = hub.post_form("/answer", "id=%23&text=yes");
    assert_eq!(status_of(&response), 303);
    assert_eq!(header_of(&response, "Location"), Some("/?empty=1"));
    assert!(world.answers().is_empty(), "no id, no write");
}

/// review m-1: an explicit `cross-site` claim used to be overridden by the
/// first `Origin` header that followed it.
#[test]
fn a_cross_site_claim_is_believed_even_with_an_origin_of_ours_behind_it() {
    let world = World::new("page-crosssite");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();
    let origin = hub.origin();

    let response = hub.post_form_with(
        "/answer",
        &format!("id={id}&text=yes"),
        &[("Sec-Fetch-Site", "cross-site"), ("Origin", &origin)],
    );
    assert_eq!(status_of(&response), 403);
    assert!(world.answers().is_empty(), "and nothing was written");
}

#[test]
fn an_unknown_id_is_a_banner_and_not_a_five_hundred() {
    let world = World::new("page-unknown");
    world.ask("Should we use Redis?");
    let hub = world.hub();

    let response = hub.post_form("/answer", "id=DEADBEEF&text=yes");
    assert_eq!(status_of(&response), 303);
    assert_eq!(header_of(&response, "Location"), Some("/?unknown=1"));
    assert!(world.answers().is_empty(), "no id, no write");

    let body = body_of(&hub.get("/?unknown=1")).to_string();
    assert!(body.contains("may already be answered"), "{body}");
    // The banner is chosen here, never reflected: a crafted query cannot put
    // text on the page. The head's own guarded-reload script is the one
    // script tag a page ever carries; a reflected query would make two.
    let body = body_of(&hub.get("/?unknown=%3Cscript%3E")).to_string();
    assert_eq!(body.matches("<script").count(), 1, "{body}");
}

#[test]
fn an_empty_answer_is_refused_before_mem_is_asked() {
    let world = World::new("page-empty");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    for body in [
        format!("id={id}&text="),
        format!("id={id}&text=+++"),
        format!("id={id}"),
        "text=yes".to_string(),
    ] {
        let response = hub.post_form("/answer", &body);
        assert_eq!(status_of(&response), 303, "{body}");
        assert_eq!(
            header_of(&response, "Location"),
            Some("/?empty=1"),
            "{body}"
        );
    }
    assert!(world.answers().is_empty());
    assert!(body_of(&hub.get("/?empty=1")).contains("Type an answer first."));
}

#[test]
fn a_host_this_hub_does_not_answer_to_is_refused_everywhere() {
    let world = World::new("page-host");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    for path in ["/", "/api/questions", "/subscribe"] {
        let response = hub.raw(&format!(
            "GET {path} HTTP/1.1\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n"
        ));
        assert_eq!(status_of(&response), 403, "{path}");
    }

    // Even with an Origin that would otherwise pass: a rebinding attack
    // supplies both.
    let body = format!("id={id}&text=yes");
    let response = hub.raw(&format!(
        "POST /answer HTTP/1.1\r\nHost: evil.example.com\r\n\
         Origin: http://evil.example.com\r\nConnection: close\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    assert_eq!(status_of(&response), 403);
    assert!(world.answers().is_empty());

    // A MagicDNS short name is not a foreign host, and neither is a *.ts.net.
    for host in ["macbook", "macbook.taila27604.ts.net"] {
        let response = hub.raw(&format!(
            "GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        ));
        assert_eq!(status_of(&response), 200, "{host}");
    }
}

#[test]
fn sec_fetch_site_same_origin_is_accepted_on_its_own() {
    let world = World::new("page-secfetch");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();

    let response = hub.post_form_with(
        "/answer",
        &format!("id={id}&text=yes"),
        &[("Sec-Fetch-Site", "same-origin")],
    );
    assert_eq!(status_of(&response), 303);
    assert_eq!(world.answers().len(), 1);
}

// ---------------------------------------------------------------------------
// AC1, the round trip that is the whole point.
// ---------------------------------------------------------------------------

#[test]
fn ac1_an_answer_from_the_page_unblocks_a_waiting_agent() {
    let world = World::new("page-roundtrip");
    let id = world.ask("Should we use Redis?");
    let hub = world.hub();
    assert!(body_of(&hub.get("/")).contains("Should we use Redis?"));

    // The agent side of the contract: blocked on `mem questions --wait`.
    let mem = world.mem.clone();
    let home = world.home.clone();
    let waiting_for = id.clone();
    let waiter = std::thread::spawn(move || {
        Command::new(&mem)
            .args([
                "questions",
                "--wait",
                &waiting_for,
                "--timeout",
                "60s",
                "--json",
            ])
            .current_dir(&home)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("MEM_SYNC_CMD", "true")
            .env("MEM_NOTIFY_CMD", "true")
            // The default poll is 15 s; this test is about the round trip, not
            // about how long mem sleeps between looks.
            .env("MEM_POLL_MS", "200")
            .output()
            .expect("run mem questions --wait")
    });

    std::thread::sleep(Duration::from_millis(300));
    let response = hub.post_form("/answer", &format!("id={id}&text=yes%2C+redis"));
    assert_eq!(status_of(&response), 303, "{response}");

    let out = waiter.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("yes, redis"),
        "the waiter got the answer: {text}"
    );
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Keeps the temp dir alive for as long as the world is.
impl Drop for World {
    fn drop(&mut self) {
        let _ = &self.dir;
    }
}
