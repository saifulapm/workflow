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
