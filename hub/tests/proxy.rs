//! The CONNECT tunnel a cloud VM's sockets go through.

use std::net::TcpListener;
use std::time::{Duration, Instant};

/// A proxy that takes the connection and never answers must not hold the
/// socket's thread for good.
#[test]
fn a_proxy_that_never_answers_gives_up_in_time() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = format!("http://{}", proxy.local_addr().unwrap());
    std::thread::spawn(move || {
        let (held, _) = proxy.accept().unwrap();
        std::thread::sleep(Duration::from_secs(10));
        drop(held);
    });
    let start = Instant::now();
    let tried = hub::proxy::connect_within(
        "wss://store.example.test/store/share",
        Some(&at),
        Duration::from_millis(500),
    );
    assert!(tried.is_err());
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
}
