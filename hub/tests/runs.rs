//! Where a project's checkout is on this machine, read off the projects
//! list: the directory `workflow status --json` runs in.

mod common;

use common::{TempDir, fixture_mem};
use hub::memcli::MemCli;
use hub::model;

/// A projects doc with two rows: one with checkouts, one without, enough
/// to prove `checkout_of` takes the first entry and answers `None` for an
/// empty array or an unknown name.
const PROJECTS_DOC: &str = r#"{"projects":[
    {"id":"p1","name":"proj-alpha","remote":null,"aliases":[],"created":"2026-01-01",
     "items":0,"current":false,"checkouts":["/checkouts/alpha","/checkouts/alpha-old"]},
    {"id":"p2","name":"proj-beta","remote":null,"aliases":[],"created":"2026-01-01",
     "items":0,"current":false,"checkouts":[]}
]}"#;

#[test]
fn checkout_of_runs_reads_the_first_registered_checkout() {
    let dir = TempDir::new("runs-checkout-of");
    let bin = dir.join("bin");
    fixture_mem(&bin, &format!("echo '{PROJECTS_DOC}'"));
    let mem = MemCli::with_path(bin);

    assert_eq!(
        model::checkout_of(&mem, "proj-alpha").as_deref(),
        Some("/checkouts/alpha")
    );
    assert_eq!(
        model::checkout_of(&mem, "proj-beta"),
        None,
        "an empty checkouts array is no checkout"
    );
    assert_eq!(
        model::checkout_of(&mem, "proj-unknown"),
        None,
        "mem does not know this project"
    );
}
