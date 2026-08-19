//! A hand-rolled HTTP/1.1 server for four routes (spec §8).
//!
//! There is no server crate here on purpose: §8's ruling is that v1 adds no
//! crates, and `tiny_http` would still have left the form decoder, the query
//! decoder and the body cap to write by hand (review M-7). What it would have
//! given away for free is the part that bites — the caps — so they are named
//! constants with tests rather than an afterthought.
//!
//! Every connection serves exactly one request and closes. Keep-alive would buy
//! a phone one round trip and cost the accept loop a way to be held open.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::form::{Form, decode_path};

/// The request line holds a URL a browser built; 8 KiB is far past any of ours.
pub const MAX_REQUEST_LINE: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// §8: a 64 KiB body cap. A batched answer is a few hundred bytes.
pub const MAX_BODY: usize = 64 * 1024;
/// A phone that walks out of wifi mid-request must not hold a thread for ever.
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// The read and write timeout on every socket. `HUB_IO_TIMEOUT_MS` overrides
/// it — a test seam in mem's style (`MEM_POLL_MS`), so the suite can prove the
/// timeout fires without spending ten seconds doing it.
pub fn io_timeout() -> Duration {
    static TIMEOUT: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        std::env::var("HUB_IO_TIMEOUT_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_IO_TIMEOUT)
    })
}
/// How long a whole request has to arrive, headers and body together.
///
/// The per-read timeout above is not a bound on anything. It is reset by every
/// byte, so a client that sends one header line just inside it holds its slot
/// for as long as it likes: at `MAX_REQUEST_LINE` of 8 KiB, one byte every nine
/// seconds is about twenty-two hours on a single connection, and sixteen of
/// those took every route down — including the ones that touch no `mem` at all,
/// because the shed happens before routing. Meanwhile the doorbell is on its
/// own thread and kept ringing, so the phone buzzed, Saiful opened the link,
/// and got `503 busy` (review M-2).
pub const DEFAULT_REQUEST_DEADLINE: Duration = Duration::from_secs(15);

/// 15 s, or `HUB_REQUEST_DEADLINE_MS` — the seam the slowloris test uses.
pub fn request_deadline() -> Duration {
    static DEADLINE: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *DEADLINE.get_or_init(|| {
        std::env::var("HUB_REQUEST_DEADLINE_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_REQUEST_DEADLINE)
    })
}

/// Bounded, so a slow client cannot starve the doorbell thread of anything —
/// including memory. Sixty-four rather than sixteen: a thread is not the scarce
/// resource here and sixteen is not many — Safari opens several connections per
/// page and the page reloads itself every fifteen seconds — so the ceiling
/// wants to sit well above the number of real clients while the deadline above
/// does the actual bounding (review M-2).
pub const MAX_CONNECTIONS: usize = 64;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The raw request target, query string and all.
    pub target: String,
    /// The path, percent-decoded, without the query.
    pub path: String,
    pub query: Form,
    /// Header names are lowercased; values are trimmed.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first value for `name`. Duplicates of the headers that decide
    /// framing are rejected at parse time, so "first" is unambiguous where it
    /// matters.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn form(&self) -> Form {
        Form::parse(&String::from_utf8_lossy(&self.body))
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            content_type: content_type.to_string(),
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "text/html; charset=utf-8", body)
    }

    pub fn json(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "application/json", body)
    }

    pub fn text(status: u16, body: &str) -> Response {
        Response::new(status, "text/plain; charset=utf-8", format!("{body}\n"))
    }

    /// 303, the redirect that turns a POST into a GET — which is the whole
    /// point of the answer path: a reload must not re-answer.
    pub fn see_other(location: &str) -> Response {
        let mut response = Response::text(303, "see /");
        response
            .headers
            .push(("Location".to_string(), location.to_string()));
        response
    }

    pub fn not_found() -> Response {
        Response::text(404, "not found")
    }

    pub fn method_not_allowed(allow: &str) -> Response {
        let mut response = Response::text(405, "method not allowed");
        response
            .headers
            .push(("Allow".to_string(), allow.to_string()));
        response
    }

    pub fn header(mut self, name: &str, value: &str) -> Response {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// A request hub refused before any handler saw it: a status and one line of
/// plain text. Nothing here echoes anything the client sent.
struct Refusal {
    status: u16,
    message: &'static str,
}

impl Refusal {
    fn new(status: u16, message: &'static str) -> Refusal {
        Refusal { status, message }
    }
}

/// Serves until the process is killed. Each connection gets its own thread,
/// with a hard ceiling on how many can exist at once.
pub fn serve<H>(listener: TcpListener, handler: H) -> !
where
    H: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let live = Arc::new(AtomicUsize::new(0));

    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) => {
                // A failed accept is one client's problem, never the service's.
                eprintln!("hub: accept: {e}");
                continue;
            }
        };

        if live.load(Ordering::Acquire) >= MAX_CONNECTIONS {
            // Answered on the accept thread, in one write: this path exists to
            // shed load, so it must not cost a thread of its own.
            let mut stream = stream;
            let _ = stream.set_write_timeout(Some(io_timeout()));
            let _ = write_response(&mut stream, &Response::text(503, "busy"));
            continue;
        }

        live.fetch_add(1, Ordering::AcqRel);
        let handler = Arc::clone(&handler);
        let live_for_thread = Arc::clone(&live);
        let spawned = std::thread::Builder::new()
            .name("hub-conn".to_string())
            .spawn(move || {
                // Decrements even if the handler panics, so one bad request
                // cannot leak a permit and wedge the ceiling shut.
                let _guard = Permit(live_for_thread);
                handle(stream, handler.as_ref());
            });
        if let Err(e) = spawned {
            live.fetch_sub(1, Ordering::AcqRel);
            eprintln!("hub: spawn: {e}");
        }
    }
}

struct Permit(Arc<AtomicUsize>);

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle<H>(stream: TcpStream, handler: &H)
where
    H: Fn(&Request) -> Response,
{
    if stream.set_read_timeout(Some(io_timeout())).is_err()
        || stream.set_write_timeout(Some(io_timeout())).is_err()
    {
        return;
    }
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    let deadline = Deadline::new();

    let response = match read_request(&mut reader, &mut writer, &deadline) {
        Ok(None) => return,
        Ok(Some(request)) => handler(&request),
        Err(refusal) => Response::text(refusal.status, refusal.message),
    };
    let _ = write_response(&mut writer, &response);
    let _ = writer.flush();
    let _ = writer.shutdown(std::net::Shutdown::Both);
}

/// The clock one whole request runs against.
struct Deadline(Instant);

impl Deadline {
    fn new() -> Deadline {
        Deadline(Instant::now() + request_deadline())
    }

    /// How long the next read may block: never past the deadline, and never
    /// longer than the socket timeout it would have had anyway. `None` means
    /// the request has run out of time.
    fn budget(&self) -> Option<Duration> {
        let left = self.0.checked_duration_since(Instant::now())?;
        (!left.is_zero()).then(|| left.min(io_timeout()))
    }

    /// Arms the socket for one read. Re-armed before *every* read, which is the
    /// point: a single `set_read_timeout` at the top of the connection is what
    /// a dribbling client resets for ever.
    fn arm(&self, reader: &BufReader<TcpStream>) -> Result<(), Refusal> {
        let Some(budget) = self.budget() else {
            return Err(Refusal::new(408, "timed out reading the request"));
        };
        let _ = reader.get_ref().set_read_timeout(Some(budget));
        Ok(())
    }
}

/// Reads one request. `Ok(None)` is a connection that closed without sending
/// anything — a port scan, or a browser opening a spare socket it never uses.
fn read_request(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
    deadline: &Deadline,
) -> Result<Option<Request>, Refusal> {
    let Some(line) = read_line(reader, MAX_REQUEST_LINE, deadline)? else {
        return Ok(None);
    };
    if line.is_empty() {
        return Ok(None);
    }

    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(Refusal::new(400, "malformed request line"));
    };
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(Refusal::new(400, "malformed request line"));
    }
    if !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(Refusal::new(400, "malformed method"));
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let Some(line) = read_line(reader, MAX_REQUEST_LINE, deadline)? else {
            return Err(Refusal::new(400, "headers ended early"));
        };
        if line.is_empty() {
            break;
        }
        header_bytes += line.len() + 2;
        if header_bytes > MAX_HEADER_BYTES {
            return Err(Refusal::new(431, "headers too large"));
        }
        if headers.len() >= MAX_HEADERS {
            return Err(Refusal::new(431, "too many headers"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(Refusal::new(400, "malformed header"));
        };
        let name = name.to_ascii_lowercase();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(Refusal::new(400, "malformed header"));
        }
        // Two Content-Lengths or two Hosts is how a request smuggles a second
        // request. hub never proxies, but the answer is still no.
        if (name == "content-length" || name == "host" || name == "transfer-encoding")
            && headers.iter().any(|(k, _)| *k == name)
        {
            return Err(Refusal::new(400, "duplicate framing header"));
        }
        headers.push((name, value.trim().to_string()));
    }

    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    let body = if method == "POST" || method == "PUT" || method == "PATCH" {
        // §8: `Content-Length` required, chunked rejected with 411. Nothing
        // hub speaks to needs chunked, and a hand-rolled dechunker on the one
        // path that runs `mem answer` is not a trade worth making.
        if header("transfer-encoding").is_some() {
            return Err(Refusal::new(411, "chunked bodies are not accepted"));
        }
        let Some(length) = header("content-length") else {
            return Err(Refusal::new(411, "content-length required"));
        };
        let Ok(length) = length.trim().parse::<usize>() else {
            return Err(Refusal::new(400, "malformed content-length"));
        };
        if length > MAX_BODY {
            // Refused on the header, before a byte of it is buffered.
            return Err(Refusal::new(413, "body too large"));
        }
        // curl sends `Expect: 100-continue` for bodies past a kilobyte and
        // stalls for a second waiting for this; AC4 answers with curl.
        if header("expect").is_some_and(|e| e.eq_ignore_ascii_case("100-continue")) {
            let _ = writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
            let _ = writer.flush();
        }
        read_body(reader, length, deadline)?
    } else {
        Vec::new()
    };

    Ok(Some(Request {
        method: method.to_string(),
        target: target.to_string(),
        path: decode_path(raw_path),
        query: Form::parse(raw_query),
        headers,
        body,
    }))
}

/// Exactly `length` bytes of body, against the same deadline as the headers.
///
/// `read_exact` would not do: it loops on `read` internally, and each of those
/// reads gets the socket timeout afresh — so a client dribbling its body resets
/// the clock just as a client dribbling its headers does.
fn read_body(
    reader: &mut BufReader<TcpStream>,
    length: usize,
    deadline: &Deadline,
) -> Result<Vec<u8>, Refusal> {
    let mut body = vec![0u8; length];
    let mut filled = 0;
    while filled < length {
        deadline.arm(reader)?;
        match reader.read(&mut body[filled..]) {
            Ok(0) => return Err(Refusal::new(400, "body shorter than content-length")),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(Refusal::new(408, "timed out reading the request"));
            }
            Err(_) => return Err(Refusal::new(400, "body shorter than content-length")),
        }
    }
    Ok(body)
}

/// One CRLF-terminated line, capped. A line that reaches the cap without a
/// newline is the refusal, not a truncated line handed on as if it were whole.
fn read_line(
    reader: &mut BufReader<TcpStream>,
    limit: usize,
    deadline: &Deadline,
) -> Result<Option<String>, Refusal> {
    let mut buf = Vec::new();
    loop {
        // Re-armed on every pass, not once per line. `read_until` loops on
        // `fill_buf` internally and each of those reads gets the socket timeout
        // afresh, so a client sending one *byte* just inside it would hold the
        // connection for `limit` × the timeout — hours, on one line, which is
        // the shape of M-2 that arming per line does not close.
        deadline.arm(reader)?;
        let (chunk, newline_at) = {
            let available = match reader.fill_buf() {
                Ok(available) => available,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Refusal::new(408, "timed out reading the request")),
            };
            if available.is_empty() {
                // A connection that closed without sending anything is a port
                // scan or a spare socket, and not an error. One that closed
                // mid-line never sent a line at all.
                if buf.is_empty() {
                    return Ok(None);
                }
                return Err(Refusal::new(431, "request line or header too long"));
            }
            let newline_at = available.iter().position(|&b| b == b'\n');
            let take = newline_at.unwrap_or(available.len());
            buf.extend_from_slice(&available[..take]);
            (available.len(), newline_at)
        };
        if buf.len() > limit {
            return Err(Refusal::new(431, "request line or header too long"));
        }
        match newline_at {
            Some(at) => {
                reader.consume(at + 1);
                break;
            }
            None => reader.consume(chunk),
        }
    }
    if buf.ends_with(b"\r") {
        buf.pop();
    }
    String::from_utf8(buf).map(Some).map_err(|_| {
        // Header bytes outside ASCII are not something a browser sends and not
        // something hub has a use for.
        Refusal::new(400, "non-utf8 request")
    })
}

fn write_response(stream: &mut TcpStream, response: &Response) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason(response.status),
        sanitise(&response.content_type),
        response.body.len(),
    );
    for (name, value) in &response.headers {
        // A CR or LF smuggled into a header value would inject a header, or a
        // whole response. Nothing here should carry one; strip it anyway.
        head.push_str(&format!("{}: {}\r\n", sanitise(name), sanitise(value)));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

fn sanitise(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_values_cannot_smuggle_a_second_header() {
        assert_eq!(sanitise("ok\r\nX-Evil: 1"), "okX-Evil: 1");
    }

    #[test]
    fn every_status_hub_emits_has_a_reason_phrase() {
        for status in [200, 303, 400, 403, 404, 405, 411, 413, 431, 503] {
            assert_ne!(reason(status), "Error", "{status} has no reason phrase");
        }
    }
}
