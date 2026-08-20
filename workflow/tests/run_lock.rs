//! One live orchestrator per run directory (friction #V6KDQM3S): the lock
//! rides an open file, so a second acquire fails while the first holder
//! lives and succeeds the moment it is gone -- including a holder that
//! died without cleaning up, since the kernel drops the lock with the fd.

use workflow::run::lock_run;

fn fresh_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wf-runlock-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_second_orchestrator_is_refused_until_the_first_lets_go() {
    let dir = fresh_dir("refuse");
    let first = lock_run(&dir).expect("an uncontended run dir locks");
    assert!(
        lock_run(&dir).is_none(),
        "two live orchestrators held the same run"
    );
    drop(first);
    assert!(
        lock_run(&dir).is_some(),
        "the lock outlived the holder that dropped it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_run_dir_means_no_lock_rather_than_a_panic() {
    let dir = fresh_dir("gone");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(lock_run(&dir).is_none());
}
