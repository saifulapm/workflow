//! Unbounded time holding a bounded resource — review M-2 and M-3.
//!
//! Both findings are the same shape. `MAX_CONNECTIONS` bounds how many
//! connections may exist and `MemCli`'s gate bounds how many `mem` children may
//! run, but before this round neither had a bound on *how long* one of them
//! could hold its slot. So sixteen dribbling sockets, or one `mem` that did not
//! return, took every route down — including the routes that touch no `mem` at
//! all, because the shed happens before routing. `Restart=always` never fired
//! either: nothing had exited.
//!
//! The doorbell survives both, on its own thread, which is the worst ordering
//! available: the phone buzzes, the link is opened, and it says `503 busy`.

mod common;

use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{Hub, TempDir, body_of, fixture_mem, status_of, wait_for};
use hub::http::MAX_CONNECTIONS;

/// One connection, dribbling one byte at a time, is refused when the *request*
/// runs out of time — not when a read does.
///
/// The socket read timeout is not a bound on anything: every byte resets it. A
/// client sending one byte just inside it holds its connection for
/// `MAX_REQUEST_LINE` × the timeout, which at 8 KiB and ten seconds is about
/// twenty-two hours — on a single header line, so neither `MAX_HEADERS` nor
/// `MAX_HEADER_BYTES` ever ends it either.
#[test]
fn a_connection_that_dribbles_is_refused_once_the_request_runs_out_of_time() {
    let dir = TempDir::new("dribble-one");
    let home = dir.join("home");
    let hub = Hub::spawn_env(
        &home,
        &[],
        &["--port", "0"],
        &[
            // Deliberately generous, and deliberately longer than the deadline:
            // this is the timeout the dribble used to reset.
            ("HUB_IO_TIMEOUT_MS", "30000"),
            ("HUB_REQUEST_DEADLINE_MS", "1500"),
        ],
    );

    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    stream
        .write_all(format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n", hub.port).as_bytes())
        .unwrap();
    stream.flush().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let dribbler = {
        let stop = Arc::clone(&stop);
        let mut writer = stream.try_clone().unwrap();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if writer.write_all(b"p").is_err() || writer.flush().is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let started = Instant::now();
    let mut response = String::new();
    let _ = std::io::Read::read_to_string(&mut stream, &mut response);
    stop.store(true, Ordering::Relaxed);
    let _ = dribbler.join();

    assert_eq!(
        status_of(&response),
        408,
        "a dribbling client was not refused: {response:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it was held for {:?}, well past the deadline it was given",
        started.elapsed()
    );
}

/// And the service-level half: every slot held at once no longer takes hub down
/// permanently. Before the deadline, sixteen of these killed every route —
/// including `/subscribe` and 404s, which touch no `mem` at all, because the
/// shed happens before routing.
#[test]
fn clients_that_dribble_for_ever_cannot_hold_the_service_down() {
    let dir = TempDir::new("slowloris");
    let home = dir.join("home");
    let hub = Hub::spawn_env(
        &home,
        &[],
        &["--port", "0"],
        &[
            ("HUB_IO_TIMEOUT_MS", "30000"),
            // Long enough that "every slot is taken" is comfortably observable
            // before the deadline starts handing them back.
            ("HUB_REQUEST_DEADLINE_MS", "8000"),
        ],
    );
    let port = hub.port;

    let stop = Arc::new(AtomicBool::new(false));
    // Every dribbler is connected and holding *before* anything probes. A probe
    // that lands mid-setup takes the last free slot itself, hub sheds the
    // dribbler that was about to fill it, and the ceiling is then never reached
    // again — the connection count sits one short for the rest of the run.
    let (ready, connected) = std::sync::mpsc::channel();
    let dribblers: Vec<_> = (0..MAX_CONNECTIONS)
        .map(|_| {
            let stop = Arc::clone(&stop);
            let ready = ready.clone();
            std::thread::spawn(move || {
                let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
                    return;
                };
                let _ = stream
                    .write_all(format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n").as_bytes());
                let _ = stream.flush();
                let _ = ready.send(());
                while !stop.load(Ordering::Relaxed) {
                    // One byte, no newline: neither `MAX_HEADERS` nor
                    // `MAX_HEADER_BYTES` can end this, and at 8 KiB of request
                    // line it would take weeks to reach the cap.
                    if stream.write_all(b"p").is_err() || stream.flush().is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })
        })
        .collect();
    drop(ready);
    for _ in 0..MAX_CONNECTIONS {
        connected
            .recv_timeout(Duration::from_secs(20))
            .expect("every dribbler to connect");
    }

    let started = Instant::now();
    // They are holding every slot, so an honest request is shed.
    wait_for("every slot to be taken", Duration::from_secs(30), || {
        status_of(&hub.get("/")) == 503
    });

    // And hub comes back on its own, by taking the connections away — the
    // clients never stop dribbling and never close anything.
    wait_for(
        "hub to serve again with the dribblers still going",
        Duration::from_secs(120),
        || status_of(&hub.get("/")) == 200,
    );
    assert!(
        started.elapsed() >= Duration::from_secs(4),
        "the slots came back in {:?}, so the dribblers never really held them",
        started.elapsed()
    );

    stop.store(true, Ordering::Relaxed);
    for dribbler in dribblers {
        let _ = dribbler.join();
    }
}

/// A `mem` that never returns is a degraded page, not a dead service.
///
/// Every data route queues on one gate, so before this round a single stuck
/// child blocked `/`, all three API routes and `POST /answer` — and then each
/// of those requests held a connection permit for ever, which is M-2 again by
/// another road: sixteen page loads, four minutes of the page's own refresh,
/// and the whole service was shed.
#[test]
fn a_mem_that_never_returns_does_not_take_every_route_with_it() {
    let dir = TempDir::new("slow-mem");
    let home = dir.join("home");
    let bin = dir.join("bin");
    // Slow on the verb the page needs first, instant on the rest — so a route
    // that queues behind it is queueing for no reason of its own.
    fixture_mem(
        &bin,
        "if [ \"$1\" = questions ]; then sleep 300; fi\n\
         if [ \"$1\" = projects ]; then echo '{\"projects\":[]}'; exit 0; fi\n\
         exit 1",
    );
    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["--port", "0"],
        &[
            ("HUB_MEM_TIMEOUT_MS", "500"),
            // One poll at startup and no more, so the doorbell is not the thing
            // holding the gate when the page asks for it.
            ("HUB_POLL_MS", "600000"),
        ],
    );

    let started = Instant::now();
    let response = hub.get("/");
    assert_eq!(
        status_of(&response),
        200,
        "a mem that will not answer is what §4a's last row is for"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the render outlived every timeout it was given: {:?}",
        started.elapsed()
    );
    assert!(
        body_of(&response).contains("mem is not answering"),
        "and the page says so: {}",
        body_of(&response)
    );

    // The routes that never needed mem are untouched.
    assert_eq!(status_of(&hub.get("/subscribe")), 200);
    assert_eq!(status_of(&hub.get("/nope")), 404);

    // And the requests behind the killed child do not each wait for a timeout
    // of their own — which is what would turn a slow store into a shed service.
    let started = Instant::now();
    assert_eq!(status_of(&hub.get("/")), 200);
    assert_eq!(status_of(&hub.get("/api/questions")), 200);
    assert_eq!(status_of(&hub.get("/api/activity")), 200);
    assert_eq!(status_of(&hub.get("/api/projects")), 200);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "four routes queued for a timeout apiece: {:?}",
        started.elapsed()
    );

    // Still serving after all of that, with every slot free.
    assert_eq!(status_of(&hub.get("/")), 200);
}

/// The child is killed, not merely abandoned.
#[test]
fn a_mem_that_overruns_is_killed_rather_than_left_running() {
    let dir = TempDir::new("slow-mem-kill");
    let home = dir.join("home");
    let bin = dir.join("bin");
    let marker = dir.join("still-alive");
    // Sleeps past its ceiling, then leaves a file. If the file exists, the
    // child outlived the call that gave up on it.
    fixture_mem(
        &bin,
        &format!(
            "if [ \"$1\" = questions ]; then sleep 3; : > '{marker}'; fi\n\
             exit 1",
            marker = marker.display()
        ),
    );
    let hub = Hub::spawn_env(
        &home,
        &[&bin],
        &["--port", "0"],
        &[
            ("HUB_MEM_TIMEOUT_MS", "300"),
            ("HUB_POLL_MS", "600000"),
            // Short enough that the breaker is not still open when we look.
            ("HUB_IO_TIMEOUT_MS", "10000"),
        ],
    );

    assert_eq!(status_of(&hub.get("/")), 200);
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        !marker.exists(),
        "the child ran on after hub stopped waiting for it"
    );
}
