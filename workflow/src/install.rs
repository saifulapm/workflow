//! `workflow install` -- write what this binary carries where each tool looks
//! for it, and clear what older installs left behind.
//!
//! The binary embeds three directories of this repository (see `build.rs`):
//!
//!   skills/<name>/**   to ~/.claude/skills/<name>/** and ~/.agents/skills/<name>/**
//!   agents/<name>.md   to ~/.claude/agents/<name>.md
//!   hooks/<name>       to ~/.config/git/hooks/<name>, mode 0755
//!
//! Every path is made from `$HOME`. A symlink is never written through: a link
//! at the file, or at any directory between a destination root and the file,
//! is removed and replaced by the real thing, so the dotfiles a link pointed
//! into are left as they were. Inside a skill directory the binary ships,
//! files it no longer ships are deleted. Nothing else is touched: the
//! destination roots hold other people's skills, agents and hooks too.

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::{embedded, exit, paths, warn};

/// Skill directories older installs wrote that no skill is named after any
/// more. `workflow` is the old Claude Code plugin. A name the binary ships
/// again is not stale and stays.
const STALE_SKILLS: [&str; 10] = [
    "route",
    "lead",
    "fix",
    "garden",
    "research",
    "review",
    "work",
    "workflow",
    "implement",
    "roadmap",
];

/// The amx roles older installs wrote, in `~/.config/amx/agents`.
const STALE_ROLES: [&str; 11] = [
    "advisor",
    "dogfood",
    "fixer",
    "grill",
    "lead",
    "plan",
    "plan-refresh",
    "reader",
    "research",
    "review",
    "worker",
];

const HOOK_MODE: u32 = 0o755;

/// What an install did.
#[derive(Debug, Default)]
pub struct Report {
    /// One line per change, in the order made.
    pub lines: Vec<String>,
    pub written: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub failed: usize,
}

impl Report {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "installed: {} written, {} removed, {} unchanged",
            self.written, self.removed, self.unchanged
        );
        if self.failed > 0 {
            text.push_str(&format!(", {} failed", self.failed));
        }
        text
    }
}

/// One place an embedded file goes.
struct Dest {
    /// The directory the install owns a part of; nothing above it is checked.
    root: PathBuf,
    path: PathBuf,
    mode: Option<u32>,
}

fn skill_roots(home: &Path) -> [PathBuf; 2] {
    [home.join(".claude/skills"), home.join(".agents/skills")]
}

/// The places the embedded file at `rel` (a path below the repository) goes.
/// A file that belongs to none of the three directories goes nowhere, and a
/// file straight under `skills/` belongs to no skill.
fn destinations(home: &Path, rel: &str) -> Vec<Dest> {
    let Some((top, rest)) = rel.split_once('/') else {
        return Vec::new();
    };
    match top {
        "skills" if rest.contains('/') => skill_roots(home)
            .into_iter()
            .map(|root| Dest {
                path: root.join(rest),
                root,
                mode: None,
            })
            .collect(),
        "agents" => {
            let root = home.join(".claude/agents");
            vec![Dest {
                path: root.join(rest),
                root,
                mode: None,
            }]
        }
        "hooks" => {
            let root = home.join(".config/git/hooks");
            vec![Dest {
                path: root.join(rest),
                root,
                mode: Some(HOOK_MODE),
            }]
        }
        _ => Vec::new(),
    }
}

/// A path as the report shows it: `~/` for what is under `$HOME`.
fn show(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The directories between `root` and `path`, outermost first: the ones a
/// write to `path` goes through.
fn guarded<'a>(root: &Path, path: &'a Path) -> Vec<&'a Path> {
    let mut dirs: Vec<&Path> = path
        .ancestors()
        .skip(1)
        .take_while(|dir| *dir != root && dir.starts_with(root))
        .collect();
    dirs.reverse();
    dirs
}

/// Write the embedded files out, then remove what they replace. Never fails as
/// a whole: what could not be done is in the report.
pub fn install(home: &Path, files: &[(&str, &[u8])]) -> Report {
    let mut report = Report::default();
    // Each shipped skill and the files inside it, for the sweep below.
    let mut skills: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();

    for (rel, bytes) in files {
        for dest in destinations(home, rel) {
            if let Err(err) = place(&mut report, home, &dest, bytes) {
                report.failed += 1;
                report
                    .lines
                    .push(format!("failed {}: {err}", show(home, &dest.path)));
            }
        }
        if let Some(rest) = rel.strip_prefix("skills/")
            && let Some((name, inner)) = rest.split_once('/')
        {
            skills.entry(name).or_default().insert(inner);
        }
    }

    for (name, inner) in &skills {
        for root in skill_roots(home) {
            let dir = root.join(name);
            if !dir.is_symlink() {
                sweep(&mut report, home, &dir, "", inner);
            }
        }
    }

    for name in STALE_SKILLS {
        if skills.contains_key(name) {
            continue;
        }
        for root in skill_roots(home) {
            remove_stale(&mut report, home, &root.join(name));
        }
    }
    let roles = home.join(".config/amx/agents");
    for name in STALE_ROLES {
        remove_stale(&mut report, home, &roles.join(format!("{name}.md")));
    }
    report
}

/// Put one file in place: links out of the way first, then the bytes.
fn place(report: &mut Report, home: &Path, dest: &Dest, bytes: &[u8]) -> io::Result<()> {
    for dir in guarded(&dest.root, &dest.path) {
        if dir.is_symlink() {
            fs::remove_file(dir)?;
            report
                .lines
                .push(format!("replaced symlink {}", show(home, dir)));
        }
    }

    let mut verb = "wrote";
    match fs::symlink_metadata(&dest.path) {
        Err(_) => {}
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::remove_file(&dest.path)?;
            verb = "replaced symlink";
        }
        Ok(meta) if meta.is_file() => {
            let same_bytes = fs::read(&dest.path).is_ok_and(|have| have == bytes);
            let same_mode = dest
                .mode
                .is_none_or(|mode| meta.permissions().mode() & 0o777 == mode);
            if same_bytes && same_mode {
                report.unchanged += 1;
                return Ok(());
            }
            verb = "updated";
        }
        // A directory where a file belongs: the write below says so.
        Ok(_) => {}
    }

    if let Some(dir) = dest.path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&dest.path, bytes)?;
    if let Some(mode) = dest.mode {
        fs::set_permissions(&dest.path, fs::Permissions::from_mode(mode))?;
    }
    report.written += 1;
    report
        .lines
        .push(format!("{verb} {}", show(home, &dest.path)));
    Ok(())
}

/// Delete what is in `dir` and not in `keep`, which names the shipped files by
/// their path inside the directory, and the directories that leaves empty.
/// A link is deleted, never followed.
fn sweep(report: &mut Report, home: &Path, dir: &Path, prefix: &str, keep: &BTreeSet<&str>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let inner = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let real_dir = fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
        if real_dir {
            sweep(report, home, &path, &format!("{inner}/"), keep);
            if fs::read_dir(&path).is_ok_and(|mut left| left.next().is_none()) {
                remove(report, home, &path, fs::remove_dir(&path), "");
            }
        } else if !keep.contains(inner.as_str()) {
            remove(report, home, &path, fs::remove_file(&path), "");
        }
    }
}

/// Remove `path`, a name older installs wrote, whatever it is. Absent is fine.
fn remove_stale(report: &mut Report, home: &Path, path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    let gone = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    remove(report, home, path, gone, " (stale)");
}

fn remove(report: &mut Report, home: &Path, path: &Path, gone: io::Result<()>, note: &str) {
    match gone {
        Ok(()) => {
            report.removed += 1;
            report
                .lines
                .push(format!("removed {}{note}", show(home, path)));
        }
        Err(err) => {
            report.failed += 1;
            report
                .lines
                .push(format!("failed to remove {}: {err}", show(home, path)));
        }
    }
}

pub fn cmd_install() -> i32 {
    let Some(home) = paths::home() else {
        warn("install: HOME is not set");
        return exit::USAGE;
    };
    let report = install(&home, embedded::FILES);
    for line in &report.lines {
        println!("{line}");
    }
    println!("{}", report.summary());
    if report.failed > 0 {
        exit::FAILED
    } else {
        exit::OK
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::scratch::Scratch;

    /// A small table: one skill with a nested file, an agent, a hook.
    const FILES: &[(&str, &[u8])] = &[
        ("agents/helper.md", b"helper\n"),
        ("hooks/pre-commit", b"#!/bin/sh\nexit 0\n"),
        ("skills/alpha/SKILL.md", b"alpha\n"),
        ("skills/alpha/refs/one.md", b"one\n"),
    ];

    #[test]
    fn every_file_lands_where_its_tool_reads_it() {
        let home = Scratch::new("lands");
        let report = install(home.path(), FILES);

        assert_eq!(home.read(".claude/skills/alpha/SKILL.md"), "alpha\n");
        assert_eq!(home.read(".agents/skills/alpha/SKILL.md"), "alpha\n");
        assert_eq!(home.read(".claude/skills/alpha/refs/one.md"), "one\n");
        assert_eq!(home.read(".agents/skills/alpha/refs/one.md"), "one\n");
        assert_eq!(home.read(".claude/agents/helper.md"), "helper\n");
        assert_eq!(
            home.read(".config/git/hooks/pre-commit"),
            "#!/bin/sh\nexit 0\n"
        );
        let mode = fs::metadata(home.at(".config/git/hooks/pre-commit"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "a hook stub is executable");

        // Four copies of the skill's two files, the agent and the hook.
        assert_eq!(report.written, 6);
        assert_eq!(report.lines.len(), 6);
        assert!(
            report
                .lines
                .contains(&"wrote ~/.agents/skills/alpha/SKILL.md".to_string()),
            "{:?}",
            report.lines
        );
        assert_eq!(report.failed, 0);
        assert_eq!(
            report.summary(),
            "installed: 6 written, 0 removed, 0 unchanged"
        );
    }

    #[test]
    fn a_second_install_changes_nothing() {
        let home = Scratch::new("twice");
        install(home.path(), FILES);
        let report = install(home.path(), FILES);
        assert!(report.lines.is_empty(), "{:?}", report.lines);
        assert_eq!(report.unchanged, 6);
        assert_eq!(
            report.summary(),
            "installed: 0 written, 0 removed, 6 unchanged"
        );
    }

    #[test]
    fn a_changed_file_is_updated_and_a_hook_regains_its_mode() {
        let home = Scratch::new("update");
        install(home.path(), FILES);
        home.put(".claude/skills/alpha/SKILL.md", "edited by hand\n");
        fs::set_permissions(
            home.at(".config/git/hooks/pre-commit"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let report = install(home.path(), FILES);
        assert_eq!(home.read(".claude/skills/alpha/SKILL.md"), "alpha\n");
        let mode = fs::metadata(home.at(".config/git/hooks/pre-commit"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(
            report.lines,
            [
                "updated ~/.config/git/hooks/pre-commit",
                "updated ~/.claude/skills/alpha/SKILL.md",
            ]
        );
    }

    #[test]
    fn a_symlinked_file_becomes_a_file_and_its_target_is_left_alone() {
        let home = Scratch::new("link-file");
        home.put("dotfiles/helper.md", "the dotfiles copy\n");
        fs::create_dir_all(home.at(".claude/agents")).unwrap();
        symlink(
            home.at("dotfiles/helper.md"),
            home.at(".claude/agents/helper.md"),
        )
        .unwrap();

        let report = install(home.path(), FILES);

        let meta = fs::symlink_metadata(home.at(".claude/agents/helper.md")).unwrap();
        assert!(meta.is_file() && !meta.file_type().is_symlink());
        assert_eq!(home.read(".claude/agents/helper.md"), "helper\n");
        assert_eq!(home.read("dotfiles/helper.md"), "the dotfiles copy\n");
        assert!(
            report
                .lines
                .contains(&"replaced symlink ~/.claude/agents/helper.md".to_string()),
            "{:?}",
            report.lines
        );
    }

    #[test]
    fn a_symlinked_directory_is_replaced_and_never_written_through() {
        let home = Scratch::new("link-dir");
        home.put("dotfiles/alpha/SKILL.md", "the dotfiles copy\n");
        home.put("dotfiles/alpha/extra.md", "kept in the dotfiles\n");
        fs::create_dir_all(home.at(".claude/skills")).unwrap();
        symlink(home.at("dotfiles/alpha"), home.at(".claude/skills/alpha")).unwrap();

        let report = install(home.path(), FILES);

        let meta = fs::symlink_metadata(home.at(".claude/skills/alpha")).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink());
        assert_eq!(home.read(".claude/skills/alpha/SKILL.md"), "alpha\n");
        assert_eq!(home.read("dotfiles/alpha/SKILL.md"), "the dotfiles copy\n");
        assert_eq!(
            home.read("dotfiles/alpha/extra.md"),
            "kept in the dotfiles\n"
        );
        assert!(!home.has(".claude/skills/alpha/extra.md"));
        assert!(
            report
                .lines
                .contains(&"replaced symlink ~/.claude/skills/alpha".to_string()),
            "{:?}",
            report.lines
        );
    }

    #[test]
    fn the_stale_names_are_removed_and_only_those() {
        let home = Scratch::new("stale");
        for name in STALE_SKILLS {
            home.put(&format!(".claude/skills/{name}/SKILL.md"), "old\n");
        }
        home.put(".agents/skills/route/SKILL.md", "old\n");
        home.put(".agents/skills/workflow/hooks/hooks.json", "{}\n");
        for name in STALE_ROLES {
            home.put(&format!(".config/amx/agents/{name}.md"), "old\n");
        }
        // Someone else's: none of these is on a list.
        home.put(
            ".claude/skills/test-driven-development/SKILL.md",
            "theirs\n",
        );
        home.put(".claude/skills/desktop/SKILL.md", "theirs\n");
        home.put(".agents/skills/amx/SKILL.md", "theirs\n");
        home.put(".claude/agents/code-reviewer.md", "theirs\n");
        home.put(".config/amx/agents/mine.md", "theirs\n");
        home.put(".config/git/hooks/post-merge", "theirs\n");

        let report = install(home.path(), FILES);

        for name in STALE_SKILLS {
            assert!(!home.has(&format!(".claude/skills/{name}")), "{name}");
        }
        assert!(!home.has(".agents/skills/route"));
        assert!(!home.has(".agents/skills/workflow"));
        for name in STALE_ROLES {
            assert!(
                !home.has(&format!(".config/amx/agents/{name}.md")),
                "{name}"
            );
        }
        assert!(
            report
                .lines
                .contains(&"removed ~/.claude/skills/workflow (stale)".to_string()),
            "{:?}",
            report.lines
        );
        // Every stale skill in the Claude directory, two in the shared one, and
        // every role.
        assert_eq!(report.removed, STALE_SKILLS.len() + 2 + STALE_ROLES.len());

        for kept in [
            ".claude/skills/test-driven-development/SKILL.md",
            ".claude/skills/desktop/SKILL.md",
            ".agents/skills/amx/SKILL.md",
            ".claude/agents/code-reviewer.md",
            ".config/amx/agents/mine.md",
            ".config/git/hooks/post-merge",
        ] {
            assert_eq!(home.read(kept), "theirs\n", "{kept}");
        }
    }

    #[test]
    fn a_stale_name_is_removed_even_as_a_symlink_and_its_target_stays() {
        let home = Scratch::new("stale-link");
        home.put("dotfiles/route/SKILL.md", "dotfiles\n");
        fs::create_dir_all(home.at(".claude/skills")).unwrap();
        symlink(home.at("dotfiles/route"), home.at(".claude/skills/route")).unwrap();

        install(home.path(), FILES);

        assert!(!home.has(".claude/skills/route"));
        assert_eq!(home.read("dotfiles/route/SKILL.md"), "dotfiles\n");
    }

    #[test]
    fn a_name_the_binary_ships_is_not_stale() {
        let home = Scratch::new("shipped");
        home.put(".claude/skills/route/SKILL.md", "old\n");
        let files: &[(&str, &[u8])] = &[("skills/route/SKILL.md", b"new\n")];

        let report = install(home.path(), files);

        assert_eq!(home.read(".claude/skills/route/SKILL.md"), "new\n");
        assert_eq!(report.removed, 0);
    }

    #[test]
    fn a_file_the_binary_no_longer_ships_is_deleted_from_a_shipped_skill() {
        let home = Scratch::new("sweep");
        install(home.path(), FILES);
        home.put(".claude/skills/alpha/old.md", "gone\n");
        home.put(".claude/skills/alpha/refs/gone.md", "gone\n");
        home.put(".agents/skills/alpha/old/deep/notes.md", "gone\n");
        symlink(
            home.at("dotfiles"),
            home.at(".claude/skills/alpha/stray-link"),
        )
        .unwrap();

        let report = install(home.path(), FILES);

        for gone in [
            ".claude/skills/alpha/old.md",
            ".claude/skills/alpha/refs/gone.md",
            ".claude/skills/alpha/stray-link",
            ".agents/skills/alpha/old",
        ] {
            assert!(!home.has(gone), "{gone}");
        }
        assert_eq!(home.read(".claude/skills/alpha/refs/one.md"), "one\n");
        assert_eq!(home.read(".claude/skills/alpha/SKILL.md"), "alpha\n");
        assert!(
            report
                .lines
                .contains(&"removed ~/.claude/skills/alpha/old.md".to_string()),
            "{:?}",
            report.lines
        );
        // The three files, the link, and the two directories they emptied.
        assert_eq!(report.removed, 6, "{:?}", report.lines);
    }

    #[test]
    fn other_skills_files_are_left_in_a_dir_the_binary_does_not_ship() {
        let home = Scratch::new("others");
        home.put(".claude/skills/theirs/SKILL.md", "theirs\n");
        home.put(".claude/skills/theirs/extra.md", "theirs\n");

        install(home.path(), FILES);

        assert_eq!(home.read(".claude/skills/theirs/extra.md"), "theirs\n");
    }

    #[test]
    fn what_cannot_be_written_is_reported_and_the_rest_still_installs() {
        let home = Scratch::new("failed");
        // A file where the agents directory belongs.
        home.put(".claude/agents", "not a directory\n");

        let report = install(home.path(), FILES);

        assert_eq!(report.failed, 1, "{:?}", report.lines);
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.starts_with("failed ~/.claude/agents/helper.md: ")),
            "{:?}",
            report.lines
        );
        assert_eq!(home.read(".claude/skills/alpha/SKILL.md"), "alpha\n");
        assert!(report.summary().ends_with(", 1 failed"));
    }

    #[test]
    fn files_outside_the_three_directories_go_nowhere() {
        let home = Scratch::new("nowhere");
        let files: &[(&str, &[u8])] = &[
            ("README.md", b"x"),
            (
                "skills/loose.md",
                b"a file straight under skills belongs to no skill",
            ),
            ("elsewhere/file", b"x"),
        ];
        let report = install(home.path(), files);
        assert!(report.lines.is_empty() && report.unchanged == 0);
        assert!(!home.has(".claude"));
    }

    #[test]
    fn the_embedded_table_is_what_the_directories_hold() {
        let paths: Vec<&str> = embedded::FILES.iter().map(|(path, _)| *path).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "the table is in path order");
        assert!(
            paths.iter().all(|p| ["skills/", "agents/", "hooks/"]
                .iter()
                .any(|d| p.starts_with(d))),
            "{paths:?}"
        );
        for hook in ["commit-msg", "pre-commit", "pre-push"] {
            assert!(paths.contains(&format!("hooks/{hook}").as_str()), "{hook}");
        }
        assert!(paths.contains(&"skills/mem/SKILL.md"));
        assert!(
            !paths.iter().any(|p| p.contains("/.")),
            "hidden files are not carried: {paths:?}"
        );
        for path in paths.iter().filter(|p| p.starts_with("agents/")) {
            assert!(path.ends_with(".md"), "{path}");
        }
        let skills: BTreeSet<&str> = paths
            .iter()
            .filter_map(|p| p.strip_prefix("skills/")?.split('/').next())
            .collect();
        for skill in skills {
            assert!(
                paths.contains(&format!("skills/{skill}/SKILL.md").as_str()),
                "{skill} has no SKILL.md"
            );
        }
    }
}
