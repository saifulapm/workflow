//! The release tag a device tmux line pushes, which lets one yes to the
//! line cover its next pushes of that tag.

/// How many more times a yes to a line that pushes a release tag runs it.
pub const REPUSHES: u32 = 3;

/// What a yes to a line that pushes `tag` covers, in the device's
/// question. Only a yes to a question that says so is spent again.
pub fn terms(tag: &str) -> String {
    format!(
        "Yes runs it now, and this same line up to {REPUSHES} more times within 24 hours of your yes to push {tag} again; Slack shows each re-push."
    )
}

/// The one tag `line` pushes, named in a `git push` as `refs/tags/<tag>`.
/// None for a line that pushes no tag or more than one, and for any line
/// the hub cannot read word for word: a quote, `$`, a backtick, a comment,
/// a pipe or a subshell may hide or change what runs.
pub fn pushed_tag(line: &str) -> Option<String> {
    if line.contains([
        '\'', '"', '`', '$', '#', '\\', '|', '(', ')', '{', '}', '<', '>', '\n',
    ]) || line.replace("&&", "").contains('&')
    {
        return None;
    }
    let mut tags: Vec<String> = Vec::new();
    for command in line.split("&&").flat_map(|c| c.split(';')) {
        for tag in command_tags(command)? {
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    match tags.as_slice() {
        [tag] => Some(tag.clone()),
        _ => None,
    }
}

/// The tags one command pushes: none for a command that is not
/// `git push`, None for a push the hub cannot bound.
fn command_tags(command: &str) -> Option<Vec<String>> {
    let words: Vec<&str> = command
        .split_whitespace()
        .skip_while(|w| assignment(w))
        .collect();
    let ["git", "push", rest @ ..] = words.as_slice() else {
        return Some(Vec::new());
    };
    let mut names = Vec::new();
    for word in rest {
        match *word {
            // A value of its own word, or every ref of a kind.
            "-o" | "--push-option" | "--repo" | "--receive-pack" | "--exec" | "--tags"
            | "--all" | "--mirror" | "--follow-tags" => return None,
            w if w.starts_with('-') => {}
            w => names.push(w),
        }
    }
    // The first name is the remote; the rest are refspecs.
    let mut tags = Vec::new();
    for spec in names.iter().skip(1) {
        let spec = spec.trim_start_matches('+');
        let dst = spec.rsplit(':').next().unwrap_or_default();
        if let Some(tag) = dst.strip_prefix("refs/tags/") {
            if tag.is_empty()
                || !tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
            {
                return None;
            }
            tags.push(tag.to_string());
        }
    }
    Some(tags)
}

/// The tell for re-push `n` of `tag`: git's `--shortstat` of its diff
/// since `sha`, the HEAD of the push Saiful said yes to, with every count.
pub fn re_push_tell(n: u32, tag: &str, shortstat: &str, sha: &str) -> String {
    let count = |word: &str| {
        shortstat
            .split(',')
            .find(|part| part.contains(word))
            .and_then(|part| part.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
    };
    let counted =
        |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let short = sha.get(..7).unwrap_or(sha);
    format!(
        "re-push {n} of {REPUSHES} of {tag}: {}, {}, {} since {short}",
        counted(count("changed"), "file changed", "files changed"),
        counted(count("insertion"), "insertion", "insertions"),
        counted(count("deletion"), "deletion", "deletions"),
    )
}

/// The Unix milliseconds a ULID was minted at, from its first ten
/// characters; None for anything that is not a ULID.
pub fn ulid_ms(id: &str) -> Option<u64> {
    const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if id.len() != 26 || !id.starts_with(|c| ('0'..='7').contains(&c)) {
        return None;
    }
    id.chars()
        .try_fold(0u128, |n, c| Some(n * 32 + CROCKFORD.find(c)? as u128))?;
    id[..10]
        .chars()
        .try_fold(0u64, |ms, c| Some(ms * 32 + CROCKFORD.find(c)? as u64))
}

/// `NAME=value` before a command.
fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::{pushed_tag, re_push_tell, ulid_ms};

    #[test]
    fn a_push_naming_one_tag_by_its_ref_pushes_that_tag() {
        for line in [
            "git push origin refs/tags/v1",
            "git push -f origin refs/tags/v1",
            "git tag -f v1 && git push -f origin refs/tags/v1",
            "WORKFLOW_ALLOW_PUSH=1 git push origin refs/tags/v1",
            "git push origin +refs/tags/v1",
            "git push origin 4438a764:refs/tags/v1",
            "git push origin HEAD:refs/heads/main refs/tags/v1",
            "git push origin :refs/tags/v1; git push origin refs/tags/v1",
        ] {
            assert_eq!(pushed_tag(line).as_deref(), Some("v1"), "{line}");
        }
        assert_eq!(
            pushed_tag("git push origin refs/tags/ui-v2.2.0").as_deref(),
            Some("ui-v2.2.0")
        );
    }

    #[test]
    fn a_re_push_tell_counts_files_insertions_and_deletions() {
        let sha = "7361ebd0c0ffee";
        assert_eq!(
            re_push_tell(1, "v1", " 1 file changed, 1 insertion(+)\n", sha),
            "re-push 1 of 3 of v1: 1 file changed, 1 insertion, 0 deletions since 7361ebd"
        );
        assert_eq!(
            re_push_tell(
                3,
                "v1",
                " 4 files changed, 10 insertions(+), 1 deletion(-)",
                sha
            ),
            "re-push 3 of 3 of v1: 4 files changed, 10 insertions, 1 deletion since 7361ebd"
        );
        assert_eq!(
            re_push_tell(2, "v1", "", sha),
            "re-push 2 of 3 of v1: 0 files changed, 0 insertions, 0 deletions since 7361ebd"
        );
    }

    #[test]
    fn a_ulid_s_time_is_its_first_ten_characters() {
        // The ULID spec's own example.
        let at: jiff::Timestamp = "2016-07-30T23:54:10.259Z".parse().unwrap();
        assert_eq!(
            ulid_ms("01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            Some(at.as_millisecond() as u64)
        );
        // A mem answer, created at 19:07:15.576: its id is minted moments after.
        let created: jiff::Timestamp = "2026-11-18T19:07:15.576Z".parse().unwrap();
        let minted = ulid_ms("01M7R0WQ98NQV75E7DEAZJZJGS").unwrap();
        assert!((0..1000).contains(&(minted - created.as_millisecond() as u64)));
        for id in [
            "01ANSWER",
            "",
            "81M7R0WQ98NQV75E7DEAZJZJGS",
            "01M7R0WQ98NQV75E7DEAZJZJGU",
        ] {
            assert_eq!(ulid_ms(id), None, "{id}");
        }
    }

    #[test]
    fn a_line_the_hub_cannot_bound_pushes_no_tag() {
        for line in [
            "git push origin main v1",
            "git push origin tag v1",
            "git commit -m \"prepare refs/tags/v1\"",
            "git commit -m 'git push origin refs/tags/v1'",
            "echo git push origin refs/tags/v1",
            "git push origin refs/tags/v1 refs/tags/v2",
            "git push origin refs/tags/v1 && git push origin refs/tags/v2",
            "git push --tags origin",
            "git push --follow-tags origin refs/tags/v1",
            "git push -o ci.skip origin refs/tags/v1",
            "git push origin refs/tags/$TAG",
            "git push origin refs/tags/v1 # note",
            "git push origin refs/tags/v1 | cat",
            "git push origin refs/tags/v1 & rm -r x",
            "git push origin refs/tags/",
            "git -C x push origin refs/tags/v1",
            "git push origin refs/tags/v1\nrm -r x",
        ] {
            assert_eq!(pushed_tag(line), None, "{line}");
        }
    }
}
