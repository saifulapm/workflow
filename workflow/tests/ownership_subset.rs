//! The ownership fixture table of AC12: pattern → what it does and does not
//! claim, against a real repository, with the expectations written here rather
//! than read back out of the evaluator.

use std::path::PathBuf;
use std::process::Command;

use workflow::ownership::{show, violations};

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = std::env::temp_dir().join(format!("wf-own-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = Repo(dir);
        r.git(&["init", "-q", "."]);
        r.git(&["config", "user.email", "tester@example.invalid"]);
        r.git(&["config", "user.name", "Workflow Tester"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r.write("README.md", "seed\n");
        r.git(&["add", "README.md"]);
        r.commit("seed");
        r
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.0)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(&self, msg: &str) {
        self.git(&["-c", "core.hooksPath=/dev/null", "commit", "-qm", msg]);
    }

    fn write(&self, path: &str, body: &str) {
        let p = self.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn unowned(&self, base: &str, branch: &str, patterns: &[&str]) -> Vec<String> {
        let owned: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        show(&violations(&self.0, base, branch, &owned))
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn any(records: &[String], needle: &str) -> bool {
    records.iter().any(|r| r.contains(needle))
}

#[test]
fn the_working_tree_side_answers_the_fixture_table() {
    let r = Repo::new("tree");
    for f in [
        "app/Services/Cart.php",
        "app/Services/CartTotals.php",
        "app/Services/Cartx/Deep.php",
        "app/Services/With Space.php",
        "tests/Unit/CartTest.php",
        ".env",
        "config/.env.prod",
    ] {
        r.write(f, "x\n");
    }
    let base = r.head();

    let out = r.unowned(
        &base,
        "HEAD",
        &["app/Services/Cart*.php", "tests/Unit/Cart*"],
    );
    assert!(!any(&out, "app/Services/Cart.php"), "{out:?}");
    assert!(!any(&out, "CartTotals"), "{out:?}");
    assert!(!any(&out, "tests/Unit/CartTest"), "{out:?}");
    // `*` does not cross a slash.
    assert!(any(&out, "Cartx/Deep.php"), "{out:?}");
    assert!(any(&out, "With Space"), "{out:?}");
    assert!(any(&out, ".env"), "{out:?}");

    // `**` does cross, and reaches the root.
    let out = r.unowned(&base, "HEAD", &["**/.env*"]);
    assert!(!out.iter().any(|l| l.ends_with(" .env")), "{out:?}");
    assert!(!any(&out, "config/.env.prod"), "{out:?}");
    assert!(any(&out, "Cart"), "{out:?}");

    // Case matters here, deliberately: a case-mismatched write parks rather
    // than merging, which is the safe direction (spec §9).
    let out = r.unowned(&base, "HEAD", &["app/services/cart*.php"]);
    assert!(any(&out, "app/Services/Cart.php"), "{out:?}");
}

#[test]
fn a_rename_is_judged_whole_record_on_the_committed_side() {
    let r = Repo::new("rename");
    r.write("app/Services/Cart.php", "x\n");
    r.write("tests/Unit/CartTest.php", "x\n");
    r.git(&["add", "-A"]);
    r.commit("fixtures");
    let base = r.head();

    r.git(&["checkout", "-qb", "work"]);
    r.git(&[
        "mv",
        "app/Services/Cart.php",
        "app/Services/CartRenamed.php",
    ]);
    r.commit("rename inside ownership");
    let out = r.unowned(&base, "work", &["app/Services/Cart*.php"]);
    assert!(
        out.is_empty(),
        "a rename inside the patterns is owned: {out:?}"
    );

    r.git(&["mv", "tests/Unit/CartTest.php", "tests/Unit/BasketTest.php"]);
    r.commit("rename out of ownership");
    let out = r.unowned(
        &base,
        "work",
        &["app/Services/Cart*.php", "tests/Unit/Cart*"],
    );
    assert!(any(&out, "BasketTest"), "{out:?}");
    assert!(!any(&out, "CartRenamed"), "{out:?}");
    // The whole record travels together: the rename is one line, both paths.
    let rename = out.iter().find(|l| l.contains("BasketTest")).unwrap();
    assert!(rename.contains("CartTest"), "half a rename: {rename}");
}

#[test]
fn a_committed_file_outside_the_patterns_is_caught_even_with_a_clean_tree() {
    let r = Repo::new("committed");
    r.write("app/Services/Owned.php", "x\n");
    r.git(&["add", "-A"]);
    r.commit("fixtures");
    let base = r.head();

    r.git(&["checkout", "-qb", "work"]);
    r.write("NOTOWNED.txt", "outside\n");
    r.git(&["add", "NOTOWNED.txt"]);
    r.commit("reach outside the task");

    assert_eq!(r.git(&["status", "--porcelain"]), "", "the tree is clean");
    let out = r.unowned(&base, "work", &["app/Services/Owned.php"]);
    assert!(any(&out, "NOTOWNED.txt"), "{out:?}");
}

#[test]
fn a_task_that_claims_nothing_owns_nothing() {
    let r = Repo::new("nothing");
    r.write("app/One.php", "x\n");
    let base = r.head();
    let out = r.unowned(&base, "HEAD", &[]);
    assert!(any(&out, "app/One.php"), "{out:?}");
}

/// The friction the anchor exists for (#A2JXGNB8). A task whose dependency
/// landed on the integration branch has to bring that branch into its own
/// before it can build on it. Measured from the run's base, the diff then
/// charges the task with every file its siblings merged; measured from the
/// merge base with integration, it sees only what this task wrote.
#[test]
fn work_the_branch_only_inherited_from_integration_is_not_this_tasks() {
    let r = Repo::new("inherited");
    r.write("app/Services/Owned.php", "x\n");
    r.git(&["add", "-A"]);
    r.commit("fixtures");
    let base = r.head();

    // A sibling task's work, merged onto integration while this one waited.
    r.git(&["checkout", "-qb", "integration", &base]);
    r.write("app/Other/Sibling.php", "sibling\n");
    r.git(&["add", "-A"]);
    r.commit("Add the sibling service");

    // This task's branch, cut from the base, pulling integration in to get it.
    r.git(&["checkout", "-qb", "work", &base]);
    r.write("app/Services/Owned.php", "changed\n");
    r.git(&["add", "-A"]);
    r.commit("Change the owned service");
    r.git(&["merge", "-q", "--no-edit", "integration"]);

    let out = r.unowned("integration", "work", &["app/Services/Owned.php"]);
    assert!(
        out.is_empty(),
        "charged with a sibling's merged files: {out:?}"
    );

    // And the anchor does not hide what this task really did reach outside.
    r.write("NOTOWNED.txt", "outside\n");
    r.git(&["add", "-A"]);
    r.commit("Reach outside the task");
    let out = r.unowned("integration", "work", &["app/Services/Owned.php"]);
    assert!(any(&out, "NOTOWNED.txt"), "{out:?}");
    assert!(!any(&out, "Sibling"), "{out:?}");
}

/// The other arrangement, and the one that makes the anchor a merge base
/// rather than the branch tip: a task that never needed its siblings' work
/// sits where it was cut, and the integration branch has moved on without it.
/// Compared tip to tip, every file the siblings added reads as a deletion this
/// task made.
#[test]
fn a_branch_that_never_took_integration_in_is_not_charged_with_its_files() {
    let r = Repo::new("behind");
    r.write("app/Services/Owned.php", "x\n");
    r.git(&["add", "-A"]);
    r.commit("fixtures");
    let base = r.head();

    r.git(&["checkout", "-qb", "integration", &base]);
    r.write("app/Other/Sibling.php", "sibling\n");
    r.git(&["add", "-A"]);
    r.commit("Add the sibling service");

    r.git(&["checkout", "-qb", "work", &base]);
    r.write("app/Services/Owned.php", "changed\n");
    r.git(&["add", "-A"]);
    r.commit("Change the owned service");

    let out = r.unowned("integration", "work", &["app/Services/Owned.php"]);
    assert!(
        out.is_empty(),
        "a sibling's file read as this task's deletion: {out:?}"
    );
}
