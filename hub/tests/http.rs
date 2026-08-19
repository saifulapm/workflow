//! H2 — the framing rules and the caps of spec §8, over a real socket.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::{Hub, TempDir, header_of, status_of, wait_for};
use hub::http::{MAX_BODY, MAX_CONNECTIONS, MAX_HEADERS};

/// Every test here wants a hub with a fast IO timeout, so a test that proves a
/// stalled client is dropped does not spend ten seconds proving it.
fn hub(dir: &TempDir) -> Hub {
    let home = dir.join("home");
    Hub::spawn_env(
        &home,
        &[],
        &["--port", "0"],
        &[("HUB_IO_TIMEOUT_MS", "1500")],
    )
}

#[test]
fn an_unknown_path_is_404() {
    let dir = TempDir::new("http-404");
    let hub = hub(&dir);
    assert_eq!(status_of(&hub.get("/nope")), 404);
    assert_eq!(
        status_of(&hub.get("/answer")),
        405,
        "known path, wrong verb"
    );
    assert_eq!(status_of(&hub.get("/")), 200);
}

#[test]
fn a_wrong_method_is_405_and_names_what_is_allowed() {
    let dir = TempDir::new("http-405");
    let hub = hub(&dir);

    let response = hub.get("/answer");
    assert_eq!(status_of(&response), 405);
    assert_eq!(header_of(&response, "Allow"), Some("POST"));

    let response = hub.post_form("/", "x=1");
    assert_eq!(status_of(&response), 405);
    assert_eq!(header_of(&response, "Allow"), Some("GET"));
}

#[test]
fn a_post_without_content_length_is_411() {
    let dir = TempDir::new("http-411");
    let hub = hub(&dir);

    let response = hub.raw(&format!(
        "POST /answer HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        hub.port
    ));
    assert_eq!(status_of(&response), 411);
}

#[test]
fn a_chunked_post_is_411_rather_than_a_hand_rolled_dechunker() {
    let dir = TempDir::new("http-chunked");
    let hub = hub(&dir);

    let response = hub.raw(&format!(
        "POST /answer HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nTransfer-Encoding: chunked\r\n\
         Connection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        hub.port
    ));
    assert_eq!(status_of(&response), 411);
}

#[test]
fn a_body_over_the_cap_is_refused_on_the_header_not_buffered() {
    let dir = TempDir::new("http-413");
    let hub = hub(&dir);

    // The Content-Length claims 1 MiB and the body never arrives. A server that
    // buffered first would sit here until the read timeout; this one answers on
    // the header, so the refusal comes back immediately.
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "POST /answer HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: 1048576\r\n\r\n",
        hub.port
    )
    .unwrap();
    stream.flush().unwrap();

    let started = Instant::now();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(status_of(&response), 413);
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "the refusal waited for a body it had already rejected: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_body_at_the_cap_is_still_accepted() {
    let dir = TempDir::new("http-cap");
    let hub = hub(&dir);

    let filler = "a".repeat(MAX_BODY - "id=X&text=".len());
    let body = format!("id=X&text={filler}");
    assert_eq!(body.len(), MAX_BODY);
    assert_eq!(status_of(&hub.post_form("/answer", &body)), 303);

    let body = format!("{body}a");
    assert_eq!(status_of(&hub.post_form("/answer", &body)), 413);
}

#[test]
fn an_over_long_request_line_is_refused() {
    let dir = TempDir::new("http-line");
    let hub = hub(&dir);

    let long = "a".repeat(9 * 1024);
    let response = hub.raw(&format!(
        "GET /?x={long} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        hub.port
    ));
    assert_eq!(status_of(&response), 431);
}

#[test]
fn too_many_headers_are_refused() {
    let dir = TempDir::new("http-headers");
    let hub = hub(&dir);

    let mut request = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n", hub.port);
    for n in 0..MAX_HEADERS + 5 {
        request.push_str(&format!("X-Pad-{n}: 1\r\n"));
    }
    request.push_str("\r\n");
    assert_eq!(status_of(&hub.raw(&request)), 431);
}

#[test]
fn a_header_block_over_the_byte_cap_is_refused() {
    let dir = TempDir::new("http-headerbytes");
    let hub = hub(&dir);

    // Ten headers, well under the count cap, but 20 KiB of them.
    let mut request = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n", hub.port);
    for n in 0..10 {
        request.push_str(&format!("X-Pad-{n}: {}\r\n", "a".repeat(2 * 1024)));
    }
    request.push_str("\r\n");
    assert_eq!(status_of(&hub.raw(&request)), 431);
}

#[test]
fn a_duplicate_content_length_is_refused() {
    let dir = TempDir::new("http-dup");
    let hub = hub(&dir);

    let response = hub.raw(&format!(
        "POST /answer HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Length: 4\r\n\
         Content-Length: 5\r\nConnection: close\r\n\r\nid=X",
        hub.port
    ));
    assert_eq!(status_of(&response), 400);
}

#[test]
fn a_malformed_request_line_is_400() {
    let dir = TempDir::new("http-garbage");
    let hub = hub(&dir);

    for line in [
        "GET\r\n\r\n",
        "GET / HTTP/9.9\r\n\r\n",
        "get / HTTP/1.1\r\n\r\n",
    ] {
        assert_eq!(status_of(&hub.raw(line)), 400, "{line:?}");
    }
}

#[test]
fn a_stalled_client_is_dropped_and_never_blocks_another() {
    let dir = TempDir::new("http-stall");
    let hub = hub(&dir);

    // A connection that sends half a request line and then nothing.
    let mut stalled = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
    stalled.write_all(b"GET / HTT").unwrap();
    stalled.flush().unwrap();

    // The service is still answering while that one hangs.
    assert_eq!(status_of(&hub.get("/")), 200);

    // And the stalled one is dropped by the read timeout rather than held.
    stalled
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut response = String::new();
    let started = Instant::now();
    stalled.read_to_string(&mut response).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "the stalled connection outlived its timeout"
    );
    assert_eq!(status_of(&response), 408);
}

#[test]
fn past_the_connection_ceiling_hub_sheds_load_instead_of_queueing() {
    let dir = TempDir::new("http-ceiling");
    let hub = hub(&dir);

    // Fill every slot with a connection that has sent nothing.
    let mut held = Vec::new();
    for _ in 0..MAX_CONNECTIONS {
        let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).unwrap();
        stream.write_all(b"G").unwrap();
        stream.flush().unwrap();
        held.push(stream);
    }

    wait_for(
        "a 503 once every slot is taken",
        Duration::from_secs(5),
        || status_of(&hub.get("/")) == 503,
    );

    // The slots free themselves on the read timeout, and hub recovers.
    drop(held);
    wait_for("hub to recover", Duration::from_secs(10), || {
        status_of(&hub.get("/")) == 200
    });
}

#[test]
fn a_response_always_carries_a_length_and_closes() {
    let dir = TempDir::new("http-framing");
    let hub = hub(&dir);

    let response = hub.get("/");
    assert_eq!(header_of(&response, "Connection"), Some("close"));
    let length: usize = header_of(&response, "Content-Length")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(common::body_of(&response).len(), length);
}

#[test]
fn a_connection_that_says_nothing_costs_nothing() {
    let dir = TempDir::new("http-quiet");
    let hub = hub(&dir);

    // A port scan, or a browser's spare socket. Opened and closed, no request.
    for _ in 0..20 {
        drop(TcpStream::connect(("127.0.0.1", hub.port)).unwrap());
    }
    assert_eq!(status_of(&hub.get("/")), 200);
}
