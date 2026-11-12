//! `Scratch::script`, the fake binaries every other test runs. A test process
//! forks from many threads at once, and a child forked while this process
//! holds a fake open for writing keeps that handle until it execs, so the
//! fake's own exec fails with "Text file busy".

#[path = "../src/scratch.rs"]
mod scratch;

use std::process::Command;

use scratch::Scratch;

#[test]
fn a_fake_written_while_other_threads_fork_still_runs() {
    let threads: Vec<_> = (0..8)
        .map(|n| {
            std::thread::spawn(move || {
                let w = Scratch::new("script-race");
                for i in 0..60 {
                    let fake = w.script(&format!("fake-{n}-{i}"), "echo ran");
                    let out = Command::new(&fake).output().unwrap_or_else(|e| {
                        panic!("{} would not run: {e}", fake.display());
                    });
                    assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}
