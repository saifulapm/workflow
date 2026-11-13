//! The hub's list of destructive terminal lines.

/// Whether `line` holds a command on the hub's list of destructive ones.
pub fn held(line: &str) -> bool {
    // Quotes and shell punctuation become spaces, so a quoted or chained
    // command shows up as plain words. The hub matches words, not meaning.
    let text: String = line
        .chars()
        .map(|c| {
            if "'\"`;&|(){}<>$\n".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let words: Vec<&str> = text.split_whitespace().collect();
    (0..words.len()).any(|i| destructive_at(&words, i))
}

/// Whether the command whose program word is `words[i]` is on the list.
fn destructive_at(words: &[&str], i: usize) -> bool {
    let rest = &words[i + 1..];
    // `\rm` skips an alias and `npx wrangler@3` names a version: both are
    // still the program.
    let program = words[i].trim_start_matches('\\');
    let program = program.rsplit('/').next().unwrap_or_default();
    let program = program.split('@').next().unwrap_or_default();
    match program {
        // GNU rm takes its flags after the paths too.
        "rm" => rest.iter().any(|w| recursive(w)),
        // A thread's own yes, written as Saiful's from the device's shell, or
        // its own choice of secrets or device, written as a device's.
        "mem" => {
            rest.contains(&"answer")
                || (rest.contains(&"project")
                    && (rest.contains(&"set") || rest.contains(&"unset"))
                    && (rest.contains(&"cloud-secrets") || rest.contains(&"device")))
        }
        // A value read back through the pane would reach the cloud. Only the
        // plain forms: the hub matches words, and a script beats any list.
        "pass" => rest
            .first()
            .is_some_and(|w| matches!(*w, "show" | "otp" | "-c" | "--clip") || w.contains('/')),
        "gpg" => rest.iter().any(|w| matches!(*w, "-d" | "--decrypt")),
        "git" => git_force_push(rest),
        "gh" => rest.starts_with(&["repo", "delete"]) || rest.starts_with(&["auth", "token"]),
        "cf" => rest.contains(&"delete"),
        "wrangler" => rest.contains(&"delete") || (rest.contains(&"deploy") && !staging(rest)),
        "shopify" => matches!(rest, ["app", "release" | "deploy", ..]),
        _ => false,
    }
}

fn recursive(flag: &str) -> bool {
    flag == "--recursive"
        || (flag.starts_with('-') && !flag.starts_with("--") && flag.contains(['r', 'R']))
}

/// `push` after `git`, up to the next `git`, followed by a force flag.
fn git_force_push(rest: &[&str]) -> bool {
    let rest = rest.split(|w| *w == "git").next().unwrap_or_default();
    let Some(push) = rest.iter().position(|w| *w == "push") else {
        return false;
    };
    rest[push + 1..].iter().any(|w| force(w))
}

fn force(word: &str) -> bool {
    word.starts_with('+')
        || matches!(word, "--force" | "--force-if-includes")
        || word.starts_with("--force-with-lease")
        || (word.starts_with('-') && !word.starts_with("--") && word.contains('f'))
}

/// `--env staging`, `-e staging`, `--env=staging` or `-e=staging`.
fn staging(rest: &[&str]) -> bool {
    rest.iter().enumerate().any(|(n, w)| match *w {
        "--env" | "-e" => rest.get(n + 1) == Some(&"staging"),
        "--env=staging" | "-e=staging" => true,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::held;

    #[test]
    fn holds_destructive_lines() {
        for line in [
            "rm -r scratch",
            "git push --force",
            "git push -f",
            "gh repo delete x",
            "cf dns records delete 1",
            "wrangler delete",
            "wrangler deploy",
            "shopify app release",
            "echo 'rm -r x'",
            "true && rm -r x",
            "rm -rf /tmp/x",
            "/bin/rm -R x",
            "git push origin +main",
            "git push --force-with-lease",
            "shopify app deploy",
            "cd x;rm -r y",
            "\\rm -rf build",
            "rm build -r",
            "npx wrangler@latest deploy",
            "bunx wrangler@3 delete",
            "mem answer AAAA1111 yes",
            "mem --project cloud_walk answer AAAA1111 yes",
            "pass show Shopify/ShortCart/token",
            "pass Shopify/ShortCart/token",
            "pass -c Shopify/x",
            "pass otp otp/x",
            "echo $(pass show x/y)",
            "gpg -d secret.gpg",
            "gpg --decrypt ~/.password-store/x.gpg",
            "gh auth token",
            "mem --project pawsome project set cloud-secrets 'X=Shopify/x'",
            "mem project unset cloud-secrets",
            "mem project set device nuc",
        ] {
            assert!(held(line), "should hold: {line}");
        }
    }

    #[test]
    fn lets_other_lines_through() {
        for line in [
            "wrangler deploy --env staging",
            "workflow shopify deploy shortcart",
            "gh repo view x",
            "C-c",
            "rm x",
            "rm -f x",
            "git push",
            "git push origin main",
            "git commit -m force",
            "ls -r",
            "echo deploy",
            "wrangler dev",
            "wrangler deploy -e staging",
            "mem questions",
            "echo answer",
            "git commit -m 'tests pass'",
            "pass",
            "pass ls",
            "gh auth status",
            "mem project current",
            "mem project set verify 'just test'",
            "gpg --list-keys",
        ] {
            assert!(!held(line), "should pass: {line}");
        }
    }
}
