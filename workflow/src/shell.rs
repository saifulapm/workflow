//! A bash command line read as its simple commands, for the guards: enough of
//! bash's grammar to tell a command from text, not a shell. Quotes are taken
//! off a word and their text kept, so `cd "/a b"` still has its target and a
//! commit message that names a test runner is one word, never a command. A
//! heredoc's body is text, and the line after its delimiter is commands
//! again. `\` and a newline join two lines into one.

/// What follows a simple command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `;`, a newline, a subshell's edge, or the end of the line.
    Seq,
    And,
    Or,
    /// `|` or `|&`.
    Pipe,
    /// `&`.
    Background,
}

#[derive(Debug)]
pub struct Simple {
    pub words: Vec<String>,
    /// How many subshells it runs inside: `( )`, `$( )` and backticks.
    pub depth: usize,
    pub then: Op,
}

/// The simple commands of `line` in order, or None when it is no command
/// line bash would run whole: an unclosed quote, subshell or backtick.
pub fn commands(line: &str) -> Option<Vec<Simple>> {
    let mut lexer = Lexer::default();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        i += 1;
        match c {
            '\\' => match next {
                Some('\n') => i += 1,
                Some(escaped) => {
                    lexer.push(escaped);
                    i += 1;
                }
                None => {}
            },
            '\'' => {
                let end = chars[i..].iter().position(|&q| q == '\'')?;
                lexer.word.get_or_insert_default();
                chars[i..i + end].iter().for_each(|&q| lexer.push(q));
                i += end + 1;
            }
            '"' => {
                lexer.word.get_or_insert_default();
                loop {
                    match *chars.get(i)? {
                        '"' => break,
                        '\\' if matches!(chars.get(i + 1), Some('"' | '\\' | '$' | '`')) => {
                            lexer.push(chars[i + 1]);
                            i += 2;
                        }
                        '\\' if chars.get(i + 1) == Some(&'\n') => i += 2,
                        q => {
                            lexer.push(q);
                            i += 1;
                        }
                    }
                }
                i += 1;
            }
            '#' if lexer.word.is_none() => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ' ' | '\t' => lexer.end_word(),
            '\n' => {
                lexer.end(Op::Seq);
                i = lexer.skip_heredocs(&chars, i);
            }
            ';' => lexer.end(Op::Seq),
            '&' if next == Some('&') => {
                lexer.end(Op::And);
                i += 1;
            }
            // `2>&1`, `>&2` and `&>file` are redirections, part of a word.
            '&' if lexer
                .word
                .as_deref()
                .is_some_and(|w| w.ends_with(['>', '<']))
                || next == Some('>') =>
            {
                lexer.push('&')
            }
            '&' => lexer.end(Op::Background),
            '|' if next == Some('|') => {
                lexer.end(Op::Or);
                i += 1;
            }
            '|' => {
                lexer.end(Op::Pipe);
                if next == Some('&') {
                    i += 1;
                }
            }
            '<' if next == Some('<') && chars.get(i + 1) == Some(&'<') => {
                // A here-string is a word.
                lexer.push_str("<<<");
                i += 2;
            }
            '<' if next == Some('<') => {
                lexer.end_word();
                i += 1;
                let strip_tabs = chars.get(i) == Some(&'-');
                if strip_tabs {
                    i += 1;
                }
                while matches!(chars.get(i), Some(' ' | '\t')) {
                    i += 1;
                }
                let mut delimiter = String::new();
                while let Some(&d) = chars.get(i) {
                    if d.is_whitespace() || matches!(d, ';' | '&' | '|' | '<' | '>' | '(' | ')') {
                        break;
                    }
                    if !matches!(d, '\'' | '"' | '\\') {
                        delimiter.push(d);
                    }
                    i += 1;
                }
                lexer.heredocs.push((delimiter, strip_tabs));
            }
            '(' => {
                // `$(` opens a subshell as `(` does; `$((` an arithmetic one,
                // which holds no command anyone would guard.
                if let Some(word) = lexer.word.as_mut()
                    && word.ends_with('$')
                {
                    word.pop();
                    if word.is_empty() {
                        lexer.word = None;
                    }
                }
                lexer.end(Op::Seq);
                lexer.depth += 1;
            }
            ')' => {
                lexer.end(Op::Seq);
                lexer.depth = lexer.depth.checked_sub(1)?;
            }
            '`' => {
                lexer.end(Op::Seq);
                if lexer.in_tick {
                    lexer.depth = lexer.depth.checked_sub(1)?;
                } else {
                    lexer.depth += 1;
                }
                lexer.in_tick = !lexer.in_tick;
            }
            _ => lexer.push(c),
        }
    }
    lexer.end(Op::Seq);
    (lexer.depth == 0 && !lexer.in_tick).then_some(lexer.out)
}

#[derive(Default)]
struct Lexer {
    out: Vec<Simple>,
    words: Vec<String>,
    word: Option<String>,
    depth: usize,
    in_tick: bool,
    /// The heredocs opened on this line, whose bodies start at its end.
    heredocs: Vec<(String, bool)>,
}

impl Lexer {
    fn push(&mut self, c: char) {
        self.word.get_or_insert_default().push(c);
    }

    fn push_str(&mut self, s: &str) {
        self.word.get_or_insert_default().push_str(s);
    }

    fn end_word(&mut self) {
        if let Some(word) = self.word.take() {
            self.words.push(word);
        }
    }

    fn end(&mut self, then: Op) {
        self.end_word();
        if self.words.is_empty() {
            // `a &&\nb` is one list: the newline after `&&` ends nothing.
            return;
        }
        self.out.push(Simple {
            words: std::mem::take(&mut self.words),
            depth: self.depth,
            then,
        });
    }

    /// Past the bodies of the heredocs opened on the line that ended at
    /// `i`: each runs to a line that is its delimiter alone.
    fn skip_heredocs(&mut self, chars: &[char], mut i: usize) -> usize {
        for (delimiter, strip_tabs) in std::mem::take(&mut self.heredocs) {
            while i < chars.len() {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(chars.len(), |n| i + n);
                let line: String = chars[i..end].iter().collect();
                i = (end + 1).min(chars.len());
                let line = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if line == delimiter {
                    break;
                }
            }
        }
        i
    }
}

/// The words from the program on: past variable assignments, keywords such as
/// `if` and `!`, wrappers such as `timeout 600` and `env`, and the package
/// runners that run a tool by name (`npx`, `pnpm exec`).
pub fn argv(words: &[String]) -> &[String] {
    let mut at = 0;
    while let Some(word) = words.get(at) {
        let name = program_name(word);
        at += if is_assignment(word) || is_keyword(word) {
            1
        } else if name == "command"
            && matches!(words.get(at + 1).map(String::as_str), Some("-v" | "-V"))
        {
            // `command -v vitest` looks vitest up and runs nothing.
            return &[];
        } else if is_wrapper(name) {
            1 + words[at + 1..]
                .iter()
                .take_while(|w| w.starts_with('-') || w.starts_with(|c: char| c.is_ascii_digit()))
                .count()
        } else if matches!(name, "npx" | "bunx") {
            1
        } else if matches!(name, "pnpm" | "npm" | "yarn" | "bun")
            && matches!(
                words.get(at + 1).map(String::as_str),
                Some("exec" | "dlx" | "x")
            )
        {
            2
        } else {
            break;
        };
    }
    &words[at.min(words.len())..]
}

/// A program's name without its folder: `node_modules/.bin/vitest` is vitest.
pub fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The first word after `argv[0]` that is no option: a subcommand.
pub fn subcommand(argv: &[String]) -> Option<&str> {
    argv.iter()
        .skip(1)
        .map(String::as_str)
        .find(|w| !w.starts_with('-') && !w.starts_with('+'))
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "{" | "}" | "!" | "if" | "then" | "else" | "elif" | "do" | "while" | "until"
    )
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
    })
}

fn is_wrapper(name: &str) -> bool {
    matches!(
        name,
        "env" | "command" | "exec" | "time" | "nice" | "timeout" | "nohup" | "sudo" | "xargs"
    )
}
