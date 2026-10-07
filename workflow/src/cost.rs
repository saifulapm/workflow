//! What a session the engine started spent, as one `mem log --type run` line:
//! `cost <slug> <kind> <name>: key=value ...`. The tail is key=value so the
//! hub, status and serve can each read it without mem learning the words.

/// One session's cost, or a milestone's sum of them. A field is `None` when
/// it is unknown, and the line then leaves it out rather than write a zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostLine {
    pub slug: String,
    pub kind: String,
    pub name: String,
    pub minutes: Option<u64>,
    pub context: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub sessions: Option<u64>,
}

impl CostLine {
    pub fn line(&self) -> String {
        let numbers = [
            ("minutes", self.minutes),
            ("context", self.context),
            ("in", self.input),
            ("out", self.output),
            ("sessions", self.sessions),
        ];
        let words = [("model", &self.model), ("role", &self.role)];
        let mut line = format!("cost {} {} {}:", self.slug, self.kind, self.name);
        for (key, n) in numbers {
            if let Some(n) = n {
                line.push_str(&format!(" {key}={n}"));
            }
        }
        for (key, w) in words {
            if let Some(w) = w {
                line.push_str(&format!(" {key}={w}"));
            }
        }
        line
    }

    /// The inverse of `line`. It also takes a row of mem's text listing,
    /// which puts `<date>  #<id>  ` first, and skips a key it does not know
    /// so an older reader survives a newer field.
    pub fn parse(line: &str) -> Option<CostLine> {
        let line = line.trim();
        let body = line
            .strip_prefix("cost ")
            .or_else(|| line.split_once("  cost ").map(|(_, body)| body))?;
        let (head, tail) = body.split_once(':')?;
        let mut head = head.split_whitespace();
        let (slug, kind, name) = (head.next()?, head.next()?, head.next()?);
        if head.next().is_some() {
            return None;
        }
        let mut cost = CostLine {
            slug: slug.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            minutes: None,
            context: None,
            input: None,
            output: None,
            model: None,
            role: None,
            sessions: None,
        };
        for field in tail.split_whitespace() {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "minutes" => cost.minutes = value.parse().ok(),
                "context" => cost.context = value.parse().ok(),
                "in" => cost.input = value.parse().ok(),
                "out" => cost.output = value.parse().ok(),
                "sessions" => cost.sessions = value.parse().ok(),
                "model" => cost.model = Some(value.to_string()),
                "role" => cost.role = Some(value.to_string()),
                _ => {}
            }
        }
        Some(cost)
    }

    /// A milestone's line: one session per line summed, each number the sum
    /// of the lines that have it, and left out when none does.
    pub fn sum(slug: &str, lines: &[CostLine]) -> CostLine {
        let add = |field: fn(&CostLine) -> Option<u64>| {
            lines
                .iter()
                .filter_map(field)
                .fold(None, |sum, n| Some(sum.unwrap_or(0) + n))
        };
        CostLine {
            slug: slug.to_string(),
            kind: "milestone".to_string(),
            name: slug.to_string(),
            minutes: add(|c| c.minutes),
            context: add(|c| c.context),
            input: add(|c| c.input),
            output: add(|c| c.output),
            model: None,
            role: None,
            sessions: Some(lines.len() as u64),
        }
    }
}
