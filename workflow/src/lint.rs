//! `workflow lint-msg` -- the two tiers of spec §3, over a commit message, a
//! branch name or a PR body.

use std::io::{IsTerminal, Read};
use std::path::Path;

use crate::gitcmd::Git;
use crate::{exit, memcli, warn};

/// Tier one, provenance: these never pass, and no ruling clears them.
const HARD: &[(&str, Hard)] = &[
    ("a Claude co-author trailer", Hard::CoAuthored),
    (
        "the \"generated with\" line",
        Hard::Literal("generated with claude code"),
    ),
    ("a claude.ai/code link", Hard::Literal("claude.ai/code")),
    ("a Claude session URL", Hard::ClaudeUrl),
    ("a session URL trailer", Hard::SessionTrailer),
    ("the shipflow name", Hard::Literal("shipflow")),
    ("a focus:, gate: or track/ prefix", Hard::ProcessPrefix),
    ("a robot or sparkle emoji", Hard::Emoji),
];

#[derive(Clone, Copy)]
enum Hard {
    /// `co-authored-by:[[:space:]]*claude`
    CoAuthored,
    /// A fixed string, matched case-insensitively.
    Literal(&'static str),
    /// `https?://[a-z0-9.-]*claude\.(ai|com)/`
    ClaudeUrl,
    /// `^[[:space:]]*session(-url)?:[[:space:]]*https?://`
    SessionTrailer,
    /// `^[[:space:]]*(focus:|gate:|track/)`
    ProcessPrefix,
    Emoji,
}

fn is_sp(c: char) -> bool {
    c.is_ascii_whitespace()
}

fn skip_spaces(s: &str) -> &str {
    s.trim_start_matches(is_sp)
}

impl Hard {
    fn hits(&self, line: &str) -> bool {
        let lower = line.to_ascii_lowercase();
        match self {
            Hard::Literal(needle) => lower.contains(needle),
            Hard::CoAuthored => lower
                .match_indices("co-authored-by:")
                .any(|(i, m)| skip_spaces(&lower[i + m.len()..]).starts_with("claude")),
            Hard::ClaudeUrl => {
                for scheme in ["http://", "https://"] {
                    for (i, m) in lower.match_indices(scheme) {
                        let rest = &lower[i + m.len()..];
                        let host: String = rest
                            .chars()
                            .take_while(|c| {
                                c.is_ascii_lowercase()
                                    || c.is_ascii_digit()
                                    || *c == '.'
                                    || *c == '-'
                            })
                            .collect();
                        let after = &rest[host.len()..];
                        if after.starts_with('/')
                            && (host.ends_with("claude.ai") || host.ends_with("claude.com"))
                        {
                            return true;
                        }
                    }
                }
                false
            }
            Hard::SessionTrailer => {
                let head = skip_spaces(&lower);
                let rest = head
                    .strip_prefix("session-url:")
                    .or_else(|| head.strip_prefix("session:"));
                match rest {
                    Some(r) => {
                        let r = skip_spaces(r);
                        r.starts_with("http://") || r.starts_with("https://")
                    }
                    None => false,
                }
            }
            Hard::ProcessPrefix => {
                let head = skip_spaces(&lower);
                head.starts_with("focus:")
                    || head.starts_with("gate:")
                    || head.starts_with("track/")
            }
            Hard::Emoji => line.contains('🤖') || line.contains('✨'),
        }
    }
}

/// Tier two, vocabulary: warn, and a lint-exception ruling naming the term
/// clears that term and only that term.
///
/// `AI` and `LLM` are matched case-sensitively and the rest are not. That is a
/// decision, not an oversight: they are initialisms, and they are only process
/// vocabulary when they are written as initialisms. Matched case-insensitively,
/// `AI` catches the woman's name Ai, a variable called `ai`, and any language
/// where it is an ordinary word -- noise in exactly the messages the rule is
/// supposed to leave alone. `agent`, `subagent` and `orchestrat*` are ordinary
/// English words whose capitalisation carries nothing, so case is ignored there.
const WARN: &[(&str, Warn)] = &[
    ("AI", Warn::Word("AI", false)),
    ("agent", Warn::Word("agent", true)),
    ("LLM", Warn::Word("LLM", false)),
    ("subagent", Warn::Loose("subagent")),
    ("orchestrat", Warn::Loose("orchestrat")),
    ("spec review", Warn::Loose("spec review")),
    ("plan review", Warn::Loose("plan review")),
    ("ship review", Warn::Loose("ship review")),
];

#[derive(Clone, Copy)]
enum Warn {
    /// `(^|[^A-Za-z])<word>s?([^A-Za-z]|$)`; the flag is case-insensitivity.
    Word(&'static str, bool),
    /// A substring, case-insensitively.
    Loose(&'static str),
}

fn alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

fn word_hit(text: &str, word: &str, icase: bool) -> bool {
    let hay = if icase {
        text.to_ascii_lowercase()
    } else {
        text.to_string()
    };
    let needle = if icase {
        word.to_ascii_lowercase()
    } else {
        word.to_string()
    };
    let bytes = hay.as_bytes();
    for (i, _) in hay.match_indices(needle.as_str()) {
        if i > 0 && alpha(bytes[i - 1]) {
            continue;
        }
        let mut end = i + needle.len();
        if end < bytes.len() && (bytes[end] == b's' || bytes[end] == b'S') {
            end += 1;
        }
        if end >= bytes.len() || !alpha(bytes[end]) {
            return true;
        }
    }
    false
}

impl Warn {
    fn hits(&self, text: &str) -> bool {
        match self {
            Warn::Word(w, icase) => word_hit(text, w, *icase),
            Warn::Loose(w) => text.to_ascii_lowercase().contains(*w),
        }
    }
}

/// What git hands the commit-msg hook is the file it will use, commentary and
/// all: the scissors line, everything after it, and every comment line are not
/// the message.
pub fn strip_git_commentary(text: &str) -> String {
    let cc = Git::here()
        .out(&["config", "--get", "core.commentChar"])
        .unwrap_or_default();
    let cc = match cc.as_str() {
        "" | "auto" => '#',
        other => {
            let mut it = other.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => c,
                _ => '#',
            }
        }
    };

    let mut out = String::new();
    for line in text.lines() {
        if line.contains("------------------------ >8 ------------------------") {
            break;
        }
        if line.starts_with(cc) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 0 clean or warned, 1 hard fail. Prints its findings.
pub fn lint_text(text: &str) -> bool {
    let mut hard = false;
    for (label, rule) in HARD {
        if text.lines().any(|l| rule.hits(l)) {
            warn(format!("lint: {label} -- nothing clears this one."));
            hard = true;
        }
    }
    if hard {
        warn("rewrite the text in ordinary engineering voice and try again.");
        return false;
    }

    let mut warned: Vec<&str> = Vec::new();
    let mut exceptions: Option<String> = None;
    for (term, rule) in WARN {
        if !text.lines().any(|l| rule.hits(l)) {
            continue;
        }
        let body = exceptions.get_or_insert_with(|| memcli::ruling_bodies("lint-exception"));
        if !body
            .to_ascii_lowercase()
            .contains(&term.to_ascii_lowercase())
        {
            warned.push(term);
        }
    }
    if !warned.is_empty() {
        for term in &warned {
            warn(format!(
                "lint warning: \"{term}\" reads as process vocabulary here."
            ));
        }
        warn("if the word belongs to the product, clear it once --");
        warn(
            "  mem save --kind ruling --type lint-exception \"<term> is the product's own word - why - cost if wrong\"",
        );
    }
    true
}

pub fn cmd_lint_msg(msgfile: Option<&Path>, string: Option<&str>) -> i32 {
    let text = if let Some(file) = msgfile {
        match std::fs::read_to_string(file) {
            Ok(raw) => strip_git_commentary(&raw),
            Err(_) => {
                warn(format!("lint-msg: cannot read {}", file.display()));
                return exit::FAILED;
            }
        }
    } else if let Some(s) = string {
        s.to_string()
    } else if std::io::stdin().is_terminal() {
        println!("usage: workflow lint-msg [<msgfile>] [--string \"<text>\"]");
        return exit::OK;
    } else {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        buf
    };

    if lint_text(&text) {
        exit::OK
    } else {
        exit::FAILED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_hit(text: &str) -> bool {
        text.lines()
            .any(|l| HARD.iter().any(|(_, rule)| rule.hits(l)))
    }

    fn warn_terms(text: &str) -> Vec<&'static str> {
        WARN.iter()
            .filter(|(_, rule)| text.lines().any(|l| rule.hits(l)))
            .map(|(t, _)| *t)
            .collect()
    }

    #[test]
    fn the_provenance_tier_catches_every_shape_it_is_meant_to() {
        for text in [
            "Extract cart pricing\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
            "Extract cart pricing\n\nGenerated with Claude Code",
            "See https://claude.ai/code for the rest",
            "Session: https://claude.ai/s/018f2c7e-0000-7000-8000-000000000000",
            "Port the last of shipflow across",
            "focus: extract cart pricing",
            "gate: tighten the pre-commit check",
            "track/cart-pricing",
            "Extract cart pricing 🤖",
            "Extract cart pricing ✨",
            "  Session-URL:   http://claude.com/s/x",
        ] {
            assert!(hard_hit(text), "should have failed: {text:?}");
        }
    }

    #[test]
    fn ordinary_engineering_voice_passes_the_provenance_tier() {
        for text in [
            "Extract cart pricing into a service",
            "Fix the checkout total when a coupon is applied",
            "Note the session id in the run directory",
            "Move to https://example.com/code",
        ] {
            assert!(!hard_hit(text), "should have passed: {text:?}");
        }
    }

    #[test]
    fn the_initialisms_are_case_sensitive_and_the_english_words_are_not() {
        assert_eq!(warn_terms("Add an AI summary field"), vec!["AI"]);
        assert!(warn_terms("Rename the column to ai_summary").is_empty());
        assert!(warn_terms("Credit the photograph to Ai Nakamura").is_empty());
        assert!(warn_terms("Refresh the retry backoff again").is_empty());
        assert_eq!(warn_terms("Refresh the User Agent string"), vec!["agent"]);
        assert_eq!(warn_terms("Note the LLM token ceiling"), vec!["LLM"]);
        assert!(warn_terms("Note the llm token ceiling").is_empty());
        assert_eq!(
            warn_terms("Note the ORCHESTRATION step"),
            vec!["orchestrat"]
        );
        assert_eq!(
            warn_terms("Answer the spec review comments"),
            vec!["spec review"]
        );
    }

    #[test]
    fn the_plural_and_the_boundary_both_count() {
        assert_eq!(warn_terms("Retire the agents"), vec!["agent"]);
        assert!(warn_terms("The agency approved it").is_empty());
    }
}
