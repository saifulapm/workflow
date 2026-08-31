//! Exit codes (spec §7). Every command's contract in one place, because the
//! hooks, the adapters and the orchestrator all read them.
//!
//!   verify         0 green · 1 failed · 2 no verifier · 3 test removal
//!   lint-msg       0 clean (warnings included) · 1 hard fail
//!   review-needed  0 a cold review is wanted · 1 it is not
//!   run            0 complete · 1 failed tasks · 2 config/plan error
//!   reap           0 nothing to do · 1 reaped something
//!   doctor         0 healthy · 1 findings

pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
pub const TEST_REMOVAL: i32 = 3;

/// No verifier in this repo. The same number as a usage error on purpose: the
/// bash contract fixed it at 2 and the hooks read it.
pub const NO_VERIFIER: i32 = 2;

/// Put the default SIGPIPE handler back, before anything is printed.
///
/// Rust's runtime ignores SIGPIPE so a write to a closed pipe comes back as
/// EPIPE, and `println!` turns that into a panic: `workflow status | head`
/// ended in a stack trace and exit 101, the same wart mem had (friction
/// #ECTJYVXX). A reader that stops reading is the reader's business. End where
/// it did, the way every other command in a pipeline does.
///
/// None of the codes above apply to that ending -- it is a signal, not an exit
/// code -- which is the honest answer and the one `head` expects.
pub fn end_where_the_reader_did() {
    // SIGPIPE is 13 and SIG_DFL is 0 on Linux and on macOS, the two platforms
    // this is built for. Declared here rather than pulled in with libc: it is
    // one call, made once, before the first write.
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    unsafe { signal(13, 0) };
}
