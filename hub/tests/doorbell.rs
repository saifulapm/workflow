//! H6 — the doorbell (spec §5, AC2, AC5).
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
    Hub, TempDir, fixture_bin, fixture_mem, invocations, mem_in, real_mem, seed_project, status_of,
    wait_for,
};

struct World {
    dir: TempDir,
    home: PathBuf,
    bin: PathBuf,
    curl_log: PathBuf,
    /// One line per ring: `yes` if the id was already in the seen file when
    /// curl ran, `no` if it was not.
    order_log: PathBuf,
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
        let bin = dir.join("bin");
        let mem = real_mem().expect("build mem first");

        // The real mem, against a throwaway store.
        fixture_bin(
            &bin,
            "mem",
            &format!(
                "export XDG_DATA_HOME='{home}/data' XDG_CACHE_HOME='{home}/cache'\n\
                 export XDG_STATE_HOME='{home}/state' XDG_CONFIG_HOME='{home}/config'\n\
                 export MEM_SYNC_CMD=true MEM_NOTIFY_CMD=true\n\
                 exec '{mem}' \"$@\"",
                home = home.display(),
                mem = mem.display(),
            ),
        );

        let curl_log = dir.join("curl.log");
        let order_log = dir.join("order.log");
        let seen = home.join("state/hub/seen");
        // Records the argv, then whether the id being rung for was already
        // durable — which is the ordering §5 is specific about.
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
            config,
            mem,
        }
    }

    fn hub(&self) -> Hub {
        Hub::spawn_env(
            &self.home,
            &[&self.bin],
            &["--config", self.config.to_str().unwrap(), "--port", "0"],
            // A 15 s poll would make every test here take a minute.
            &[("HUB_POLL_MS", "120")],
        )
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

    fn seen(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.join("state/hub/seen"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Waits for the doorbell to have gone round at least twice with nothing new.
fn settle() {
    std::thread::sleep(Duration::from_millis(400));
}

#[test]
fn ac2_a_new_question_rings_exactly_once_however_many_times_it_is_polled() {
    let world = World::new("bell-once", "http://127.0.0.1:9");
    let hub = world.hub();
    // First start with an empty queue: seeding has nothing to record.
    settle();
    assert!(world.rings().is_empty());

    world.ask("Should we use Redis?");
    wait_for("the doorbell to ring", Duration::from_secs(5), || {
        !world.rings().is_empty()
    });

    // Several more polls go by; the id is in the seen file, so nothing rings.
    settle();
    settle();
    assert_eq!(world.rings().len(), 1, "{:?}", world.rings());
    assert_eq!(world.seen().len(), 1);

    // The service is still serving, and the question is on the page.
    assert_eq!(status_of(&hub.get("/")), 200);
}

#[test]
fn the_ring_carries_no_question_text_and_never_touches_a_shell() {
    let world = World::new("bell-body", "http://127.0.0.1:9");
    let _hub = world.hub();
    settle();
    world.ask("Should we deploy the thing that must not be named?");
    wait_for("the doorbell to ring", Duration::from_secs(5), || {
        !world.rings().is_empty()
    });

    let rings = world.rings();
    assert_eq!(rings.len(), 1, "{rings:?}");
    let argv = &rings[0];
    // §5's own command line, argument for argument.
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
    settle();
    world.ask("Should we use Redis?");
    wait_for("the doorbell to ring", Duration::from_secs(5), || {
        !world.rings().is_empty()
    });

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
    settle();
    world.ask("Should we use Redis?");
    wait_for("the first ring", Duration::from_secs(5), || {
        world.rings().len() == 1
    });
    let seen_before = world.seen();

    // Exactly what `systemctl --user restart` does.
    drop(hub);
    let hub = world.hub();
    settle();
    settle();

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
    settle();
    settle();

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

/// A question that arrives by sync — born on another machine — is recorded but
/// not rung: the machine that asked already rang the phone, up to a sync round
/// earlier. Ringing again here is the same buzz twice, minutes apart, with a
/// link to the wrong hub.
#[test]
fn a_question_from_another_machine_is_recorded_and_not_rung() {
    let world = World::new("bell-foreign", "http://127.0.0.1:9");
    let machine_file = world.home.join("config/qshell/machine");
    std::fs::create_dir_all(machine_file.parent().unwrap()).unwrap();
    std::fs::write(&machine_file, "here-hub").unwrap();

    let _hub = world.hub();
    settle(); // the seeding round, on an empty queue

    // The sibling's question lands in the store, the way bisync would land it:
    // mem reads the machine file per invocation, hub read it once at start.
    std::fs::write(&machine_file, "far-nuc").unwrap();
    world.ask("Should we use Redis?");
    wait_for("the id to be recorded", Duration::from_secs(5), || {
        world.seen().len() == 1
    });
    settle();
    assert!(
        world.rings().is_empty(),
        "a synced-in question is the sibling's doorbell, not ours: {:?}",
        world.rings()
    );

    // A question asked on this machine still rings.
    std::fs::write(&machine_file, "here-hub").unwrap();
    world.ask("And Postgres over MySQL?");
    wait_for("the local question to ring", Duration::from_secs(5), || {
        world.rings().len() == 1
    });
    assert_eq!(world.seen().len(), 2, "both recorded, one rung");
}

#[test]
fn an_unreachable_ntfy_keeps_the_service_serving() {
    // Real curl, and a port with nothing on it: curl fails, hub carries on.
    let world = World::new("bell-unreachable", "http://127.0.0.1:9");
    std::fs::remove_file(world.bin.join("curl")).unwrap();
    let hub = world.hub();
    settle();
    world.ask("Should we use Redis?");
    settle();
    settle();

    assert_eq!(status_of(&hub.get("/")), 200);
    assert_eq!(status_of(&hub.get("/api/questions")), 200);
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
    settle();
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
    settle();
    world.ask("Should we use Redis?");
    settle();
    settle();

    assert!(
        world.rings().is_empty(),
        "an id that could not be recorded must not be rung: {:?}",
        world.rings()
    );
    assert_eq!(status_of(&hub.get("/")), 200, "and the page still renders");
}

/// review M-1: one empty-stdout poll at first start used to throw the seeding
/// guard away and ring for the whole backlog.
///
/// The gate was `outcome.broken()`, which is `None` for `Outcome::Absent`, so
/// §4a's "exit non-zero, empty stdout" row fell straight through it: the loop
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
    let seen = || {
        std::fs::read_to_string(home.join("state/hub/seen"))
            .unwrap_or_default()
            .lines()
            .count()
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
        std::fs::read_to_string(&polls)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
            > 1,
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
    settle();
    world.ask("Should we use Redis?");
    wait_for("the doorbell to ring", Duration::from_secs(5), || {
        !world.rings().is_empty()
    });

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
    settle();
    world.ask("Should we use Redis?");
    wait_for("the doorbell to ring", Duration::from_secs(5), || {
        !world.rings().is_empty()
    });

    let body = world.rings()[0][4].clone();
    assert!(body.contains("http://chosen.example:8787/"), "{body}");
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
