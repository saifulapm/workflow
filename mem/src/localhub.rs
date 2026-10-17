//! This machine's local hub, on its unix socket: `mem ask` hands it the
//! question, and `mem questions --wait` asks it for the answer, which reaches
//! the local hub before it reaches any store. With no local hub, both carry on
//! as before.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

/// How long one long poll may be held, in seconds.
const HOLD_SECS: u64 = 25;

/// `HUB_LOCAL_SOCKET`, else `hub/local.sock` under the state directory: the
/// hub's own rule.
pub fn socket_path(state: &Path) -> PathBuf {
    match std::env::var_os("HUB_LOCAL_SOCKET").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => state.join("hub/local.sock"),
    }
}

/// The question as the local hub needs it.
pub struct Asked<'a> {
    pub id: &'a str,
    pub project: Option<&'a str>,
    pub text: &'a str,
    pub options: &'a [String],
    pub recommend: Option<&'a str>,
    pub audience: Option<&'a str>,
}

/// Hands the question over. Best effort: the question is already written,
/// and a machine with no local hub still has the hub web.
pub fn ask(socket: &Path, asked: &Asked) {
    let body = json!({
        "id": asked.id,
        "project": asked.project,
        "text": asked.text,
        "options": asked.options,
        "recommend": asked.recommend,
        "audience": asked.audience.unwrap_or("human"),
    })
    .to_string();
    if let Err(e) = call(
        socket,
        "POST",
        "/ask",
        body.as_bytes(),
        Duration::from_secs(5),
    ) {
        // No socket is the usual case on a machine without a local hub.
        if e.kind() != std::io::ErrorKind::NotFound
            && e.kind() != std::io::ErrorKind::ConnectionRefused
        {
            eprintln!("mem: the local hub did not take the question: {e}");
        }
    }
}

pub enum Waited {
    Answered(String),
    /// Nothing within the time given.
    Nothing,
    /// No local hub to ask.
    NoHub,
}

/// Asks the local hub for the answer to `short_id`, holding for at most
/// `within`.
pub fn wait(socket: &Path, short_id: &str, within: Duration) -> Waited {
    let secs = within.as_secs().clamp(1, HOLD_SECS);
    let path = format!("/wait/{short_id}?s={secs}");
    match call(socket, "GET", &path, b"", Duration::from_secs(secs + 10)) {
        Ok((200, body)) => Waited::Answered(body),
        Ok(_) => Waited::Nothing,
        Err(_) => Waited::NoHub,
    }
}

/// Waits on the local hub alone until `timeout`, for a machine whose store
/// cannot be waited on (a cloud session's remote one).
pub fn wait_until(socket: &Path, short_id: &str, timeout: Duration) -> Waited {
    let started = Instant::now();
    loop {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Waited::Nothing;
        }
        match wait(socket, short_id, left) {
            Waited::Nothing => continue,
            other => return other,
        }
    }
}

/// Tells the hub the sharing outbox has a new write. Never waits long: the
/// write is already safe in the outbox.
pub fn kick(socket: &Path) {
    let _ = call(
        socket,
        "POST",
        "/share/kick",
        b"",
        Duration::from_millis(200),
    );
}

/// The short id a question is known by on the local hub: the last eight
/// characters of whatever form it was given in.
pub fn short_id(id: &str) -> String {
    let id = id.trim().trim_start_matches('#').to_ascii_uppercase();
    let skip = id.chars().count().saturating_sub(8);
    id.chars().skip(skip).collect()
}

/// A shared project's file, fetched by the hub from the project's files
/// store; None when the hub is not there or has no such file.
pub fn file(socket: &Path, rel: &str) -> Option<Vec<u8>> {
    let query: String = rel
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let path = format!("/share/file?path={query}");
    match call_bytes(socket, "GET", &path, b"", Duration::from_secs(60)) {
        Ok((200, bytes)) => Some(bytes),
        _ => None,
    }
}

pub fn call(
    socket: &Path,
    method: &str,
    path: &str,
    body: &[u8],
    timeout: Duration,
) -> std::io::Result<(u16, String)> {
    let (code, bytes) = call_bytes(socket, method, path, body, timeout)?;
    Ok((code, String::from_utf8_lossy(&bytes).into_owned()))
}

fn call_bytes(
    socket: &Path,
    method: &str,
    path: &str,
    body: &[u8],
    timeout: Duration,
) -> std::io::Result<(u16, Vec<u8>)> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: local\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    let code = status
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| std::io::Error::other("no status line"))?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
    }
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok((code, bytes))
}

#[cfg(test)]
mod tests {
    use super::short_id;

    #[test]
    fn a_short_id_is_the_last_eight_of_any_form() {
        assert_eq!(short_id("#2hqmhmrz"), "2HQMHMRZ");
        assert_eq!(short_id("01M51FJ6AYQ3Y270YPR69MDKTT"), "R69MDKTT");
    }
}
