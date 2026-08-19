//! Time formatting shared by the read verbs. Everything on disk is an RFC3339
//! string; everything displayed is short.

use jiff::Timestamp;

/// `2026-08-12` in UTC, which is what the search line and the digest show.
pub fn date(epoch_seconds: i64) -> String {
    match Timestamp::from_second(epoch_seconds) {
        Ok(ts) => ts.to_string().chars().take(10).collect(),
        Err(_) => "----------".to_string(),
    }
}

/// `--since` accepts RFC3339 or `<N>[mhd]` (spec §7): `90m`, `4h`, `2d`.
pub fn parse_since(text: &str, now: Timestamp) -> Option<Timestamp> {
    let text = text.trim();
    if let Ok(ts) = text.parse::<Timestamp>() {
        return Some(ts);
    }
    let (digits, unit) = text.split_at(text.len().checked_sub(1)?);
    let n: i64 = digits.parse().ok()?;
    let seconds = match unit {
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return None,
    };
    Timestamp::from_second(now.as_second() - seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_render_as_ten_characters() {
        let ts: Timestamp = "2026-08-12T14:05:00Z".parse().unwrap();
        assert_eq!(date(ts.as_second()), "2026-08-12");
    }

    #[test]
    fn since_takes_rfc3339_or_a_relative_span() {
        let now: Timestamp = "2026-08-18T12:00:00Z".parse().unwrap();
        assert_eq!(
            parse_since("2026-08-01T00:00:00Z", now)
                .unwrap()
                .to_string(),
            "2026-08-01T00:00:00Z"
        );
        assert_eq!(
            parse_since("90m", now).unwrap().as_second(),
            now.as_second() - 5400
        );
        assert_eq!(
            parse_since("4h", now).unwrap().as_second(),
            now.as_second() - 14400
        );
        assert_eq!(
            parse_since("2d", now).unwrap().as_second(),
            now.as_second() - 172800
        );
        assert!(parse_since("nonsense", now).is_none());
        assert!(parse_since("5y", now).is_none());
        assert!(parse_since("", now).is_none());
    }
}
