//! `application/x-www-form-urlencoded`, by hand (spec §8).
//!
//! Hand-rolled because there is no approved crate for it: `tiny_http` would not
//! have supplied it either (review M-7), so this decoder exists in every version
//! of the plan. It is also where a wrong answer would be silent — the
//! orchestrator acts on whatever text comes out of here — so the adversarial
//! cases in §10 AC10 are tests, not comments.

/// One decoded form or query string, in the order the pairs arrived.
#[derive(Debug, Default, Clone)]
pub struct Form {
    pairs: Vec<(String, String)>,
}

impl Form {
    /// Decodes a `a=1&b=2` string. Everything is lossy at the UTF-8 boundary
    /// rather than fatal: a phone keyboard cannot produce invalid UTF-8, but a
    /// crafted request can, and dropping the whole answer over one bad byte
    /// would be the wrong trade.
    pub fn parse(input: &str) -> Form {
        let mut pairs = Vec::new();
        for field in input.split('&') {
            if field.is_empty() {
                continue;
            }
            let (raw_key, raw_value) = match field.split_once('=') {
                Some((key, value)) => (key, value),
                // `?debug` — present, with no value. Distinct from absent.
                None => (field, ""),
            };
            pairs.push((decode(raw_key), decode(raw_value)));
        }
        Form { pairs }
    }

    /// The first value for `key`, or `None` when the field was not sent at all.
    /// An empty field is `Some("")`: "the user cleared the box" and "the user
    /// never saw the box" are different answers.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

/// Form decoding: `+` is a space, `%XX` is a byte, anything else is itself.
pub fn decode(input: &str) -> String {
    String::from_utf8_lossy(&decode_bytes(input.as_bytes(), true)).into_owned()
}

/// Path decoding: the same, except `+` is a literal plus.
pub fn decode_path(input: &str) -> String {
    String::from_utf8_lossy(&decode_bytes(input.as_bytes(), false)).into_owned()
}

/// Decoding runs over **bytes** and only becomes a string at the end. A
/// character like 🙂 arrives as `%F0%9F%99%82` — four separate escapes — so a
/// decoder that produced a `char` per escape would corrupt every multi-byte
/// character a phone can type.
fn decode_bytes(input: &[u8], plus_is_space: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < input.len() => match (hex(input[i + 1]), hex(input[i + 2])) {
                (Some(high), Some(low)) => {
                    out.push(high << 4 | low);
                    i += 3;
                }
                // `%ZZ` and a trailing `%` are not escapes; a browser will not
                // send them, and dropping them would silently eat a character.
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Percent-encodes one query-string component. Everything outside the
/// unreserved set goes, including the `+` that would otherwise decode as a
/// space and the `&`/`=` that would otherwise start a new field.
pub fn encode_component(input: &str) -> String {
    const UNRESERVED: &[u8] = b"-._~";
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED.contains(byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_and_decoding_are_inverses_over_awkward_text() {
        for text in [
            "a b&c=d+🙂",
            "",
            "%",
            "100%",
            "\"; touch /tmp/pwned; #",
            "<script>alert(1)</script>",
            "line\nbreak\ttab",
            "ünïcödé — em dash",
        ] {
            let encoded = encode_component(text);
            assert!(
                !encoded.contains(['&', '=', '+', ' ']),
                "{encoded} is not safe in a query string"
            );
            assert_eq!(decode(&encoded), text, "round trip of {text:?}");
        }
    }
}
