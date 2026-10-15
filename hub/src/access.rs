//! Cloudflare Access's token, `Cf-Access-Jwt-Assertion`, checked on every
//! request to the MCP endpoint. Nothing else guards that port, so anything
//! short of an RS256 token signed by the team's key, for this app, inside its
//! lifetime, is refused.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rsa::{BoxedUint, Pkcs1v15Sign, RsaPublicKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::proc::{self, Ended};

/// A token naming a kid the cached keys lack refetches at most this often, so
/// a stream of made-up kids cannot turn into a stream of curls.
const REFETCH_EVERY: Duration = Duration::from_secs(60);
/// curl's own `--max-time` is 10 s; this is the backstop if curl ignores it.
const CURL_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Access {
    aud: String,
    team: String,
    keys: Mutex<Keys>,
}

#[derive(Default)]
struct Keys {
    by_kid: HashMap<String, RsaPublicKey>,
    fetched: Option<Instant>,
}

impl Access {
    pub fn new(aud: &str, team: &str) -> Access {
        Access {
            aud: aud.to_string(),
            team: team.trim_end_matches('/').to_string(),
            keys: Mutex::new(Keys::default()),
        }
    }

    /// `Ok` only for a token this hub's Access app would have issued. The
    /// error says why, for the journal; the client only ever sees a 401.
    pub fn check(&self, token: Option<&str>, now: i64) -> Result<(), &'static str> {
        let token = token.ok_or("no Cf-Access-Jwt-Assertion")?;
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("not a JWT");
        };
        let head: Value = json_part(header)?;
        // RS256 and nothing else: `none` and the HS family are how a token
        // gets accepted without the team's key.
        if head.get("alg").and_then(Value::as_str) != Some("RS256") {
            return Err("alg is not RS256");
        }
        let kid = head.get("kid").and_then(Value::as_str).ok_or("no kid")?;
        let key = self.key(kid).ok_or("unknown kid")?;
        let signature = b64url(signature).ok_or("bad signature encoding")?;
        let digest = Sha256::digest(format!("{header}.{payload}").as_bytes());
        key.verify(Pkcs1v15Sign::new::<Sha256>(), &digest, &signature)
            .map_err(|_| "bad signature")?;

        let claims: Value = json_part(payload)?;
        let aud_ok = match claims.get("aud") {
            Some(Value::String(aud)) => *aud == self.aud,
            Some(Value::Array(auds)) => auds.iter().any(|a| a.as_str() == Some(&self.aud)),
            _ => false,
        };
        if !aud_ok {
            return Err("aud is another app's");
        }
        if claims.get("iss").and_then(Value::as_str) != Some(self.team.as_str()) {
            return Err("iss is not the team");
        }
        let exp = claims.get("exp").and_then(Value::as_i64).ok_or("no exp")?;
        if exp <= now {
            return Err("expired");
        }
        if let Some(nbf) = claims.get("nbf")
            && nbf.as_i64().is_none_or(|nbf| nbf > now)
        {
            return Err("not yet valid");
        }
        Ok(())
    }

    /// The key for `kid`, fetching the team's keys when none are cached yet
    /// or, at most once a minute, when `kid` is new (Access rotates keys).
    fn key(&self, kid: &str) -> Option<RsaPublicKey> {
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = keys.by_kid.get(kid) {
            return Some(key.clone());
        }
        if keys.fetched.is_some_and(|at| at.elapsed() < REFETCH_EVERY) {
            return None;
        }
        keys.fetched = Some(Instant::now());
        match self.fetch() {
            Ok(fresh) => keys.by_kid = fresh,
            Err(why) => eprintln!("hub: mcp: fetching Access keys: {why}"),
        }
        keys.by_kid.get(kid).cloned()
    }

    /// hub has no HTTP client, so the JWKS comes through curl, as the
    /// doorbell's rings do. The URL is argv, never a shell string.
    fn fetch(&self) -> Result<HashMap<String, RsaPublicKey>, String> {
        let url = format!("{}/cdn-cgi/access/certs", self.team);
        let mut command = Command::new("curl");
        command.args(["-fsS", "--max-time", "10", &url]);
        let done = match proc::output_within(&mut command, CURL_TIMEOUT) {
            Ended::Exited(done) if done.code == Some(0) => done,
            Ended::Exited(done) => {
                return Err(format!(
                    "curl exited {:?}: {}",
                    done.code,
                    String::from_utf8_lossy(&done.stderr).trim()
                ));
            }
            Ended::TimedOut => return Err("curl timed out".to_string()),
            Ended::Failed(why) => return Err(why),
        };
        let jwks: Value = serde_json::from_slice(&done.stdout).map_err(|e| e.to_string())?;
        let mut keys = HashMap::new();
        for jwk in jwks["keys"].as_array().ok_or("no keys array")? {
            if jwk["kty"] != "RSA" || jwk.get("alg").is_some_and(|alg| alg != "RS256") {
                continue;
            }
            let (Some(kid), Some(n), Some(e)) = (
                jwk["kid"].as_str(),
                jwk["n"].as_str().and_then(b64url),
                jwk["e"].as_str().and_then(b64url),
            ) else {
                continue;
            };
            let n = BoxedUint::from_be_slice_vartime(&n);
            let e = BoxedUint::from_be_slice_vartime(&e);
            if let Ok(key) = RsaPublicKey::new(n, e) {
                keys.insert(kid.to_string(), key);
            }
        }
        Ok(keys)
    }
}

fn json_part(part: &str) -> Result<Value, &'static str> {
    let bytes = b64url(part).ok_or("bad base64url")?;
    serde_json::from_slice(&bytes).map_err(|_| "bad JSON")
}

/// Unpadded base64url, the only encoding a JWT or a JWK uses.
fn b64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        // Only the low 14 bits are ever unread, so the rest can go.
        acc = (acc << 6 | u32::from(v)) & 0x3fff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // One leftover character is never a whole byte.
    (bits < 6).then_some(out)
}
