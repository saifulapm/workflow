//! Cloudflare Access's token, `Cf-Access-Jwt-Assertion`, checked on every
//! request but the store write. The rules are hub's: anything short of an
//! RS256 token signed by the team's key, for one of this Worker's Access
//! apps, inside its lifetime, is refused. This file holds the parts that need
//! no Worker (reading the token, the claims rules, reading the JWKS); the
//! Worker fetches the keys and checks the signature with WebCrypto.

use std::collections::HashMap;

use serde_json::{Value, json};

pub struct Access {
    /// One aud per Access app: next-hub and next-mem are two apps on this
    /// one Worker.
    auds: Vec<String>,
    team: String,
}

/// A token whose header passed: RS256, with a kid. Its signature is not
/// checked yet.
pub struct Jwt<'a> {
    pub kid: String,
    /// `<header>.<payload>`, the bytes the signature covers.
    pub signed: &'a str,
    pub signature: Vec<u8>,
    payload: &'a str,
}

impl Access {
    /// `aud` is a comma-separated list. `None` when either setting is
    /// missing or empty, and then the Worker refuses every guarded request.
    pub fn new(aud: Option<&str>, team: Option<&str>) -> Option<Access> {
        let auds: Vec<String> = aud?
            .split(',')
            .map(str::trim)
            .filter(|aud| !aud.is_empty())
            .map(str::to_string)
            .collect();
        let team = team?.trim().trim_end_matches('/');
        if auds.is_empty() || team.is_empty() {
            return None;
        }
        Some(Access {
            auds,
            team: team.to_string(),
        })
    }

    /// Where Access publishes the team's signing keys.
    pub fn certs_url(&self) -> String {
        format!("{}/cdn-cgi/access/certs", self.team)
    }

    /// The claims of a token whose signature has been checked: `Ok` only
    /// for one of this Worker's auds, from the team, inside its lifetime.
    pub fn claims(&self, jwt: &Jwt, now: i64) -> Result<(), &'static str> {
        let claims: Value = json_part(jwt.payload)?;
        let ours = |aud: &Value| {
            aud.as_str()
                .is_some_and(|aud| self.auds.iter().any(|a| a == aud))
        };
        let aud_ok = match claims.get("aud") {
            Some(Value::Array(auds)) => auds.iter().any(ours),
            Some(aud) => ours(aud),
            None => false,
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
}

/// Whether a checked token stands for a person. Access signs a person's
/// token with their email; a service token carries its Client ID in
/// `common_name`, an empty `sub` and no email. Every machine and cloud
/// session holds the service token the mem app admits.
pub fn person(jwt: &Jwt) -> bool {
    json_part(jwt.payload).is_ok_and(|claims| {
        claims
            .get("email")
            .and_then(Value::as_str)
            .is_some_and(|email| !email.trim().is_empty())
    })
}

/// Splits the token and checks its header. The error says why, for the
/// log; the client only ever sees a 401.
pub fn parse(token: Option<&str>) -> Result<Jwt<'_>, &'static str> {
    let token = token.ok_or("no Cf-Access-Jwt-Assertion")?;
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("not a JWT");
    };
    let head: Value = json_part(header)?;
    // RS256 and nothing else: `none` and the HS family are how a token gets
    // accepted without the team's key.
    if head.get("alg").and_then(Value::as_str) != Some("RS256") {
        return Err("alg is not RS256");
    }
    let kid = head.get("kid").and_then(Value::as_str).ok_or("no kid")?;
    Ok(Jwt {
        kid: kid.to_string(),
        signed: &token[..header.len() + 1 + payload.len()],
        signature: b64url(signature).ok_or("bad signature encoding")?,
        payload,
    })
}

/// The team's RS256 keys by kid, each as the JWK WebCrypto imports.
pub fn jwks(body: &[u8]) -> Result<HashMap<String, Value>, String> {
    let jwks: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let mut keys = HashMap::new();
    for jwk in jwks["keys"].as_array().ok_or("no keys array")? {
        if jwk["kty"] != "RSA" || jwk.get("alg").is_some_and(|alg| alg != "RS256") {
            continue;
        }
        let (Some(kid), Some(n), Some(e)) =
            (jwk["kid"].as_str(), jwk["n"].as_str(), jwk["e"].as_str())
        else {
            continue;
        };
        if b64url(n).is_none() || b64url(e).is_none() {
            continue;
        }
        keys.insert(kid.to_string(), json!({"kty": "RSA", "n": n, "e": e}));
    }
    Ok(keys)
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

#[cfg(test)]
mod tests {
    use super::*;

    const TEAM: &str = "https://saifulapm.cloudflareaccess.com";

    fn access() -> Access {
        Access::new(
            Some("hub-aud, mem-aud"),
            Some("https://saifulapm.cloudflareaccess.com/"),
        )
        .unwrap()
    }

    /// A token with these claims; the signature is a stand-in, since only
    /// the Worker checks it.
    fn token(claims: &str) -> String {
        // {"alg":"RS256","kid":"k1"}
        format!(
            "eyJhbGciOiJSUzI1NiIsImtpZCI6ImsxIn0.{}.c2ln",
            encode(claims.as_bytes())
        )
    }

    fn encode(bytes: &[u8]) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        bytes
            .chunks(3)
            .flat_map(|c| {
                let n = c.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - c.len()));
                (0..=c.len()).map(move |i| ABC[(n >> (18 - 6 * i) & 63) as usize] as char)
            })
            .collect()
    }

    fn claims(claims: &str, now: i64) -> Result<(), &'static str> {
        let token = token(claims);
        access().claims(&parse(Some(&token)).unwrap(), now)
    }

    #[test]
    fn unset_or_empty_settings_give_no_access() {
        assert!(Access::new(None, Some(TEAM)).is_none());
        assert!(Access::new(Some("aud"), None).is_none());
        assert!(Access::new(Some(" , "), Some(TEAM)).is_none());
        assert!(Access::new(Some("aud"), Some("")).is_none());
    }

    #[test]
    fn the_certs_url_is_under_the_team() {
        assert_eq!(
            access().certs_url(),
            "https://saifulapm.cloudflareaccess.com/cdn-cgi/access/certs"
        );
    }

    #[test]
    fn parse_keeps_the_kid_and_the_signed_bytes() {
        let token = token("{}");
        let jwt = parse(Some(&token)).unwrap();
        assert_eq!(jwt.kid, "k1");
        assert_eq!(jwt.signed, "eyJhbGciOiJSUzI1NiIsImtpZCI6ImsxIn0.e30");
        assert_eq!(jwt.signature, b"sig");
    }

    #[test]
    fn parse_refuses_what_is_not_an_rs256_jwt() {
        assert_eq!(parse(None).err(), Some("no Cf-Access-Jwt-Assertion"));
        assert_eq!(parse(Some("a.b")).err(), Some("not a JWT"));
        assert_eq!(parse(Some("a.b.c.d")).err(), Some("not a JWT"));
        // {"alg":"none","kid":"k1"} and {"alg":"HS256","kid":"k1"}
        assert_eq!(
            parse(Some("eyJhbGciOiJub25lIiwia2lkIjoiazEifQ.e30.")).err(),
            Some("alg is not RS256")
        );
        assert_eq!(
            parse(Some("eyJhbGciOiJIUzI1NiIsImtpZCI6ImsxIn0.e30.c2ln")).err(),
            Some("alg is not RS256")
        );
        // {"alg":"RS256"}
        assert_eq!(
            parse(Some("eyJhbGciOiJSUzI1NiJ9.e30.c2ln")).err(),
            Some("no kid")
        );
    }

    #[test]
    fn claims_take_any_listed_aud_from_the_team_in_its_lifetime() {
        let iss = format!(r#""iss":"{TEAM}""#);
        assert_eq!(
            claims(&format!(r#"{{"aud":["mem-aud"],{iss},"exp":101}}"#), 100),
            Ok(())
        );
        assert_eq!(
            claims(
                &format!(r#"{{"aud":"hub-aud",{iss},"exp":101,"nbf":100}}"#),
                100
            ),
            Ok(())
        );
        assert_eq!(
            claims(&format!(r#"{{"aud":["other"],{iss},"exp":101}}"#), 100),
            Err("aud is another app's")
        );
        assert_eq!(
            claims(
                r#"{"aud":["hub-aud"],"iss":"https://other.cloudflareaccess.com","exp":101}"#,
                100
            ),
            Err("iss is not the team")
        );
        assert_eq!(
            claims(&format!(r#"{{"aud":["hub-aud"],{iss},"exp":100}}"#), 100),
            Err("expired")
        );
        assert_eq!(
            claims(&format!(r#"{{"aud":["hub-aud"],{iss}}}"#), 100),
            Err("no exp")
        );
        assert_eq!(
            claims(
                &format!(r#"{{"aud":["hub-aud"],{iss},"exp":200,"nbf":101}}"#),
                100
            ),
            Err("not yet valid")
        );
    }

    #[test]
    fn a_person_is_a_token_with_an_email_and_a_service_token_is_not() {
        let person = token(r#"{"aud":["hub-aud"],"sub":"7335d417","email":"saiful@example.com"}"#);
        assert!(super::person(&parse(Some(&person)).unwrap()));
        // Access's service token: the Client ID in common_name, sub empty.
        let service = token(r#"{"aud":["mem-aud"],"common_name":"e3678.access","sub":""}"#);
        assert!(!super::person(&parse(Some(&service)).unwrap()));
        let blank = token(r#"{"aud":["hub-aud"],"email":""}"#);
        assert!(!super::person(&parse(Some(&blank)).unwrap()));
    }

    #[test]
    fn jwks_keeps_only_rs256_rsa_keys_with_a_kid() {
        let body = br#"{"keys":[
            {"kty":"RSA","alg":"RS256","kid":"k1","n":"AQAB","e":"AQAB"},
            {"kty":"RSA","kid":"k2","n":"AQAB","e":"AQAB"},
            {"kty":"RSA","alg":"RS512","kid":"k3","n":"AQAB","e":"AQAB"},
            {"kty":"EC","kid":"k4","x":"AQAB","y":"AQAB"},
            {"kty":"RSA","kid":"k5","n":"no+pad=","e":"AQAB"},
            {"kty":"RSA","n":"AQAB","e":"AQAB"}
        ]}"#;
        let keys = jwks(body).unwrap();
        let mut kids: Vec<&str> = keys.keys().map(String::as_str).collect();
        kids.sort();
        assert_eq!(kids, ["k1", "k2"]);
        assert_eq!(keys["k1"], json!({"kty": "RSA", "n": "AQAB", "e": "AQAB"}));
    }
}
