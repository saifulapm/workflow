//! Exit codes (spec §7). Every one of them is part of the contract with the
//! adapters and the orchestrator, so they live in one place and are exercised
//! by tests.

use std::fmt;

pub const OK: i32 = 0;
/// id/query/argument resolution failed on any verb.
pub const NOT_FOUND: i32 = 1;
pub const USAGE: i32 = 2;
/// Store error on writes, or doctor/sync unable to run.
pub const STORE: i32 = 3;
pub const WAIT_TIMEOUT: i32 = 4;
pub const CAS_CONFLICT: i32 = 5;
/// Write accepted and on disk, but over budget.
pub const OVER_BUDGET: i32 = 6;
pub const AMBIGUOUS: i32 = 7;

/// An error that knows which exit code it must produce.
#[derive(Debug)]
pub struct Coded {
    pub code: i32,
    pub message: String,
}

impl fmt::Display for Coded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Coded {}

pub fn coded(code: i32, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Coded {
        code,
        message: message.into(),
    })
}

pub fn not_found(message: impl Into<String>) -> anyhow::Error {
    coded(NOT_FOUND, message)
}

pub fn usage(message: impl Into<String>) -> anyhow::Error {
    coded(USAGE, message)
}

pub fn store_error(message: impl Into<String>) -> anyhow::Error {
    coded(STORE, message)
}

/// An unclassified failure is a store error: everything mem does that can fail
/// unexpectedly is a filesystem or index operation.
pub fn code_of(err: &anyhow::Error) -> i32 {
    err.downcast_ref::<Coded>().map_or(STORE, |c| c.code)
}

/// Put the default SIGPIPE handler back, before anything is printed.
///
/// Rust's runtime ignores SIGPIPE so a write to a closed pipe comes back as
/// EPIPE, and `println!` turns that into a panic: `mem log | head -4` ended in
/// a stack trace and exit 101 (friction #ECTJYVXX). A reader that stops
/// reading is the reader's business. End where it did, the way every other
/// command in a pipeline does.
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
