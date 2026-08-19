//! Exit codes (spec §7). Every command's contract in one place, because the
//! hooks, the adapters and the orchestrator all read them.
//!
//!   verify         0 green · 1 failed · 2 no verifier · 3 test removal
//!   lint-msg       0 clean (warnings included) · 1 hard fail
//!   review-needed  0 a cold review is wanted · 1 it is not
//!   run            0 complete · 1 parked or failed tasks · 2 config/plan error
//!   reap           0 nothing to do · 1 reaped something
//!   doctor         0 healthy · 1 findings

pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
pub const TEST_REMOVAL: i32 = 3;

/// No verifier in this repo. The same number as a usage error on purpose: the
/// bash contract fixed it at 2 and the hooks read it.
pub const NO_VERIFIER: i32 = 2;
