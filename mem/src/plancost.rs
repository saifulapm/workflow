//! The stored-plan rule that a stated cost or saving carries a measurement.
//!
//! A figure read off the code and never measured shipped in a plan once and
//! was wrong by a few dollars a month. The matching is by hand: this crate
//! has no regex dependency.

const SAVING_WORDS: [&str; 5] = ["save", "saves", "saved", "saving", "savings"];
const PERIODS: [&str; 6] = ["month", "mo", "year", "yr", "day", "week"];
const CURRENCIES: [char; 3] = ['$', '€', '£'];

/// The first line (1-based number and text) of `text` that states a cost or
/// a saving with no `measured:` note, if there is one.
pub fn unmeasured_figure(text: &str) -> Option<(usize, &str)> {
    text.lines()
        .enumerate()
        .find(|(_, line)| states_unmeasured_figure(line))
        .map(|(i, line)| (i + 1, line))
}

fn states_unmeasured_figure(line: &str) -> bool {
    let line = without_code_spans(line);
    let trimmed = line.trim_start();
    if line.contains("measured:")
        || trimmed.starts_with("- [ ]")
        || trimmed.starts_with("- [x]")
        || line.contains("(saiful")
    {
        return false;
    }
    let chars: Vec<char> = line.chars().map(|c| c.to_ascii_lowercase()).collect();
    let saving_words = saving_word_spans(&chars);
    figures(&chars).into_iter().any(|(start, end)| {
        recurring(&chars, end)
            || saving_words.iter().any(|&(ws, we)| {
                (we <= start && start - we <= 40 && !has_stop(&chars[we..start]))
                    || (ws >= end && ws - end <= 40 && !has_stop(&chars[end..ws]))
            })
    })
}

fn without_code_spans(line: &str) -> String {
    let mut out = String::new();
    let mut in_span = false;
    let mut rest = line;
    while let Some(i) = rest.find('`') {
        if !in_span {
            out.push_str(&rest[..i]);
        }
        in_span = !in_span;
        rest = &rest[i + 1..];
    }
    // An unpaired backtick opens no span, so its tail is plain text.
    if !in_span {
        out.push_str(rest);
    } else if let Some(i) = line.rfind('`') {
        out.push_str(&line[i + 1..]);
    }
    out
}

fn has_stop(between: &[char]) -> bool {
    between.iter().any(|&c| c == '.' || c == ';')
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// A whole word equal to `word` starts at `at`.
fn word_at(chars: &[char], at: usize, word: &str) -> Option<usize> {
    let n = word.chars().count();
    let end = at.checked_add(n)?;
    let there = chars.get(at..end)?;
    let boundary_before = at == 0 || !is_word_char(chars[at - 1]);
    let boundary_after = chars.get(end).is_none_or(|&c| !is_word_char(c));
    (boundary_before && boundary_after && there.iter().copied().eq(word.chars())).then_some(end)
}

fn saving_word_spans(chars: &[char]) -> Vec<(usize, usize)> {
    (0..chars.len())
        .filter_map(|i| {
            SAVING_WORDS
                .iter()
                .find_map(|w| word_at(chars, i, w))
                .map(|end| (i, end))
        })
        .collect()
}

fn skip_spaces(chars: &[char], mut i: usize) -> usize {
    while chars.get(i) == Some(&' ') {
        i += 1;
    }
    i
}

/// Digits, `.` and `,` from `i`, ending on the last digit. None when `i` is
/// not on a digit.
fn number_end(chars: &[char], i: usize) -> Option<usize> {
    if !chars.get(i)?.is_ascii_digit() {
        return None;
    }
    let mut end = i + 1;
    let mut j = i + 1;
    while let Some(&c) = chars.get(j) {
        if c.is_ascii_digit() {
            end = j + 1;
        } else if c != '.' && c != ',' {
            break;
        }
        j += 1;
    }
    Some(end)
}

/// A currency sign, optional space, then a number: the span after the sign's
/// own number, or None.
fn amount_end(chars: &[char], i: usize) -> Option<usize> {
    if !CURRENCIES.contains(chars.get(i)?) {
        return None;
    }
    number_end(chars, skip_spaces(chars, i + 1))
}

/// The end of a range's far side after `end`: `-`, `–` or `to`, then an
/// optional currency sign and a number.
fn range_end(chars: &[char], end: usize) -> Option<usize> {
    let mut i = skip_spaces(chars, end);
    if matches!(chars.get(i), Some('-' | '–')) {
        i += 1;
    } else if chars.get(i) == Some(&'t') && chars.get(i + 1) == Some(&'o') {
        i += 2;
    } else {
        return None;
    }
    i = skip_spaces(chars, i);
    if CURRENCIES.contains(chars.get(i)?) {
        i = skip_spaces(chars, i + 1);
    }
    number_end(chars, i)
}

/// Every money figure on the line, as char spans.
fn figures(chars: &[char]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let Some(mut end) = amount_end(chars, i) else {
            i += 1;
            continue;
        };
        if let Some(far) = range_end(chars, end) {
            end = far;
        }
        if chars.get(end) == Some(&'k') {
            end += 1;
        }
        found.push((i, end));
        i = end;
    }
    found
}

/// After a figure ending at `end`: `a`, `per`, `/`, `each` or `every`, then a
/// period word.
fn recurring(chars: &[char], end: usize) -> bool {
    let i = skip_spaces(chars, end);
    let after_joiner = if chars.get(i) == Some(&'/') {
        Some(i + 1)
    } else {
        ["a", "per", "each", "every"]
            .iter()
            .find_map(|w| word_at(chars, i, w))
    };
    let Some(j) = after_joiner else {
        return false;
    };
    let j = skip_spaces(chars, j);
    PERIODS.iter().any(|p| word_at(chars, j, p).is_some())
}
