//! The request a page reads and the response it gives, with no server
//! behind them: the hub's own server, or a Worker, fills one in and sends the
//! other.

use crate::form::Form;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The path, percent-decoded, without the query.
    pub path: String,
    pub query: Form,
    /// Header names are lowercased; values are trimmed.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The address the connection came from, for the journal.
    pub peer: String,
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
