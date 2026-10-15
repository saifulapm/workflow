//! The doorbell.
//!
//! `ntfy_base` never points at ntfy.sh in this file. Most tests put a fixture
//! `curl` on PATH, which records the argv and the state of the seen file at the
//! moment of the ring; one runs the real `curl` against a socket this test
//! opened, so the publish itself is exercised end to end.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use common::{
    Hub, TempDir, fixture_bin, fixture_mem, invocations, mem_in, real_mem, recording_mem,
    seed_project, status_of, wait_for,
};

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    curl_log: PathBuf,
    /// One line per ring: `yes` if the id was already in the seen file when
    /// curl ran, `no` if it was not.
    order_log: PathBuf,
    /// Every mem invocation hub made, so a test can count the doorbell's
    /// polls instead of guessing how long one takes.
    mem_log: PathBuf,
    config: PathBuf,
    mem: PathBuf,
}

impl World {
    /// `ntfy_base` points at `sink`, which is a port nothing listens on unless
    /// the test says otherwise.
    fn new(tag: &str, sink: &str) -> World {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let mem = real_mem().expect("build mem first");
        // The real mem, against a throwaway store, with hub's every call to
        // it on record.
        let (bin, mem_log) = recording_mem(dir.path(), &home);

        let curl_log = dir.join("curl.log");
        let order_log = dir.join("order.log");
        let seen = home.join("state/hub/seen");
        // Records the argv, then whether the id being rung for was already
        // durable — which is the ordering that matters.
        fixture_bin(
            &bin,
            "curl",
            &format!(
                "printf '\\1' >> '{log}'\n\
                 for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
                 if [ -s '{seen}' ]; then echo yes >> '{order}'; else echo no >> '{order}'; fi\n\
                 exit 0",
                log = curl_log.display(),
                seen = seen.display(),
                order = order_log.display(),
            ),
        );

        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            format!("topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\nntfy_base = \"{sink}\"\n"),
        )
        .unwrap();

        seed_project(&mem, &home, "proj-alpha", "alpha did a thing");
        World {
            dir,
            home,
            bin,
            curl_log,
            order_log,
            mem_log,
            config,
            mem,
        }
    }

    fn hub(&self) -> Hub {
        self.hub_with(&[])
    }

    fn hub_with(&self, env: &[(&str, &str)]) -> Hub {
        // A 15 s poll would make every test here take a minute.
        let mut env = env.to_vec();
        env.push(("HUB_POLL_MS", "120"));
        Hub::spawn_env(
            &self.home,
            &[&self.bin],
            &["--config", self.config.to_str().unwrap(), "--port", "0"],
            &env,
        )
    }

    /// The route the hub starts with, as its own file holds it.
    fn route_device(&self, machine: &str) {
        let path = self.home.join("state/hub/route.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("{{\"device\":true,\"machine\":\"{machine}\"}}"),
        )
        .unwrap();
    }

    fn ask_for(&self, question: &str, audience: &str) {
        let out = mem_in(
            &self.mem,
            &self.home,
            &self.home.join("proj-alpha"),
            &["ask", question, "--for", audience],
        );
        assert!(out.status.success(), "{out:?}");
    }

    fn ask(&self, question: &str) {
        let out = mem_in(
            &self.mem,
            &self.home,
            &self.home.join("proj-alpha"),
            &["ask", question],
        );
        assert!(out.status.success(), "{out:?}");
    }

    fn rings(&self) -> Vec<Vec<String>> {
        invocations(&self.curl_log)
    }

    /// The question ids the doorbell recorded. Its other keys, a stall, a
    /// finished roadmap and the marker beside them, have a space in them.
    fn seen(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.join("state/hub/seen"))
            .unwrap_or_default()
            .lines()
            .filter(|key| !key.contains(' '))
            .map(str::to_string)
            .collect()
    }

    /// How many times the doorbell has asked mem for the queue.
    fn polls(&self) -> usize {
        invocations(&self.mem_log)
            .iter()
            .filter(|argv| argv.first().is_some_and(|a| a == "questions"))
            .count()
    }

    /// Waits for the doorbell to go round twice more: the round in flight, if
    /// any, and one whole round after it, so whatever the queue held at the
    /// call has been seen and judged. This used to be a fixed sleep, and under
    /// suite load the seeding round outlasted it: a question asked after the
    /// sleep was swallowed as backlog and never rang.
    fn settle(&self) {
        let before = self.polls();
        wait_for("the doorbell to go round", Duration::from_secs(15), || {
            self.polls() >= before + 2
        });
    }

    /// Waits for the first ring to be whole. The fixture curl writes the
    /// record marker first and the order line last, so a non-empty curl log
    /// can still be missing the argv and the order log; a test that read them
    /// then failed under suite load.
    fn wait_for_ring(&self) {
        wait_for("the doorbell to ring", Duration::from_secs(15), || {
            std::fs::metadata(&self.order_log).is_ok_and(|m| m.len() > 0)
        });
    }
}

#[test]
fn ac2_a_new_question_rings_exactly_once_however_many_times_it_is_polled() {
    let world = World::new("bell-once", "http://127.0.0.1:9");
    let hub = world.hub();
    // First start with an empty queue: seeding has nothing to record.
    world.settle();
    assert!(world.rings().is_empty());

    world.ask("Should we use Redis?");
    wait_for("the doorbell to ring", Duration::from_secs(5), || {
        !world.rings().is_empty()
    });

    // Several more polls go by; the id is in the seen file, so nothing rings.
    world.settle();
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    assert_eq!(world.seen().len(), 1);

    // The service is still serving, and the question is on the page.
    assert_eq!(status_of(&hub.get("/")), 200);
}

#[test]
fn the_ring_carries_no_question_text_and_never_touches_a_shell() {
    let world = World::new("bell-body", "http://127.0.0.1:9");
    let _hub = world.hub();
    world.settle();
    world.ask("Should we deploy the thing that must not be named?");
    world.wait_for_ring();

    let rings = world.rings();
    assert_eq!(rings.len(), 1, "{rings:?}");
    let argv = &rings[0];
    // The documented command line, argument for argument.
    assert_eq!(argv[0], "-fsS");
    assert_eq!(argv[1], "-m");
    assert_eq!(argv[2], "10");
    assert_eq!(argv[3], "-d");
    assert_eq!(
        argv[5],
        "http://127.0.0.1:9/workflow-TESTTESTTESTTESTTESTTESTTE"
    );

    let body = &argv[4];
    assert!(body.starts_with("question waiting on "), "{body}");
    assert!(body.contains("(proj-alpha)"), "{body}");
    assert!(body.contains("http://"), "a link back to this hub: {body}");
    assert!(
        !body.contains("must not be named"),
        "the question text left the tailnet: {body}"
    );
}

#[test]
fn the_seen_file_is_written_before_the_ring() {
    let world = World::new("bell-order", "http://127.0.0.1:9");
    let _hub = world.hub();
    world.settle();
    world.ask("Should we use Redis?");
    world.wait_for_ring();

    // A crash between the two has to lose a doorbell, not loop on one.
    let order = std::fs::read_to_string(&world.order_log).unwrap();
    assert_eq!(
        order.trim(),
        "yes",
        "curl ran before the id was durable, so a crash here would re-ring"
    );
}

#[test]
fn ac5_a_restart_does_not_re_ring() {
    let world = World::new("bell-restart", "http://127.0.0.1:9");
    let hub = world.hub();
    world.settle();
    world.ask("Should we use Redis?");
    wait_for("the first ring", Duration::from_secs(5), || {
        world.rings().len() == 1
    });
    let seen_before = world.seen();

    // Exactly what `systemctl --user restart` does.
    drop(hub);
    let hub = world.hub();
    world.settle();
    world.settle();

    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    assert_eq!(world.seen(), seen_before);
    assert_eq!(status_of(&hub.get("/")), 200);
}

#[test]
fn the_first_start_records_the_backlog_and_rings_for_none_of_it() {
    let world = World::new("bell-seed", "http://127.0.0.1:9");
    // Three questions already pending before hub has ever run — a fresh
    // install, or a machine that has just synced somebody else's queue.
    world.ask("one?");
    world.ask("two?");
    world.ask("three?");
    assert!(!world.home.join("state/hub/seen").exists());

    let hub = world.hub();
    world.settle();
    world.settle();

    assert!(
        world.rings().is_empty(),
        "an install should not buzz once per pending question: {:?}",
        world.rings()
    );
    assert_eq!(world.seen().len(), 3, "but all three are recorded");

    // And the next one after that does ring.
    world.ask("four?");
    wait_for(
        "the fourth question to ring",
        Duration::from_secs(5),
        || world.rings().len() == 1,
    );
    assert_eq!(status_of(&hub.get("/")), 200);
}

/// A question that arrives by sync, born on another machine, rings here: this
/// hub is the only one, so no other machine rang for it.
#[test]
fn a_question_from_another_machine_rings_on_this_hub() {
    let world = World::new("bell-foreign", "http://127.0.0.1:9");
    let machine_file = world.home.join("config/qshell/machine");
    std::fs::create_dir_all(machine_file.parent().unwrap()).unwrap();
    std::fs::write(&machine_file, "here-hub").unwrap();

    let _hub = world.hub();
    world.settle(); // the seeding round, on an empty queue

    // The sibling's question lands in the store, the way bisync would land it:
    // mem reads the machine file per invocation, hub read it once at start.
    std::fs::write(&machine_file, "far-nuc").unwrap();
    world.ask("Should we use Redis?");
    wait_for("the id to be recorded", Duration::from_secs(15), || {
        world.seen().len() == 1
    });
    world.wait_for_ring();
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    assert!(
        world.rings()[0].iter().any(|a| a.contains("proj-alpha")),
        "{:?}",
        world.rings()
    );
    // The push names the machine that asked, not the hub's own.
    let body = &world.rings()[0][4];
    assert!(body.starts_with("question waiting on far-nuc "), "{body}");
}

#[test]
fn an_unreachable_ntfy_keeps_the_service_serving() {
    // Real curl, and a port with nothing on it: curl fails, hub carries on.
    let world = World::new("bell-unreachable", "http://127.0.0.1:9");
    std::fs::remove_file(world.bin.join("curl")).unwrap();
    let hub = world.hub();
    world.settle();
    world.ask("Should we use Redis?");
    world.settle();
    world.settle();

    assert_eq!(status_of(&hub.get("/")), 200);
    assert_eq!(status_of(&hub.get("/wiki")), 200);
    // The id is still recorded, so the failure is not retried for ever.
    assert_eq!(world.seen().len(), 1);
}

#[test]
fn the_publish_reaches_a_real_sink_over_real_curl() {
    // The one test that runs curl for real. The sink is a socket this test
    // opened on loopback — ntfy.sh is never involved.
    let sink = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = sink.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = sink.accept() {
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let _ = tx.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
        }
    });

    let world = World::new("bell-real", &format!("http://127.0.0.1:{port}"));
    // No fixture curl: the real one, off /usr/bin.
    std::fs::remove_file(world.bin.join("curl")).unwrap();
    let _hub = world.hub();
    world.settle();
    world.ask("Should we use Redis?");

    let request = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the sink received a publish");
    assert!(
        request.starts_with("POST /workflow-TESTTESTTESTTESTTESTTESTTE "),
        "{request}"
    );
    assert!(request.contains("question waiting on "), "{request}");
    assert!(request.contains("(proj-alpha)"), "{request}");
    assert!(!request.contains("Redis"), "{request}");
}

#[test]
fn a_state_directory_that_cannot_be_written_does_not_become_a_ring_every_poll() {
    let world = World::new("bell-readonly", "http://127.0.0.1:9");
    // A file where the directory should be: `create_dir_all` cannot win.
    std::fs::create_dir_all(world.home.join("state")).unwrap();
    std::fs::write(world.home.join("state/hub"), "in the way").unwrap();

    let hub = world.hub();
    world.settle();
    world.ask("Should we use Redis?");
    world.settle();
    world.settle();

    assert!(
        world.rings().is_empty(),
        "an id that could not be recorded must not be rung: {:?}",
        world.rings()
    );
    assert_eq!(status_of(&hub.get("/")), 200, "and the page still renders");
}

/// One empty-stdout poll at first start used to throw the seeding
/// guard away and ring for the whole backlog.
///
/// The gate was `outcome.broken()`, which is `None` for `Outcome::Absent`, so
/// an "exit non-zero, empty stdout" answer fell straight through it: the loop
/// over no rows did nothing and `seeding` was cleared anyway, and the next poll
/// treated every already-pending question as new. On the live machine that
/// first round wrote eleven ids — eleven push notifications to a public broker,
/// naming eleven projects, if that one poll had come back empty. The trigger is
/// ordinary: any `mem` that exits non-zero with nothing on stdout, on the one
/// start where it matters.
#[test]
fn a_first_poll_that_prints_nothing_does_not_spend_the_seeding_round() {
    let dir = TempDir::new("bell-silent-first");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin = dir.join("bin");

    let curl_log = dir.join("curl.log");
    fixture_bin(
        &bin,
        "curl",
        &format!(
            "printf '\\1' >> '{log}'\n\
             for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
             exit 0",
            log = curl_log.display()
        ),
    );

    // Three questions already pending, and a `mem` whose *first* `questions`
    // call prints nothing and exits 1.
    let queue = dir.join("queue.json");
    std::fs::write(&queue, backlog(3)).unwrap();
    let polls = dir.join("polls");
    fixture_mem(
        &bin,
        &format!(
            "if [ \"$1\" = questions ]; then\n\
             \x20 n=$(cat '{polls}' 2>/dev/null || echo 0)\n\
             \x20 echo $((n+1)) > '{polls}'\n\
             \x20 [ \"$n\" -eq 0 ] && exit 1\n\
             \x20 cat '{queue}'\n\
             \x20 exit 1\n\
             fi\n\
             if [ \"$1\" = projects ]; then echo '{{\"projects\":[]}}'; exit 0; fi\n\
             exit 1",
            polls = polls.display(),
            queue = queue.display(),
        ),
    );

    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\nntfy_base = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();

    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["--config", config.to_str().unwrap(), "--port", "0"],
        &[("HUB_POLL_MS", "120")],
    );
    // The question ids recorded, as `World::seen` counts them.
    let seen = || {
        std::fs::read_to_string(home.join("state/hub/seen"))
            .unwrap_or_default()
            .lines()
            .filter(|key| !key.contains(' '))
            .count()
    };
    let polled = || {
        std::fs::read_to_string(&polls)
            .ok()
            .and_then(|n| n.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let settle = || {
        let before = polled();
        wait_for("the doorbell to go round", Duration::from_secs(15), || {
            polled() >= before + 2
        });
    };
    wait_for(
        "the backlog to be recorded",
        Duration::from_secs(10),
        || seen() == 3,
    );
    settle();
    settle();

    assert!(
        invocations(&curl_log).is_empty(),
        "a first start rings zero times, however the first poll went: {:?}",
        invocations(&curl_log)
    );
    assert!(
        polled() > 1,
        "the doorbell stopped polling instead of seeding"
    );

    // And the doorbell is alive: the fourth question does ring, exactly once.
    std::fs::write(&queue, backlog(4)).unwrap();
    wait_for(
        "the fourth question to ring",
        Duration::from_secs(10),
        || !invocations(&curl_log).is_empty(),
    );
    settle();
    settle();
    assert_eq!(invocations(&curl_log).len(), 1);
    assert_eq!(status_of(&hub.get("/")), 200);
}

/// `n` pending questions, in the shape `mem questions --json` prints.
fn backlog(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "{{\"id\":\"01M0BF1F8BY8FXGZS428J1TS{i:02}\",\
                  \"short_id\":\"28J1TS{i:02}\",\
                  \"title\":\"question {i}\",\
                  \"project\":\"p\"}}"
            )
        })
        .collect();
    format!("{{\"questions\":[{}]}}", rows.join(","))
}

/// The click URL the phone is sent used to derive from the qshell machine name
/// — `macbook-m2` here, which resolves on no network at all, so the buzz opened
/// a dead link.
#[test]
fn the_doorbell_link_is_the_tailnet_name_rather_than_the_machine_name() {
    let world = World::new("bell-url", "http://127.0.0.1:9");
    let hub = Hub::spawn_env(
        &world.home,
        &[&world.bin],
        &["--config", world.config.to_str().unwrap(), "--port", "0"],
        &[
            ("HUB_POLL_MS", "120"),
            ("HUB_TAILNET_NAME", "macbook.taila27604.ts.net"),
        ],
    );
    world.settle();
    world.ask("Should we use Redis?");
    world.wait_for_ring();

    let body = world.rings()[0][4].clone();
    assert!(
        body.contains(&format!("http://macbook.taila27604.ts.net:{}/", hub.port)),
        "{body}"
    );
    // And that name is one hub answers to, so the link is not a 403.
    assert_eq!(status_of(&hub.get("/")), 200);
}

#[test]
fn an_explicit_hub_url_still_wins_over_the_tailnet_name() {
    let world = World::new("bell-url-config", "http://127.0.0.1:9");
    let mut config = std::fs::read_to_string(&world.config).unwrap();
    config.push_str("hub_url = \"http://chosen.example:8787/\"\n");
    std::fs::write(&world.config, config).unwrap();

    let _hub = Hub::spawn_env(
        &world.home,
        &[&world.bin],
        &["--config", world.config.to_str().unwrap(), "--port", "0"],
        &[
            ("HUB_POLL_MS", "120"),
            ("HUB_TAILNET_NAME", "macbook.taila27604.ts.net"),
        ],
    );
    world.settle();
    world.ask("Should we use Redis?");
    world.wait_for_ring();

    let body = world.rings()[0][4].clone();
    assert!(body.contains("http://chosen.example:8787/"), "{body}");
}

/// On Phone every ring is a push, even while someone sits at a machine: the
/// quiet desk is what Device is for (decision, m5).
#[test]
fn on_phone_a_watched_machine_still_rings_the_phone() {
    let world = World::new("bell-watched", "http://127.0.0.1:9");
    let hub = world.hub();
    hub.poll_agent(r#"{"machine":"mini","agents":[],"checkouts":[],"watched":true}"#);
    world.settle();

    world.ask("Should we use Redis?");
    world.wait_for_ring();
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    let commands =
        hub.poll_agent(r#"{"machine":"mini","agents":[],"checkouts":[],"watched":true}"#);
    assert!(!commands.contains("notify"), "{commands}");
}

/// An orchestrator's question is answered by the orchestrator, so it never
/// rings; Saiful's own rings like any person's.
#[test]
fn only_a_persons_question_rings() {
    let world = World::new("bell-audience", "http://127.0.0.1:9");
    let _hub = world.hub();
    world.settle();

    world.ask_for("Widen the files?", "orchestrator");
    world.settle();
    assert!(world.rings().is_empty(), "{:?}", world.rings());

    world.ask_for("Approve the roadmap?", "saiful");
    world.wait_for_ring();
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
}

/// The notify commands a machine's next poll hands it.
fn notifies(hub: &Hub, report: &str) -> Vec<serde_json::Value> {
    let answer: serde_json::Value = serde_json::from_str(&hub.poll_agent(report)).unwrap();
    answer["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "notify")
        .cloned()
        .collect()
}

const MINI_AT_DESK: &str = r#"{"machine":"mini","agents":[],"checkouts":[],"watched":true}"#;
const MINI_AWAY: &str = r#"{"machine":"mini","agents":[],"checkouts":[],"watched":false}"#;

/// On Device, the ring is one desktop notification on that machine, carrying
/// what the push would and no question text, and nothing goes to ntfy.
#[test]
fn on_device_a_question_shows_on_that_machine_and_pushes_nothing() {
    let world = World::new("bell-device", "http://127.0.0.1:9");
    world.route_device("mini");
    let hub = world.hub();
    hub.poll_agent(MINI_AT_DESK);
    world.settle();

    world.ask("Should we deploy the thing that must not be named?");
    wait_for("the id to be recorded", Duration::from_secs(15), || {
        world.seen().len() == 1
    });
    let shown = notifies(&hub, MINI_AT_DESK);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let body = shown[0]["body"].as_str().unwrap();
    assert!(body.starts_with("question waiting on "), "{body}");
    assert!(body.contains("(proj-alpha)"), "{body}");
    assert!(!body.contains("must not be named"), "{body}");
    world.settle();
    assert!(world.rings().is_empty(), "{:?}", world.rings());
    assert!(notifies(&hub, MINI_AT_DESK).is_empty(), "shown once");
}

/// A machine whose screen went idle or locked a moment ago still holds the
/// route: it falls to the phone only after the away time.
#[test]
fn on_device_a_machine_idle_a_moment_still_shows_it() {
    let world = World::new("bell-device-idle", "http://127.0.0.1:9");
    world.route_device("mini");
    let hub = world.hub();
    hub.poll_agent(MINI_AWAY);
    world.settle();

    world.ask("Should we use Redis?");
    wait_for("the id to be recorded", Duration::from_secs(15), || {
        world.seen().len() == 1
    });
    world.settle();
    assert_eq!(notifies(&hub, MINI_AWAY).len(), 1);
    assert!(world.rings().is_empty(), "{:?}", world.rings());
}

/// Locked or idle for the away time, the route's machine is away, and the
/// ring goes to the phone instead.
#[test]
fn on_device_a_machine_away_long_enough_rings_the_phone() {
    let world = World::new("bell-device-away", "http://127.0.0.1:9");
    world.route_device("mini");
    let hub = world.hub_with(&[("HUB_AWAY_MS", "300")]);
    hub.poll_agent(MINI_AWAY);
    world.settle();
    std::thread::sleep(Duration::from_millis(400));
    hub.poll_agent(MINI_AWAY);

    world.ask("Should we use Redis?");
    world.wait_for_ring();
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    assert!(notifies(&hub, MINI_AWAY).is_empty());

    // Back at the desk, the next ring shows on the machine again.
    hub.poll_agent(MINI_AT_DESK);
    world.ask("And Postgres over MySQL?");
    wait_for("the id to be recorded", Duration::from_secs(15), || {
        world.seen().len() == 2
    });
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    assert_eq!(notifies(&hub, MINI_AT_DESK).len(), 1);
}

/// A machine whose agent stopped polling is away however it was last seen.
#[test]
fn on_device_an_offline_machine_rings_the_phone() {
    let world = World::new("bell-device-offline", "http://127.0.0.1:9");
    world.route_device("mini");
    let hub = world.hub_with(&[("HUB_AGENT_WINDOW_MS", "200")]);
    hub.poll_agent(MINI_AT_DESK);
    world.settle();
    std::thread::sleep(Duration::from_millis(300));

    world.ask("Should we use Redis?");
    world.wait_for_ring();
    world.settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
}

/// An unwatched machine rings the phone directly — sibling hubs are links on
/// the page, never a delivery channel.
#[test]
fn an_unwatched_machine_rings_the_phone_and_never_probes_a_sibling() {
    let world = World::new("bell-unwatched", "http://127.0.0.1:9");
    let mut config = std::fs::read_to_string(&world.config).unwrap();
    config.push_str("siblings = [\"http://127.0.0.1:9\"]\n");
    std::fs::write(&world.config, config).unwrap();
    let _hub = world.hub();
    world.settle();

    world.ask("Should we use Redis?");
    world.wait_for_ring();
    world.settle();

    let calls = world.rings();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(
        calls[0]
            .iter()
            .any(|a| a.contains("workflow-TESTTESTTESTTESTTESTTESTTE")),
        "the one delivery is the ntfy publish: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .flatten()
            .any(|a| a.contains("/api/presence") || a.contains("/api/notify")),
        "no sibling was probed or notified: {calls:?}"
    );
}

#[test]
fn the_seen_file_is_one_id_per_line_and_survives_being_read_back() {
    let dir = TempDir::new("bell-seenfile");
    let path = dir.join("state/hub/seen");
    assert!(hub::doorbell::load(&path).is_none(), "absent, not empty");

    hub::doorbell::remember(&path, "01M0BF1F8BY8FXGZS428J1TSD1").unwrap();
    hub::doorbell::remember(&path, "01M0BF3B9VWD6PMF213HNZ1Q1D").unwrap();

    let seen = hub::doorbell::load(&path).unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen.contains("01M0BF1F8BY8FXGZS428J1TSD1"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().lines().count(),
        2,
        "one id per line"
    );
}

/// Keeps the temp dir alive for the life of the world.
impl Drop for World {
    fn drop(&mut self) {
        let _ = &self.dir;
    }
}

/// A hub over a fake `mem` that prints whatever JSON the test last put in
/// `data/`, so a test can stage a stall or a finished roadmap without
/// driving an orchestrator.
struct Fake {
    _dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    data: PathBuf,
    curl_log: PathBuf,
    mem_log: PathBuf,
    config: PathBuf,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        let dir = TempDir::new(tag);
        let home = dir.join("home");
        let bin = dir.join("bin");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let machine_file = home.join("config/qshell/machine");
        std::fs::create_dir_all(machine_file.parent().unwrap()).unwrap();
        std::fs::write(&machine_file, "here-hub").unwrap();

        let curl_log = dir.join("curl.log");
        fixture_bin(
            &bin,
            "curl",
            &format!(
                "printf '\\1' >> '{log}'\n\
                 for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
                 exit 0",
                log = curl_log.display()
            ),
        );
        let mem_log = dir.join("argv.log");
        fixture_mem(
            &bin,
            &format!(
                "printf '\\1' >> '{log}'\n\
                 for a in \"$@\"; do printf '%s\\0' \"$a\" >> '{log}'; done\n\
                 case \"$1\" in\n\
                 \x20 questions) cat '{data}/questions.json'; exit 0 ;;\n\
                 \x20 projects) cat '{data}/projects.json'; exit 0 ;;\n\
                 \x20 handoff) cat '{data}/handoff.json' 2>/dev/null || echo '{{\"body\":\"\"}}'; exit 0 ;;\n\
                 esac\n\
                 exit 1",
                log = mem_log.display(),
                data = data.display(),
            ),
        );

        let config = dir.join("config.toml");
        std::fs::write(
            &config,
            "topic = \"workflow-TESTTESTTESTTESTTESTTESTTE\"\n\
             ntfy_base = \"http://127.0.0.1:9\"\n\
             hub_url = \"http://hub.test:8787/\"\n",
        )
        .unwrap();

        let fake = Fake {
            _dir: dir,
            home,
            bin,
            data,
            curl_log,
            mem_log,
            config,
        };
        fake.put("questions.json", "{\"questions\":[]}");
        fake.projects("2026-10-05T09:00:00Z", "approved", 1);
        fake
    }

    fn hub(&self) -> Hub {
        Hub::spawn_env(
            &self.home,
            &[&self.bin],
            &["--config", self.config.to_str().unwrap(), "--port", "0"],
            &[("HUB_POLL_MS", "120")],
        )
    }

    /// Written whole and renamed into place, so the fake never prints half a
    /// document.
    fn put(&self, name: &str, text: &str) {
        let staged = self.data.join(format!(".{name}"));
        std::fs::write(&staged, text).unwrap();
        std::fs::rename(&staged, self.data.join(name)).unwrap();
    }

    /// proj-alpha, last active at `alpha_at`, and proj-beta, neither checked
    /// out here, as on the one hub, with `beta_done` of beta's three
    /// milestones done under `beta_status`.
    fn projects(&self, alpha_at: &str, beta_status: &str, beta_done: u32) {
        self.put(
            "projects.json",
            &format!(
                "{{\"projects\":[\
                 {{\"name\":\"proj-alpha\",\"last_activity\":\"{alpha_at}\",\
                   \"roadmap_status\":\"approved\",\"milestones_done\":1,\
                   \"milestones_total\":3,\"checkouts\":[]}},\
                 {{\"name\":\"proj-beta\",\"last_activity\":\"2026-10-05T08:00:00Z\",\
                   \"roadmap_status\":\"{beta_status}\",\"milestones_done\":{beta_done},\
                   \"milestones_total\":3,\"checkouts\":[]}}]}}"
            ),
        );
    }

    /// Every ring's body, in the order curl was run.
    fn rings(&self) -> Vec<String> {
        invocations(&self.curl_log)
            .into_iter()
            .map(|argv| argv[4].clone())
            .collect()
    }

    fn seen(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.join("state/hub/seen"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn spawns(&self, verb: &str) -> usize {
        invocations(&self.mem_log)
            .iter()
            .filter(|argv| argv.first().is_some_and(|a| a == verb))
            .count()
    }

    /// The round in flight and one whole round after it, as `World::settle`.
    fn settle(&self) {
        let before = self.spawns("questions");
        wait_for("the doorbell to go round", Duration::from_secs(15), || {
            self.spawns("questions") >= before + 2
        });
    }
}

const FINISHED: &str =
    "roadmap finished on here-hub (proj-beta) — http://hub.test:8787/p/proj-beta";

#[test]
fn a_finished_roadmap_rings_once_and_a_restart_rings_none() {
    let fake = Fake::new("bell-finished");
    let hub = fake.hub();
    fake.settle();
    assert!(fake.rings().is_empty(), "{:?}", fake.rings());

    // proj-beta's last milestone is ticked; its stored status never moves.
    fake.projects("2026-10-05T10:00:00Z", "approved", 3);
    wait_for("the finished ring", Duration::from_secs(15), || {
        !fake.rings().is_empty()
    });
    fake.settle();
    fake.settle();
    assert_eq!(fake.rings(), [FINISHED], "one ring, and no title in it");

    drop(hub);
    let _hub = fake.hub();
    fake.settle();
    fake.settle();
    assert_eq!(fake.rings(), [FINISHED], "a restart rang again");
}

#[test]
fn a_first_start_records_a_finished_roadmap_and_rings_none() {
    let fake = Fake::new("bell-finished-seed");
    fake.projects("2026-10-05T10:00:00Z", "approved", 3);
    let _hub = fake.hub();
    wait_for(
        "the backlog to be recorded",
        Duration::from_secs(15),
        || !fake.seen().is_empty(),
    );
    fake.settle();
    fake.settle();

    assert!(fake.rings().is_empty(), "{:?}", fake.rings());
    assert_eq!(
        fake.seen(),
        ["finished proj-beta 3", "finished roadmaps seeded"]
    );
}

#[test]
fn an_upgrade_records_roadmaps_already_finished_and_rings_none() {
    let fake = Fake::new("bell-finished-upgrade");
    // The seen file an older hub left: a question, and no finished roadmap,
    // since its finished ring keyed on a status nothing sets any more.
    let seen = fake.home.join("state/hub/seen");
    std::fs::create_dir_all(seen.parent().unwrap()).unwrap();
    std::fs::write(&seen, "01M0BJ42PYJV0NA7CE464V826E\n").unwrap();
    fake.projects("2026-10-05T10:00:00Z", "approved", 3);
    let _hub = fake.hub();
    fake.settle();
    fake.settle();
    assert!(fake.rings().is_empty(), "{:?}", fake.rings());
    assert!(
        fake.seen().contains(&"finished proj-beta 3".to_string()),
        "{:?}",
        fake.seen()
    );

    // A roadmap that finishes after that round still rings.
    fake.put(
        "projects.json",
        "{\"projects\":[\
         {\"name\":\"proj-alpha\",\"roadmap_status\":\"approved\",\"milestones_done\":3,\
           \"milestones_total\":3,\"checkouts\":[]},\
         {\"name\":\"proj-beta\",\"roadmap_status\":\"approved\",\"milestones_done\":3,\
           \"milestones_total\":3,\"checkouts\":[]}]}",
    );
    wait_for("the finished ring", Duration::from_secs(15), || {
        !fake.rings().is_empty()
    });
    fake.settle();
    assert_eq!(
        fake.rings(),
        ["roadmap finished on here-hub (proj-alpha) — http://hub.test:8787/p/proj-alpha"]
    );
}

/// proj-alpha building m2, run on the mini as `runner`, and the mini's report
/// listing its orchestrators as `(id, state)`.
fn building(fake: &Fake, hub: &Hub, runner: &str, agents: &[(&str, &str)]) {
    let rows: Vec<String> = agents
        .iter()
        .enumerate()
        .map(|(i, (id, state))| {
            format!("{{\"id\":\"{id}\",\"state\":\"{state}\",\"ended\":0,\"created\":{i}}}")
        })
        .collect();
    building_with(fake, hub, runner, &format!("[{}]", rows.join(",")));
}

/// `building`, with the mini's agents as the JSON list `agents`.
fn building_with(fake: &Fake, hub: &Hub, runner: &str, agents: &str) {
    fake.put(
        "projects.json",
        &format!(
            "{{\"projects\":[{{\"name\":\"proj-alpha\",\"last_activity\":\"2026-10-05T09:00:00Z\",\
              \"roadmap_status\":\"approved\",\"milestone\":\"m2\",\"milestones_done\":1,\
              \"milestones_total\":3,\"checkouts\":[],\
              \"runner\":{{\"machine\":\"mini\",\"id\":\"{runner}\",\"since\":\"2026-10-05T08:00:00Z\"}}}}]}}"
        ),
    );
    hub.poll_agent(&format!(
        "{{\"machine\":\"mini\",\"agents\":{agents},\"checkouts\":[\"proj-alpha\"]}}"
    ));
}

const STALLED: &str = "stalled on mini (proj-alpha) — http://hub.test:8787/p/proj-alpha";

#[test]
fn a_dead_orchestrator_rings_once_and_its_successors_death_rings_again() {
    let fake = Fake::new("bell-stall");
    let hub = fake.hub();
    building(
        &fake,
        &hub,
        "proj-alpha-m2",
        &[("proj-alpha-m2", "working")],
    );
    fake.settle();
    assert!(fake.rings().is_empty(), "{:?}", fake.rings());

    building(
        &fake,
        &hub,
        "proj-alpha-m2",
        &[("proj-alpha-m2", "stopped")],
    );
    wait_for("the stall to ring", Duration::from_secs(15), || {
        !fake.rings().is_empty()
    });
    fake.settle();
    fake.settle();
    assert_eq!(fake.rings(), [STALLED], "one ring per stall, not per poll");

    // Resumed as -2, which then dies too: a second stall.
    building(
        &fake,
        &hub,
        "proj-alpha-m2-2",
        &[("proj-alpha-m2", "stopped"), ("proj-alpha-m2-2", "working")],
    );
    fake.settle();
    building(
        &fake,
        &hub,
        "proj-alpha-m2-2",
        &[("proj-alpha-m2", "stopped"), ("proj-alpha-m2-2", "stopped")],
    );
    wait_for("the second stall to ring", Duration::from_secs(15), || {
        fake.rings().len() >= 2
    });
    fake.settle();
    assert_eq!(fake.rings(), [STALLED, STALLED]);
}

#[test]
fn a_runner_whose_machine_is_away_never_rings_its_stall() {
    let fake = Fake::new("bell-away");
    // The mini never polls this hub, so its runner reads away however long
    // the milestone sits.
    fake.put(
        "projects.json",
        "{\"projects\":[{\"name\":\"proj-alpha\",\"last_activity\":\"2026-10-05T09:00:00Z\",\
          \"roadmap_status\":\"approved\",\"milestone\":\"m2\",\"milestones_done\":1,\
          \"milestones_total\":3,\"checkouts\":[],\
          \"runner\":{\"machine\":\"mini\",\"id\":\"proj-alpha-m2\"}}]}",
    );
    let hub = fake.hub();
    // Another machine is online and lists the id dead: only the runner's
    // machine counts.
    hub.poll_agent(
        "{\"machine\":\"nuc\",\"agents\":[{\"id\":\"proj-alpha-m2\",\"state\":\"stopped\",\
         \"ended\":0,\"created\":1}],\"checkouts\":[\"proj-alpha\"]}",
    );
    // Past the grace a runner its machine does not list waits for.
    std::thread::sleep(Duration::from_secs(2));
    fake.settle();
    assert!(fake.rings().is_empty(), "{:?}", fake.rings());
}

#[test]
fn a_parked_milestone_does_not_ring() {
    let fake = Fake::new("bell-parked");
    fake.put(
        "handoff.json",
        "{\"body\":\"\\nparked: waiting on a question\"}",
    );
    let hub = fake.hub();
    building(&fake, &hub, "proj-alpha-m2", &[]);
    fake.settle();

    building(
        &fake,
        &hub,
        "proj-alpha-m2",
        &[("proj-alpha-m2", "stopped")],
    );
    // Past the grace a runner its machine does not list waits for.
    std::thread::sleep(Duration::from_secs(2));
    fake.settle();
    assert!(fake.rings().is_empty(), "{:?}", fake.rings());
}

#[test]
fn a_paused_project_does_not_ring_its_stall() {
    let fake = Fake::new("bell-paused");
    let hub = fake.hub();
    building(
        &fake,
        &hub,
        "proj-alpha-m2",
        &[("proj-alpha-m2", "working")],
    );
    fake.settle();

    building(
        &fake,
        &hub,
        "proj-alpha-m2",
        &[("proj-alpha-m2", "stopped")],
    );
    let projects = std::fs::read_to_string(fake.data.join("projects.json")).unwrap();
    fake.put(
        "projects.json",
        &projects.replace(
            "\"milestone\":",
            "\"paused\":\"2026-10-15T09:00:00Z by saiful\",\"milestone\":",
        ),
    );
    // Past the grace, then four rounds of the doorbell.
    std::thread::sleep(Duration::from_secs(2));
    fake.settle();
    fake.settle();
    assert!(fake.rings().is_empty(), "{:?}", fake.rings());
}

#[test]
fn a_runner_its_machine_never_listed_rings_once_after_a_grace() {
    let fake = Fake::new("bell-unstarted");
    let hub = fake.hub();
    building(
        &fake,
        &hub,
        "proj-alpha-m2",
        &[("proj-alpha-m2", "working")],
    );
    fake.settle();

    // `workflow go` recorded a runner whose orchestrator the mini has never
    // listed: it died before amx made it, or amx never did.
    building(&fake, &hub, "proj-alpha-m2-2", &[]);
    fake.settle();
    assert!(
        fake.rings().is_empty(),
        "rang inside the grace: {:?}",
        fake.rings()
    );
    wait_for(
        "the unstarted stall to ring",
        Duration::from_secs(15),
        || !fake.rings().is_empty(),
    );
    fake.settle();
    fake.settle();
    assert_eq!(fake.rings(), [STALLED]);
}

#[test]
fn an_agent_name_used_again_after_amx_forgot_it_rings_again() {
    let fake = Fake::new("bell-reused");
    let agents = |state: &str, created: u32| {
        format!(
            "[{{\"id\":\"proj-alpha-m2\",\"state\":\"{state}\",\"ended\":0,\"created\":{created}}}]"
        )
    };
    let hub = fake.hub();
    building_with(&fake, &hub, "proj-alpha-m2", &agents("working", 1));
    fake.settle();
    building_with(&fake, &hub, "proj-alpha-m2", &agents("stopped", 1));
    wait_for("the first stall to ring", Duration::from_secs(15), || {
        !fake.rings().is_empty()
    });

    // `amx clear` forgot it, and `workflow go` took the plain name again.
    building_with(&fake, &hub, "proj-alpha-m2", &agents("working", 7));
    fake.settle();
    building_with(&fake, &hub, "proj-alpha-m2", &agents("stopped", 7));
    wait_for("the second stall to ring", Duration::from_secs(15), || {
        fake.rings().len() >= 2
    });
    assert_eq!(fake.rings(), [STALLED, STALLED]);
}
