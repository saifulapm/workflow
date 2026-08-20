//! Running a child with a ceiling on how long it may take (review M-3).
//!
//! `Command::output()` waits for ever. Everything hub knows it learned by
//! running `mem`, and those runs are serialised behind one gate, so a single
//! child that does not return takes every data route with it — the page, all
//! three API routes and the answer path alike, including the ones whose own
//! verb would have answered instantly. `Restart=always` never fires either,
//! because nothing has exited.
//!
//! There is no `wait_timeout` in `std` and §8's ruling adds no crates, so this
//! is: spawn, drain both pipes on their own threads, and poll `try_wait`
//! against a deadline.
//!
//! A child that overruns is killed and **reaped somewhere else**. Waiting for a
//! process to finish dying is the unbounded wait this module exists to remove,
//! and the caller is holding a lock while it waits.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often `try_wait` is asked. A `mem` read costs about ten milliseconds, so
/// this is a handful of wakeups in the normal case and a few thousand in the
/// worst one — cheaper than a thread per call to do the same job.
const POLL: Duration = Duration::from_millis(2);

/// A child that ran to completion.
pub struct Finished {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub enum Ended {
    /// It exited on its own, in time.
    Exited(Finished),
    /// It did not, and has been killed. Whatever it had written is dropped: a
    /// half-written document is not a document, and handing one to a parser is
    /// how "mem is slow" turns into "mem is broken" for the wrong reason.
    TimedOut,
    /// It never started — no such binary, or no room to fork.
    Failed(String),
}

/// ETXTBSY is transient by nature: `cargo install` replacing the mem binary
/// while this service is live, or — in the test suites — a sibling thread's
/// fork holding a write fd on a freshly written fixture across the exec
/// window. Both writers are gone within moments, so a bounded retry turns a
/// spurious "mem is broken" into one short stutter. Anything else fails as it
/// always did, on the first attempt.
fn spawn_retrying_busy(command: &mut Command) -> std::io::Result<std::process::Child> {
    const ETXTBSY: i32 = 26;
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

/// Runs `command` to completion, or kills it once `timeout` has passed.
pub fn output_within(command: &mut Command, timeout: Duration) -> Ended {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match spawn_retrying_busy(command) {
        Ok(child) => child,
        Err(e) => return Ended::Failed(e.to_string()),
    };

    // Drained on their own threads, not after the wait. A child that fills a
    // pipe buffer blocks on the write and never exits, so a reader that runs
    // only once the child is gone would make the deadline do a read's job.
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ended::Exited(Finished {
                    code: status.code(),
                    stdout: out.join().unwrap_or_default(),
                    stderr: err.join().unwrap_or_default(),
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                // Reaped on a thread of its own: see the module note. Dropping
                // the two reader handles detaches them, and they end when the
                // pipes close, which the kill guarantees.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ended::TimedOut;
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => {
                let _ = child.kill();
                return Ended::Failed(e.to_string());
            }
        }
    }
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_that_finishes_in_time_gives_back_everything_it_wrote() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf out; printf err >&2; exit 3"]);
        let Ended::Exited(done) = output_within(&mut command, Duration::from_secs(5)) else {
            panic!("a child that exits immediately timed out");
        };
        assert_eq!(done.code, Some(3));
        assert_eq!(done.stdout, b"out");
        assert_eq!(done.stderr, b"err");
    }

    #[test]
    fn a_child_that_overruns_is_killed_and_the_call_returns() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let started = Instant::now();
        assert!(matches!(
            output_within(&mut command, Duration::from_millis(150)),
            Ended::TimedOut
        ));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the call outlived the timeout it was given"
        );
    }

    #[test]
    fn a_child_writing_more_than_a_pipe_holds_is_not_a_deadlock() {
        // 256 KiB is four times the usual pipe buffer. Without the draining
        // threads the child blocks on its own write and this times out.
        let mut command = Command::new("sh");
        command.args(["-c", "yes hub | head -c 262144"]);
        let Ended::Exited(done) = output_within(&mut command, Duration::from_secs(10)) else {
            panic!("a child that filled the pipe buffer was killed");
        };
        assert_eq!(done.stdout.len(), 262_144);
    }

    #[test]
    fn a_binary_that_does_not_exist_never_started() {
        let mut command = Command::new("hub-no-such-binary-anywhere");
        assert!(matches!(
            output_within(&mut command, Duration::from_secs(5)),
            Ended::Failed(_)
        ));
    }
}
