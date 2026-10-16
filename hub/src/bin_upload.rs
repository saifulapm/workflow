//! `hub share put-bin <name> <file>`: a machine puts a build of `mem` or
//! the hub in the store, where a cloud session's setup fetches it from
//! `/store/dl/<name>`.

use anyhow::{Context, Result, bail};

use crate::share::Store;

/// PUTs the bytes of `local` to `<store>/store/dl/<name>` and returns how
/// many it stored.
pub fn run(store: &Store, name: &str, local: &str) -> Result<usize> {
    let bytes = std::fs::read(local).with_context(|| format!("reading {local}"))?;
    let mut request = store
        .agent
        .put(format!("{}/store/dl/{name}", store.base()?));
    for (header, value) in store.headers() {
        request = request.header(header, value);
    }
    let response = request.send(&bytes[..]).context("reaching the store")?;
    match response.status().as_u16() {
        200..=299 => Ok(bytes.len()),
        status => bail!("the store answered {status} to /store/dl/{name}"),
    }
}
