//! Who is allowed to POST an answer (spec §9.2).
//!
//! The tailnet is a *network* boundary and a browser is not bound by it. A form
//! POST of `application/x-www-form-urlencoded` is a CORS-simple request: any
//! page open in Safari on the phone — itself a tailnet node — can silently
//! submit `<form action="http://macbook:8787/answer" method="post">` with an
//! attacker-chosen id and text, and never needs to read the response, because
//! the *write* was the payload (review B-4a).
//!
//! Two gates, and a request has to pass both:
//!
//! 1. **Host.** Checked on every route, not just the write, because a DNS
//!    rebinding attack that reaches `GET /` reads the question text too.
//! 2. **Origin.** `Sec-Fetch-Site: same-origin`, or an `Origin`/`Referer` that
//!    is one of ours. Anything else is 403 with no `mem answer` run.

use crate::config::Config;

/// The names this hub answers to, and the port it is really on.
pub struct Guard {
    port: u16,
    /// Lowercased: the machine-name file's value and `uname -n`, which on this
    /// machine are two different strings (review m-10).
    names: Vec<String>,
    /// `host[:port]` for every entry in the config's `origins`.
    extra: Vec<String>,
}

impl Guard {
    pub fn new(config: &Config, port: u16, machine: &str) -> Guard {
        let mut names = vec![machine.to_ascii_lowercase()];
        if let Ok(out) = std::process::Command::new("uname").arg("-n").output()
            && out.status.success()
        {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        let extra = config
            .origins
            .iter()
            .filter_map(|origin| authority(origin))
            .collect();
        Guard { port, names, extra }
    }

    /// Is this `Host:` one of ours?
    ///
    /// Four ways to be, and they are deliberately written out rather than
    /// merged, because each is a different argument:
    ///
    /// - **loopback literals**, which is what hub actually binds;
    /// - **a name with no dots**, which is what MagicDNS short names are. hub
    ///   cannot know its own tailnet name without asking tailscale, and a
    ///   single-label name is not something a web attacker can register — the
    ///   rebinding they would need starts with a domain they control;
    /// - **anything under `.ts.net`**, which is §9's "this machine's `*.ts.net`
    ///   name", generalised for the same reason: Tailscale controls those
    ///   records, and none of them can be pointed at 127.0.0.1 by an outsider;
    /// - **whatever the config's `origins` names**, which is how this is
    ///   narrowed to the exact two hostnames if that is ever wanted.
    ///
    /// A port, when the header carries one, must be the port hub actually bound.
    pub fn host_allowed(&self, host: Option<&str>) -> bool {
        let Some(host) = host else {
            // HTTP/1.1 requires it. No Host, no answer.
            return false;
        };
        let host = host.trim().to_ascii_lowercase();
        let Some((name, port)) = split_authority(&host) else {
            return false;
        };
        if let Some(port) = port
            && port != self.port
        {
            return false;
        }
        if matches!(name, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
            return true;
        }
        if self.extra.iter().any(|allowed| allowed == &host) {
            return true;
        }
        // A name at all: not empty, nothing but hostname characters, and no
        // empty label. Without this an empty `Host:` has no dots in it and
        // would fall through the single-label rule below.
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
            || name.split('.').any(str::is_empty)
        {
            return false;
        }
        if self.names.iter().any(|known| known == name) {
            return true;
        }
        !name.contains('.') || name.ends_with(".ts.net")
    }

    /// One of hub's own origins — the test both `Origin` and `Referer` face.
    pub fn origin_allowed(&self, origin: &str) -> bool {
        match authority(origin) {
            Some(authority) => self.host_allowed(Some(&authority)),
            None => false,
        }
    }

    /// §9.2, in full. `Sec-Fetch-Site: same-origin` is enough on its own;
    /// otherwise an `Origin` or a `Referer` has to name one of ours. A request
    /// carrying none of the three is refused — which includes a bare `curl`,
    /// and is why `hub/TESTING.md` gives AC4's curl an `-H Origin:`.
    pub fn may_write(&self, headers: &impl HeaderSource) -> bool {
        if let Some(site) = headers.header("sec-fetch-site") {
            // Defence in depth (review m-1): an explicit claim of anything but
            // `same-origin` used to be overridden by the first `Origin` header
            // that followed it, so `cross-site` plus an `Origin` of ours wrote.
            // No browser can produce that contradiction — `Origin` on a
            // cross-site POST is the initiating origin and cannot be forged —
            // but a header the client sent about itself should not be read only
            // when it agrees with us.
            return site == "same-origin";
        }
        if let Some(origin) = headers.header("origin") {
            // `null` is what a sandboxed iframe sends. It is not an origin.
            return origin != "null" && self.origin_allowed(origin);
        }
        if let Some(referer) = headers.header("referer") {
            return self.origin_allowed(referer);
        }
        false
    }

    /// The write rule for machine peers (`POST /api/notify`): a request that
    /// carries browser evidence is held to the same rules as `may_write`; one
    /// that carries none — a sibling hub's curl, which sends no Origin — is
    /// admitted. The default flips because the stakes differ: `/answer`
    /// changes state a waiting agent acts on, `/api/notify` shows a line of
    /// text on a screen, and the tailnet-only exposure plus `host_allowed`
    /// remain the outer boundary. Residual risk, stated: a legacy browser
    /// with no Sec-Fetch-Site could cross-site POST a spoofed popup; modern
    /// ones announce themselves and are refused.
    pub fn may_machine_write(&self, headers: &impl HeaderSource) -> bool {
        if let Some(site) = headers.header("sec-fetch-site") {
            return site == "same-origin";
        }
        if let Some(origin) = headers.header("origin") {
            return origin != "null" && self.origin_allowed(origin);
        }
        true
    }
}

/// So the check can be tested without building a whole `Request`.
pub trait HeaderSource {
    fn header(&self, name: &str) -> Option<&str>;
}

impl HeaderSource for crate::http::Request {
    fn header(&self, name: &str) -> Option<&str> {
        crate::http::Request::header(self, name)
    }
}

/// `http://host:port/path` → `host:port`.
fn authority(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| authority.to_ascii_lowercase())
}

/// `host:port` → `(host, Some(port))`, coping with the bracketed IPv6 form.
fn split_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (address, tail) = rest.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(port) => Some(port.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        return Some((address, port));
    }
    match authority.rsplit_once(':') {
        Some((name, port)) => Some((name, Some(port.parse().ok()?))),
        None => Some((authority, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Headers(Vec<(String, String)>);

    impl HeaderSource for Headers {
        fn header(&self, name: &str) -> Option<&str> {
            self.0
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> Headers {
        Headers(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn guard() -> Guard {
        let config = Config {
            origins: vec!["http://macbook.taila27604.ts.net:8787".to_string()],
            ..Config::default()
        };
        Guard {
            port: 8787,
            names: vec!["macbook-m2".to_string(), "fedora".to_string()],
            extra: config.origins.iter().filter_map(|o| authority(o)).collect(),
        }
    }

    #[test]
    fn the_hosts_hub_really_answers_on_are_allowed() {
        let guard = guard();
        for host in [
            "127.0.0.1:8787",
            "127.0.0.1",
            "localhost:8787",
            "[::1]:8787",
            "macbook:8787",    // MagicDNS short name
            "macbook-m2:8787", // the machine-name file
            "fedora:8787",     // uname -n
            "macbook.taila27604.ts.net:8787",
            "nuc.taila27604.ts.net",
        ] {
            assert!(guard.host_allowed(Some(host)), "{host}");
        }
    }

    #[test]
    fn a_host_an_attacker_could_own_is_refused() {
        let guard = guard();
        for host in [
            "evil.example.com:8787",
            "evil.example.com",
            "macbook.evil.example.com:8787",
            "127.0.0.1.evil.example.com:8787",
            // Right name, wrong port: a second service on this machine cannot
            // borrow hub's allowlist.
            "macbook:9999",
            "127.0.0.1:80",
            "",
            ".",
            "..",
            "a..b",
            "mac book",
            "macbook\u{7f}",
        ] {
            assert!(!guard.host_allowed(Some(host)), "{host}");
        }
        assert!(!guard.host_allowed(None), "a request with no Host");
    }

    #[test]
    fn same_origin_is_enough_and_a_foreign_origin_is_not() {
        let guard = guard();
        assert!(guard.may_write(&headers(&[("sec-fetch-site", "same-origin")])));
        assert!(guard.may_write(&headers(&[("origin", "http://127.0.0.1:8787")])));
        assert!(guard.may_write(&headers(&[("origin", "http://macbook:8787")])));
        assert!(guard.may_write(&headers(&[(
            "referer",
            "http://macbook.taila27604.ts.net:8787/"
        )])));

        assert!(!guard.may_write(&headers(&[("origin", "https://evil.example.com")])));
        assert!(!guard.may_write(&headers(&[("origin", "null")])));
        assert!(!guard.may_write(&headers(&[("referer", "https://evil.example.com/x")])));
        assert!(!guard.may_write(&headers(&[("sec-fetch-site", "cross-site")])));
        assert!(!guard.may_write(&headers(&[])), "nothing to go on is a no");
    }

    #[test]
    fn an_origin_header_wins_over_a_referer() {
        // A browser sets both; the Origin is the one that cannot be stripped by
        // a referrer policy, so it is the one that decides.
        let guard = guard();
        assert!(!guard.may_write(&headers(&[
            ("origin", "https://evil.example.com"),
            ("referer", "http://127.0.0.1:8787/"),
        ])));
    }

    #[test]
    fn authorities_come_out_of_urls_intact() {
        assert_eq!(authority("http://a.b:1/c?d#e").as_deref(), Some("a.b:1"));
        assert_eq!(authority("https://A.B").as_deref(), Some("a.b"));
        assert_eq!(authority("ftp://a.b"), None);
        assert_eq!(authority("http://"), None);
        assert_eq!(split_authority("[::1]:8787"), Some(("::1", Some(8787))));
        assert_eq!(split_authority("host"), Some(("host", None)));
        assert_eq!(split_authority("host:notaport"), None);
    }
}
