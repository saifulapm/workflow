//! What a session the engine started spent, read off its run line:
//! `cost <slug> <kind> <name>: key=value ...`. The engine writes one per
//! session and one `milestone` line per landed milestone, so mem needs no
//! word of this grammar and the hub reads the lines it already lists.

/// One run line's numbers. A key the line leaves out is zero here, because
/// the page only ever adds them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CostRow {
    pub kind: String,
    pub name: String,
    pub minutes: u64,
    pub context: u64,
    pub input: u64,
    pub output: u64,
    pub sessions: u64,
}

/// The milestone slug and the row of one run line's title, or `None` for a
/// run line that is not a cost.
pub fn parse_cost(title: &str) -> Option<(String, CostRow)> {
    let (head, tail) = title.trim().strip_prefix("cost ")?.split_once(':')?;
    let mut head = head.split_whitespace();
    let (slug, kind, name) = (head.next()?, head.next()?, head.next()?);
    if head.next().is_some() {
        return None;
    }
    let mut row = CostRow {
        kind: kind.to_string(),
        name: name.to_string(),
        ..CostRow::default()
    };
    for (key, value) in tail.split_whitespace().filter_map(|f| f.split_once('=')) {
        let n = value.parse().unwrap_or(0);
        match key {
            "minutes" => row.minutes = n,
            "context" => row.context = n,
            "in" => row.input = n,
            "out" => row.output = n,
            "sessions" => row.sessions = n,
            _ => {}
        }
    }
    Some((slug.to_string(), row))
}

/// `<n> sessions · <n> min · <n> in · <n> out`, the shape the week and a
/// milestone's cost share.
pub fn summary(sessions: u64, minutes: u64, input: u64, output: u64) -> String {
    format!(
        "{} sessions · {} min · {} in · {} out",
        short(sessions),
        short(minutes),
        short(input),
        short(output)
    )
}

/// A token count is millions wide, and a phone's line is not: from a
/// thousand up a number shows one decimal and `k`, from a million `M`.
pub fn short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_line_parses_with_its_slug_first() {
        let (slug, row) = parse_cost(
            "cost g1-log worker g1-t1: minutes=14 context=82000 in=1840000 out=23400 model=sonnet",
        )
        .unwrap();
        assert_eq!(slug, "g1-log");
        assert_eq!(
            row,
            CostRow {
                kind: "worker".to_string(),
                name: "g1-t1".to_string(),
                minutes: 14,
                context: 82000,
                input: 1840000,
                output: 23400,
                sessions: 0,
            }
        );
    }

    #[test]
    fn a_missing_key_is_zero() {
        let (_, row) = parse_cost("cost g1-log milestone g1-log: sessions=3 in=900").unwrap();
        assert_eq!(
            (row.sessions, row.minutes, row.input, row.output),
            (3, 0, 900, 0)
        );
    }

    #[test]
    fn a_run_line_that_is_not_a_cost_is_none() {
        assert_eq!(parse_cost("dogfood g1-log: pass"), None);
        assert_eq!(parse_cost("cost g1-log worker: minutes=1"), None);
        assert_eq!(
            parse_cost("cost g1-log worker g1-t1 extra: minutes=1"),
            None
        );
    }

    #[test]
    fn short_numbers_from_a_thousand_and_a_million() {
        assert_eq!(short(999), "999");
        assert_eq!(short(1_000), "1.0k");
        assert_eq!(short(31_200), "31.2k");
        assert_eq!(short(1_600_500), "1.6M");
    }
}
