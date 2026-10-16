//! Pairing a browser with the hub web. A machine holding the store token
//! asks the store for a single-use code (`hub pair --web`); the device opens
//! `/pair/<code>` and presses Pair, and that POST trades the code for a
//! cookie the store knows again for a year. Only a paired device's POST
//! writes, and only from the hub web's own pages. The page and its POST are
//! two steps so a chat app's link preview cannot spend the code.

use hubpages::http::Request;
use hubpages::pages::page_shell;

/// How long a code works, in milliseconds.
pub const CODE_TTL_MS: i64 = 10 * 60 * 1000;
pub const COOKIE: &str = "hub_device";
/// A year, in seconds, as `Max-Age` counts.
const COOKIE_MAX_AGE: u32 = 365 * 24 * 60 * 60;
/// What an unpaired browser is told to do, after "Pair this device first: ".
pub const HOW: &str = "run hub pair --web on one of your machines and open the link it prints";

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Six Crockford characters, so a code read off a screen survives being
/// typed: 30 random bits, spent once and good for ten minutes.
pub fn new_code(random: [u8; 10]) -> String {
    random[..6]
        .iter()
        .map(|byte| CROCKFORD[(byte & 31) as usize] as char)
        .collect()
}

/// A device's token: 80 random bits as 16 Crockford characters.
pub fn new_token(random: [u8; 10]) -> String {
    let value = random
        .iter()
        .fold(0u128, |acc, byte| (acc << 8) | u128::from(*byte));
    (0..16)
        .map(|i| CROCKFORD[((value >> (5 * (15 - i))) & 31) as usize] as char)
        .collect()
}

/// Whether `code` has the shape `new_code` makes.
pub fn is_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| CROCKFORD.contains(&b))
}

/// The device token in the request's cookie, if it carries one.
pub fn token(request: &Request) -> Option<&str> {
    request
        .header("cookie")?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
}

/// The cookie a newly paired device keeps. The hub web is https only, so it
/// is Secure.
pub fn cookie(token: &str) -> String {
    format!("{COOKIE}={token}; Path=/; Max-Age={COOKIE_MAX_AGE}; HttpOnly; Secure; SameSite=Strict")
}

/// Whether a POST comes from the hub web's own pages: `Sec-Fetch-Site:
/// same-origin` on its own, else an `Origin` or a `Referer` on the request's
/// own host. A request with none of the three is refused (hub/src/origin.rs).
pub fn may_write(request: &Request) -> bool {
    if let Some(site) = request.header("sec-fetch-site") {
        return site == "same-origin";
    }
    let Some(host) = request.header("host").map(str::to_ascii_lowercase) else {
        return false;
    };
    let ours = |url: &str| {
        url.strip_prefix("https://")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .is_some_and(|authority| authority.to_ascii_lowercase() == host)
    };
    if let Some(origin) = request.header("origin") {
        return ours(origin);
    }
    request.header("referer").is_some_and(ours)
}

/// The name a device is kept under, from its User-Agent.
pub fn device_name(user_agent: Option<&str>) -> &'static str {
    let agent = user_agent.unwrap_or_default().to_ascii_lowercase();
    [
        ("iphone", "iphone"),
        ("ipad", "ipad"),
        ("android", "android"),
        ("macintosh", "mac"),
        ("windows", "windows"),
        ("linux", "linux"),
    ]
    .into_iter()
    .find(|(needle, _)| agent.contains(needle))
    .map_or("device", |(_, name)| name)
}

/// `GET /pair/<code>` while the code is pending: the Pair button.
pub fn page(code: &str) -> String {
    page_shell(
        "pair",
        None,
        &format!(
            "<p>Pair this device with the hub web. It keeps a cookie for a year.</p>\n\
             <form method=\"post\" action=\"/pair/{code}\">\n\
             <button type=\"submit\">Pair this device</button>\n</form>\n"
        ),
    )
}

/// A code that is spent, unknown or expired.
pub fn gone() -> String {
    page_shell(
        "pair",
        None,
        "<p>This pairing link is spent, unknown or expired. \
         Run <code>hub pair --web</code> again for a new one.</p>\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(headers: &[(&str, &str)]) -> Request {
        crate::site::request(
            "POST",
            "/answer",
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            vec![],
        )
    }

    #[test]
    fn codes_and_tokens_are_crockford_of_their_length() {
        let code = new_code([0xFF, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(code, "Z01234");
        assert!(is_code(&code));
        assert!(!is_code("Z0123") && !is_code("Z0123I") && !is_code("z01234"));
        let token = new_token([7; 10]);
        assert_eq!(token.len(), 16);
        assert_ne!(token, new_token([8; 10]));
    }

    #[test]
    fn the_cookie_is_read_back_by_name() {
        let with = request(&[("Cookie", "a=b; hub_device=TOKEN1; c=d")]);
        assert_eq!(token(&with), Some("TOKEN1"));
        assert_eq!(token(&request(&[("Cookie", "hub_device=")])), None);
        assert_eq!(token(&request(&[])), None);
        assert!(cookie("T").contains("Secure") && cookie("T").contains("HttpOnly"));
    }

    #[test]
    fn only_the_hub_webs_own_pages_may_write() {
        let host = ("Host", "next-hub.on-rexa.com");
        assert!(may_write(&request(&[
            host,
            ("Sec-Fetch-Site", "same-origin")
        ])));
        assert!(!may_write(&request(&[
            host,
            ("Sec-Fetch-Site", "cross-site"),
            ("Origin", "https://next-hub.on-rexa.com")
        ])));
        assert!(may_write(&request(&[
            host,
            ("Origin", "https://next-hub.on-rexa.com")
        ])));
        assert!(!may_write(&request(&[
            host,
            ("Origin", "https://evil.example")
        ])));
        assert!(!may_write(&request(&[host, ("Origin", "null")])));
        assert!(may_write(&request(&[
            host,
            ("Referer", "https://next-hub.on-rexa.com/p/walk/questions")
        ])));
        assert!(!may_write(&request(&[
            host,
            ("Referer", "http://next-hub.on-rexa.com/")
        ])));
        assert!(!may_write(&request(&[host])));
    }
}
