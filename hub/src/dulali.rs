//! Dulali, the assistant on the VPS, answers the questions the route sends it.
//!
//! The doorbell posts each question first seen on the Dulali route to Hermes's
//! webhook, signed with a secret the two share. Dulali answers on a socket of
//! its own, with a token of its own, and the hub holds that answer for an
//! override window before it reaches mem: Saiful can replace it from home in
//! that time, and the run resumes on his. A pass rings the phone at once; a
//! Dulali that says nothing for ten minutes rings it too.
//!
//! The token lives in `hub/dulali-token`, apart from `hub/devices`, so no page
//! and no agent route takes it: the socket is the one place it opens.
//! What the hub knows about Dulali's answers lives in `hub/dulali.json`,
//! written whole, so a restart keeps every window and every pass.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::config;
use crate::http::Request;
use crate::pair;

/// How long Dulali's answer waits for Saiful's before the run resumes on it.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(120);
/// How long Dulali may say nothing before the question rings the phone.
pub const DEFAULT_SILENCE: Duration = Duration::from_secs(600);
/// How long an answer Dulali gave stays replaceable from home.
pub const KEEP: Duration = Duration::from_secs(24 * 60 * 60);
/// The most of Dulali's answer the override ring carries.
pub const RING_ANSWER_CHARS: usize = 80;

// Home draws the answers Dulali gave, so their shape lives with the pages.
pub use hubpages::host::Held;

/// What `dulali.json` holds.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    answers: BTreeMap<String, Held>,
    /// Question id to when Dulali passed it.
    #[serde(default)]
    passed: BTreeMap<String, i64>,
}

pub struct Dulali {
    path: PathBuf,
    book: Mutex<Book>,
    pub window: Duration,
    pub silence: Duration,
    /// Hermes's webhook URL; with none the route rings the phone.
    pub webhook: Option<String>,
    secret: Option<String>,
}

impl Dulali {
    pub fn open(path: PathBuf, webhook: Option<String>) -> Dulali {
        let book = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("hub: ignoring {}: {e}", path.display());
                Book::default()
            }),
            Err(_) => Book::default(),
        };
        let secret = std::env::var("HUB_DULALI_SECRET")
            .ok()
            .filter(|s| !s.trim().is_empty());
        Dulali {
            path,
            book: Mutex::new(book),
            window: duration_env("HUB_DULALI_WINDOW_MS", DEFAULT_WINDOW),
            silence: duration_env("HUB_DULALI_SILENT_MS", DEFAULT_SILENCE),
            webhook,
            secret,
        }
    }

    /// Whether questions can go to Dulali at all: a webhook and its secret.
    pub fn reachable(&self) -> bool {
        self.webhook.is_some() && self.secret.is_some()
    }

    /// Posts one question to Hermes, signed. False when it did not take;
    /// the ten-minute ring covers that.
    pub fn send(&self, row: &Value) -> bool {
        let (Some(url), Some(secret)) = (&self.webhook, &self.secret) else {
            return false;
        };
        let id = row["id"].as_str().unwrap_or("");
        // Only what Dulali needs to look the question up; it reads the rest
        // with mem, as the doorbell never sends a question's text.
        let body = json!({
            "event_type": "question",
            "id": id,
            "short_id": row["short_id"],
            "project": row["project"],
            "machine": row["machine"],
        })
        .to_string();
        let timestamp = jiff::Timestamp::now().as_second().to_string();
        let signature = sign(secret, &timestamp, &body);
        let result = Command::new("curl")
            .args([
                "-fsS",
                "-m",
                crate::doorbell::CURL_TIMEOUT_SECONDS,
                "-o",
                "/dev/null",
            ])
            .args(["-H", "Content-Type: application/json"])
            .args(["-H", &format!("X-Webhook-Timestamp: {timestamp}")])
            .args(["-H", &format!("X-Webhook-Signature-V2: {signature}")])
            .args(["-H", &format!("X-Request-ID: {id}")])
            .args(["--data-binary", &body, url])
            .output();
        match result {
            Ok(out) if out.status.success() => true,
            Ok(out) => {
                eprintln!(
                    "hub: dulali: Hermes said no ({}): {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                false
            }
            Err(e) => {
                eprintln!("hub: dulali: could not run curl: {e}");
                false
            }
        }
    }

    /// Whether Dulali has already answered or passed `id`.
    pub fn has_spoken(&self, id: &str) -> bool {
        let book = self.lock();
        book.answers.contains_key(id) || book.passed.contains_key(id)
    }

    pub fn hold(&self, id: &str, held: Held) {
        self.change(|book| {
            book.answers.insert(id.to_string(), held);
        });
    }

    pub fn pass(&self, id: &str, now_ms: i64) {
        self.change(|book| {
            book.passed.insert(id.to_string(), now_ms);
        });
    }

    /// A held answer by its question's full id or its last eight characters.
    pub fn find(&self, id: &str) -> Option<(String, Held)> {
        let id = id.trim_start_matches('#');
        if id.len() < 8 {
            return None;
        }
        self.lock()
            .answers
            .iter()
            .find(|(full, _)| *full == id || full.ends_with(id))
            .map(|(full, h)| (full.clone(), h.clone()))
    }

    pub fn held(&self, id: &str) -> Option<Held> {
        self.lock().answers.get(id).cloned()
    }

    /// The answers whose window has closed and that are not yet in mem.
    pub fn due(&self, now_ms: i64) -> Vec<(String, Held)> {
        self.lock()
            .answers
            .iter()
            .filter(|(_, h)| h.until_ms <= now_ms && h.written.is_none() && !h.replaced)
            .map(|(id, h)| (id.clone(), h.clone()))
            .collect()
    }

    /// Dulali's answers still replaceable from home, newest first.
    pub fn replaceable(&self, now_ms: i64) -> Vec<(String, Held)> {
        let mut rows: Vec<(String, Held)> = self
            .lock()
            .answers
            .iter()
            .filter(|(_, h)| !h.replaced && now_ms - h.at_ms < KEEP.as_millis() as i64)
            .map(|(id, h)| (id.clone(), h.clone()))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1.at_ms));
        rows
    }

    pub fn written(&self, id: &str, answer_id: &str) {
        self.change(|book| {
            if let Some(held) = book.answers.get_mut(id) {
                held.written = Some(answer_id.to_string());
            }
        });
    }

    pub fn replaced(&self, id: &str) {
        self.change(|book| {
            if let Some(held) = book.answers.get_mut(id) {
                held.replaced = true;
            }
        });
    }

    /// Applies one change, forgets what is past keeping, and saves.
    fn change(&self, f: impl FnOnce(&mut Book)) {
        let mut book = self.lock();
        f(&mut book);
        let cutoff = now_ms() - KEEP.as_millis() as i64;
        // A pass is kept for good: a question Dulali passed can wait longer
        // than a day, and losing its pass would ring it as silence.
        book.answers.retain(|_, h| h.at_ms >= cutoff);
        let result = serde_json::to_string(&*book)
            .map_err(anyhow::Error::from)
            .and_then(|text| pair::rewrite_private(&self.path, &text));
        if let Err(e) = result {
            eprintln!("hub: saving {}: {e:#}", self.path.display());
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Hex HMAC-SHA256 of `<timestamp>.<body>`: Hermes's generic V2 signature.
pub fn sign(secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn token_path() -> Result<PathBuf> {
    Ok(config::state_home()?.join("hub/dulali-token"))
}

/// Dulali's token, made on the first call.
pub fn token(path: &Path) -> Result<String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let token = text.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }
    let token = config::base32(&config::urandom::<16>()?);
    pair::rewrite_private(path, &format!("{token}\n"))?;
    Ok(token)
}

/// Whether the request carries Dulali's token. With no token made yet,
/// nothing does.
pub fn authorized(request: &Request, path: &Path) -> bool {
    let Ok(known) = std::fs::read_to_string(path) else {
        return false;
    };
    let known = known.trim();
    let given = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    !known.is_empty()
        && known.len() == given.len()
        && known
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// When a ULID was made, in milliseconds: its first ten characters.
pub fn ulid_ms(id: &str) -> Option<i64> {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if id.len() != 26 {
        return None;
    }
    id.bytes().take(10).try_fold(0i64, |acc, c| {
        let digit = ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase())?;
        Some(acc * 32 + digit as i64)
    })
}

/// The answer as the override ring shows it: one line, capped.
pub fn ring_answer(answer: &str) -> String {
    let line = answer.lines().next().unwrap_or("").trim();
    if line.chars().count() > RING_ANSWER_CHARS {
        let cut: String = line.chars().take(RING_ANSWER_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

pub fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn duration_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vector from Hermes's own V2 rule, computed with Python's hmac.
    #[test]
    fn the_signature_is_hex_hmac_of_timestamp_dot_body() {
        assert_eq!(
            sign("s3cret", "1760000000", "{\"id\":\"x\"}"),
            "489b39c9ebcdf2b4888267ee14eb3fd4f4e24655be67175273f684b5e674e20c"
        );
    }

    /// The plan's numbers: a two-minute window and ten minutes of silence.
    #[test]
    fn the_window_is_two_minutes_and_the_silence_ten() {
        assert_eq!(DEFAULT_WINDOW, Duration::from_secs(120));
        assert_eq!(DEFAULT_SILENCE, Duration::from_secs(600));
        assert_eq!(
            duration_env("HUB_TEST_UNSET_SEAM", DEFAULT_WINDOW),
            DEFAULT_WINDOW
        );
    }

    #[test]
    fn a_pass_outlives_the_day_answers_are_kept() {
        let dir = std::env::temp_dir().join(format!("hub-dulali-pass-{}", std::process::id()));
        let dulali = Dulali::open(dir.join("dulali.json"), None);
        let old = now_ms() - 2 * KEEP.as_millis() as i64;
        dulali.pass("01OLDPASS0000000000000000A", old);
        dulali.pass("01NEWPASS0000000000000000B", now_ms());
        assert!(dulali.has_spoken("01OLDPASS0000000000000000A"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_ulid_names_the_millisecond_it_was_made() {
        // 01M4ZM41AZ… says it was created at 2026-10-15T11:10:53.758Z, and
        // mem mints the id a few milliseconds after that stamp.
        let ms = ulid_ms("01M4ZM41AZ6Z2S16V9JFFBJ4AG").unwrap();
        let created = "2026-10-15T11:10:53.758Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_millisecond();
        assert!((0..1000).contains(&(ms - created)), "{ms} vs {created}");
        assert_eq!(ulid_ms("short"), None);
    }

    #[test]
    fn the_ring_carries_one_line_of_the_answer_capped() {
        assert_eq!(ring_answer("eight\n\nDulali checked: #ABC"), "eight");
        let long = "x".repeat(200);
        assert_eq!(ring_answer(&long).chars().count(), RING_ANSWER_CHARS);
    }
}
