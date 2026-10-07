//! Where things live, and the one path question the hooks keep asking: what is
//! this really.

use std::path::{Path, PathBuf};

/// `$HOME`, which every path the installer writes is made from, so a test can
/// point it at a scratch directory. Unset is no answer, never `/`.
pub fn home() -> Option<PathBuf> {
    match std::env::var("HOME") {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => None,
    }
}

/// `realpath`: the real thing, or nothing when part of it is missing.
pub fn realpath(p: impl AsRef<Path>) -> Option<PathBuf> {
    std::fs::canonicalize(p.as_ref()).ok()
}
