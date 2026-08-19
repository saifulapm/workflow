//! `workflow verify` -- the runtime-agnostic gate (spec §7).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::gitcmd::Git;
use crate::memcli::Project;
use crate::{exit, have, memcli, paths, repo, testdecl, warn};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Direct,
    Hook,
    Gate,
}

pub struct Verifier {
    pub label: &'static str,
    pub cmd: String,
}

/// The child suite is scrubbed of the git variables (so it sees the real repo,
/// not the commit in progress) and of the workflow variables (so its own
/// scratch commits do not re-enter the gate). Our own environment is never
/// touched -- review-3 F-3.
pub fn run_scrubbed(cmd: &str) -> bool {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    for key in [
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_PREFIX",
        "WORKFLOW_AGENT",
        "WORKFLOW_HOOK_SEEN",
    ] {
        c.env_remove(key);
    }
    c.status().map(|s| s.success()).unwrap_or(false)
}

fn json_file(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// package.json declares a script by that name worth running. What `npm init`
/// leaves behind is not a suite.
fn node_script(root: &Path, name: &str) -> bool {
    let Some(v) = json_file(&root.join("package.json")) else {
        return false;
    };
    let Some(s) = v
        .get("scripts")
        .and_then(|s| s.get(name))
        .and_then(|s| s.as_str())
    else {
        return false;
    };
    !s.is_empty() && !s.contains("no test specified")
}

/// composer.json declares `composer test` itself. The value may be a string or
/// a list of commands; either way it is declared.
fn composer_test_script(root: &Path) -> bool {
    let Some(v) = json_file(&root.join("composer.json")) else {
        return false;
    };
    match v.get("scripts").and_then(|s| s.get("test")) {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false)) => false,
        Some(_) => true,
    }
}

fn first_file(root: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| root.join(n)).find(|p| p.is_file())
}

/// A just recipe or a make target called exactly `test`: `test:`, `test: build`
/// and `test arg:` all count, `test-unit:` does not.
fn has_test_target(file: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.strip_prefix('@').unwrap_or(line);
        let Some(rest) = line.strip_prefix("test") else {
            return false;
        };
        let head_is_space = rest.starts_with(' ') || rest.starts_with('\t');
        (head_is_space && rest.contains(':'))
            || rest.trim_start_matches([' ', '\t']).starts_with(':')
    })
}

fn theme_repo(root: &Path) -> bool {
    root.join(".theme-check.yml").is_file()
        || root.join(".theme-check.yaml").is_file()
        || (root.join("config/settings_schema.json").is_file()
            && root.join("layout/theme.liquid").is_file())
}

fn executable(root: &Path, p: &str) -> bool {
    crate::gitcmd::exists_x(&root.join(p))
}

/// The ladder of spec §7. The project owns its checks; the workflow only
/// supplies a floor for what the project has not spoken for.
///
///   1. the `verify` command the project recorded in mem -- nothing else runs
///   2. the entry points the project declares: composer/pnpm scripts, a `test`
///      recipe in a justfile or a Makefile
///   3. the ecosystem floor, for whatever tier 2 did not cover
///
/// Tier 3 is a fallback, not an addition: a declared entry point beats the floor
/// for what it speaks for. `composer test` replaces the artisan/phpunit
/// detection, and a repo-wide `just test`/`make test` replaces the floor
/// altogether -- imposing `cargo clippy -- -D warnings` on a project that runs
/// its own checks is exactly the thing the ladder exists to stop. A language the
/// project said nothing about still gets its floor, so a Laravel repo that
/// declares a JavaScript suite does not quietly lose its PHP one.
pub fn detect_verifiers(root: &Path, project: Option<&Project>) -> Vec<Verifier> {
    let mut found: Vec<Verifier> = Vec::new();
    let mut add = |label: &'static str, cmd: String| found.push(Verifier { label, cmd });

    if let Some(cmd) = project.and_then(|p| p.verify.clone())
        && !cmd.is_empty()
    {
        add("project", cmd);
        return found;
    }

    let has_composer = root.join("composer.json").is_file();
    let mut declared_php = false;
    let mut declared_repo = false;

    if has_composer && composer_test_script(root) {
        add("composer", "composer test".into());
        declared_php = true;
    }
    if root.join("package.json").is_file() {
        if node_script(root, "test") {
            add("node", "pnpm run test".into());
        }
        if node_script(root, "lint") {
            add("node-lint", "pnpm lint".into());
        }
    }
    if let Some(f) = first_file(root, &["justfile", "Justfile", ".justfile", "JUSTFILE"])
        && has_test_target(&f)
    {
        add("just", "just test".into());
        declared_repo = true;
    }
    if let Some(f) = first_file(root, &["Makefile", "makefile", "GNUmakefile"])
        && has_test_target(&f)
    {
        add("make", "make test".into());
        declared_repo = true;
    }
    if declared_repo {
        return found;
    }

    if root.join("Cargo.toml").is_file() {
        add(
            "rust",
            "cargo test && cargo clippy -- -D warnings && cargo fmt --check".into(),
        );
    }

    if has_composer && !declared_php {
        // the per-project .php-version shim
        let php = if executable(root, "bin/php") {
            "./bin/php"
        } else {
            "php"
        };
        if root.join("artisan").is_file() {
            add("php", format!("{php} artisan test"));
        } else if executable(root, "vendor/bin/pest") {
            add("php", format!("{php} vendor/bin/pest"));
        } else if executable(root, "vendor/bin/phpunit") {
            add("php", format!("{php} vendor/bin/phpunit"));
        } else {
            warn("composer.json but no test script, artisan, pest or phpunit: no PHP verifier");
        }
    }

    if theme_repo(root) {
        if have("theme-check") {
            add("theme", "theme-check".into());
        } else if have("shopify") {
            add("theme", "shopify theme check".into());
        } else {
            warn("shopify theme detected but no theme-check on PATH: skipped");
        }
    }

    found
}

fn green_file(project: Option<&Project>) -> Option<PathBuf> {
    let id = &project?.id;
    if id.is_empty() {
        return None;
    }
    Some(paths::green_root().join(id))
}

fn cached_green(project: Option<&Project>) -> Option<String> {
    let f = green_file(project)?;
    Some(std::fs::read_to_string(f).ok()?.trim().to_string())
}

/// Only when the working tree matches the index, because only then did the
/// suite actually run over the tree being recorded.
fn record_green(git: &Git, project: Option<&Project>, tree: &str) {
    if tree.is_empty() || !git.quiet(&["diff", "--quiet"]) {
        return;
    }
    let Some(f) = green_file(project) else {
        return;
    };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(f, format!("{tree}\n"));
}

/// Every removed test is named by some test-removal ruling.
fn rulings_name_all(names: &[String]) -> bool {
    let body = memcli::ruling_bodies("test-removal");
    if body.is_empty() {
        return false;
    }
    names.iter().all(|n| body.contains(n.as_str()))
}

/// 0 when the staged diff is clear, an error when it nets away test
/// declarations nothing has ruled on (the caller turns that into exit 3).
fn check_test_removal(git: &Git) -> bool {
    let out = git.capture(&[
        "diff",
        "--cached",
        "-U0",
        "-M",
        "--no-color",
        "--no-ext-diff",
    ]);
    if !out.ok {
        return true;
    }
    let diff = String::from_utf8_lossy(&out.stdout).to_string();
    if diff.is_empty() {
        return true;
    }
    let report = testdecl::analyze(&diff);
    if report.net >= 0 {
        return true;
    }
    let names: Vec<String> = report.gone.iter().cloned().collect();

    if memcli::has_ruling("test-removal", Some("8h")) {
        return true;
    }
    if !names.is_empty() && rulings_name_all(&names) {
        return true;
    }

    warn(format!(
        "staged changes remove {} test declaration(s) with nothing ruling on it:",
        -report.net
    ));
    for n in &names {
        warn(format!("  - {n}"));
    }
    warn("record the decision, then re-run --");
    warn("  mem save --kind ruling --type test-removal \"<what - why - cost if wrong>\"");
    false
}

/// The exit-2 path. Location picks the lane (spec §7).
fn no_verifier() -> i32 {
    if paths::under_worktrees_root(paths::cwd()) {
        warn(
            "no verifier in this repo, and this is the plan lane (an orchestrator worktree): hard fail.",
        );
        warn("a task in an orchestrated plan must be verifiable; fix the plan or add a suite.");
        return exit::NO_VERIFIER;
    }
    if memcli::has_ruling("no-verifier", None) {
        warn("no verifier in this repo -- warning only, a no-verifier ruling is on file.");
        return exit::OK;
    }
    warn("no verifier in this repo (nothing to run).");
    warn("one-shot lane: record the decision, then re-run --");
    warn("  mem save --kind ruling --type no-verifier \"<what - why - cost if wrong>\"");
    exit::NO_VERIFIER
}

pub fn cmd_verify(mode: Mode) -> i32 {
    if !Git::here().inside_worktree() {
        warn("not inside a git work tree: nothing to verify");
        return exit::NO_VERIFIER;
    }
    let Some((git, top)) = repo::goto_toplevel() else {
        warn("cannot resolve the repository toplevel");
        return exit::NO_VERIFIER;
    };
    let project = memcli::project_current();

    if !check_test_removal(&git) {
        return exit::TEST_REMOVAL;
    }

    if mode == Mode::Hook && memcli::has_ruling("verify-optout", None) {
        warn("verify-optout ruling on file: the hook is lint-only here (the merge gate is not).");
        return exit::OK;
    }

    let tree = git.out(&["write-tree"]).unwrap_or_default();
    if mode == Mode::Hook
        && !tree.is_empty()
        && Some(&tree) == cached_green(project.as_ref()).as_ref()
    {
        warn("staged tree already verified: cached green, suite skipped.");
        return exit::OK;
    }

    let verifiers = detect_verifiers(&top, project.as_ref());
    if verifiers.is_empty() {
        return no_verifier();
    }

    let mut rc = exit::OK;
    for v in &verifiers {
        warn(format!("verify {}: {}", v.label, v.cmd));
        if !run_scrubbed(&v.cmd) {
            warn(format!("verify {}: FAILED", v.label));
            rc = exit::FAILED;
        }
    }
    if rc != exit::OK {
        return exit::FAILED;
    }

    record_green(&git, project.as_ref(), &tree);
    warn("verify: green");
    exit::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_target_is_the_one_called_exactly_test() {
        let dir = std::env::temp_dir().join(format!("wf-just-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("justfile");

        for (body, want) in [
            ("test:\n\ttouch ran\n", true),
            ("test: build\n\ttouch ran\n", true),
            ("test arg:\n\ttouch ran\n", true),
            ("@test:\n\ttouch ran\n", true),
            ("test-unit:\n\ttouch ran\n", false),
            ("testing:\n\ttouch ran\n", false),
            ("build:\n\ttouch ran\n", false),
        ] {
            std::fs::write(&f, body).unwrap();
            assert_eq!(has_test_target(&f), want, "{body:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
