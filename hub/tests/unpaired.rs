//! A browser that never paired sees no control that posts, on any page, but
//! the line that says how to pair. A POST it sends anyway is refused, says
//! the answer was not sent, and leaves one line in the journal.

mod common;

use common::{
    DEVICE_COOKIE, Hub, TempDir, body_of, fixture_bin, mem_in, real_mem, recording_mem,
    seed_project, status_of,
};

const PAIR: &str = "Pair this device first: run hub pair on here-box";

/// The pages that carry a control for a paired device: home and the
/// questions page answer, the draft roadmap approves, new files, and an
/// approved project with nothing running offers Start and Pause.
const PAGES: [&str; 5] = [
    "/",
    "/p/gamma/questions",
    "/p/gamma/roadmap",
    "/p/gamma/new",
    "/p/omega",
];

struct World {
    _dir: TempDir,
    home: std::path::PathBuf,
    bin: std::path::PathBuf,
    mem: std::path::PathBuf,
}

/// gamma has a draft roadmap and a question waiting; omega an approved
/// milestone that amx is running nothing for.
fn world(tag: &str) -> World {
    let dir = TempDir::new(tag);
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("config/qshell")).unwrap();
    std::fs::write(home.join("config/qshell/machine"), "here-box\n").unwrap();
    let (bin, _log) = recording_mem(dir.path(), &home);
    fixture_bin(&bin, "amx", "echo '[]'");
    let mem = real_mem().unwrap();
    for (name, status) in [("gamma", "draft"), ("omega", "approved")] {
        seed_project(&mem, &home, name, "seeded");
        let roadmap = dir.join(&format!("{name}-roadmap.md"));
        std::fs::write(
            &roadmap,
            format!("# roadmap: {name}\n\n- [ ] {name}1 One\n"),
        )
        .unwrap();
        let run = |args: &[&str]| {
            let out = mem_in(&mem, &home, &home.join(name), args);
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        run(&["roadmap", "--set-file", roadmap.to_str().unwrap()]);
        run(&["roadmap", "--status", status]);
    }
    let out = mem_in(
        &mem,
        &home,
        &home.join("gamma"),
        &[
            "ask",
            "--for",
            "human",
            "--options",
            "seven,eight,nine",
            "Which hour?",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    World {
        _dir: dir,
        home,
        bin,
        mem,
    }
}

fn get(hub: &Hub, path: &str, cookie: Option<&str>) -> String {
    let cookie = cookie
        .map(|c| format!("Cookie: {c}\r\n"))
        .unwrap_or_default();
    let response = hub.raw(&format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{cookie}Connection: close\r\n\r\n",
        hub.port
    ));
    assert_eq!(status_of(&response), 200, "{path}: {response}");
    body_of(&response).to_string()
}

#[test]
fn an_unpaired_browser_sees_how_to_pair_in_place_of_every_control() {
    let world = world("unpaired-pages");
    let hub = Hub::spawn(&world.home, &[&world.bin], &["--port", "0"]);
    for page in PAGES {
        let paired = get(&hub, page, Some(DEVICE_COOKIE));
        assert!(paired.contains("method=\"post\""), "{page}: {paired}");
        assert!(!paired.contains(PAIR), "{page}: {paired}");

        let unpaired = get(&hub, page, None);
        assert!(!unpaired.contains("method=\"post\""), "{page}: {unpaired}");
        assert!(unpaired.contains(PAIR), "{page}: {unpaired}");
    }
    // The question is still there to read.
    assert!(get(&hub, "/", None).contains("Which hour?"));
}

#[test]
fn an_answer_from_an_unpaired_browser_is_not_sent_and_is_logged() {
    let world = world("unpaired-post");
    let log = world.home.join("hub.log");
    let hub = Hub::spawn_logged(&world.home, &[&world.bin], &["--port", "0"], &log);
    let questions = || {
        let out = mem_in(
            &world.mem,
            &world.home,
            &world.home.join("gamma"),
            &["questions", "--json"],
        );
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["questions"][0].clone()
    };
    let id = questions()["id"].as_str().unwrap().to_string();

    let before = std::fs::read_to_string(&log).unwrap();
    let response = hub.post_form_with(
        "/answer",
        &format!("id={id}&text=eight"),
        &[("Origin", &hub.origin()), ("Cookie", "theme=dark")],
    );
    assert_eq!(status_of(&response), 403, "{response}");
    assert!(body_of(&response).contains("not sent"), "{response}");
    assert!(body_of(&response).contains(PAIR), "{response}");
    assert_eq!(questions()["answered"], false);

    let after = std::fs::read_to_string(&log).unwrap();
    let added: Vec<&str> = after[before.len()..].lines().collect();
    assert_eq!(added.len(), 1, "{added:?}");
    assert!(added[0].contains("/answer"), "{added:?}");
    assert!(added[0].contains("127.0.0.1"), "{added:?}");
}
