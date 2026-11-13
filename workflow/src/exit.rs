//! Exit codes. Every command's contract in one place, because the hooks and
//! the orchestrator read them.
//!
//!   lint-msg       0 clean (warnings included) · 1 hard fail
//!   hygiene        0 clean or warned · 1 hard finding · 2 usage
//!   install        0 installed · 1 something could not be written or removed
//!   go             0 started or the roadmap is done · 1 mem or amx failed · 2 refused
//!   pause, unpause 0 done · 1 mem failed · 2 no such project
//!   switch         0 switched · 1 a step failed · 2 refused
//!   land           0 landed or done · 1 failed · 2 parked · 3 CI still runs: run it again
//!   wait-answer    0 answered · 1 mem failed · 2 parked or no such question · 3 still open: run it again
//!   project-new    0 every check ok · 1 a check missing or mem failed · 2 no such project

pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
/// `workflow land` waited as long as one call may on a CI run that has not
/// finished: the thread runs it again.
pub const AGAIN: i32 = 3;

/// Put the default SIGPIPE handler back, before anything is printed.
///
/// Rust's runtime ignores SIGPIPE so a write to a closed pipe comes back as
/// EPIPE, and `println!` turns that into a panic: `workflow hygiene --tree | head`
/// ended in a stack trace and exit 101, the same wart mem had. A reader that
/// stops reading is the reader's business. End where it did, the way every
/// other command in a pipeline does.
///
/// None of the codes above apply to that ending -- it is a signal, not an exit
/// code -- which is the honest answer and the one `head` expects.
pub fn end_where_the_reader_did() {
    unsafe { signal(SIGPIPE, SIG_DFL) };
}

/// Runs `write` with SIGPIPE ignored, so a write to a child that has already
/// exited comes back as EPIPE instead of ending this command with no word:
/// `workflow switch` died that way when `hub share mint` failed before it
/// read its stdin.
pub fn with_sigpipe_ignored<T>(write: impl FnOnce() -> T) -> T {
    let before = unsafe { signal(SIGPIPE, SIG_IGN) };
    let out = write();
    unsafe { signal(SIGPIPE, before) };
    out
}

// SIGPIPE is 13, SIG_DFL 0 and SIG_IGN 1 on Linux and on macOS, the two
// platforms this is built for. Declared here rather than pulled in with libc:
// it is one call.
const SIGPIPE: i32 = 13;
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;

unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}
