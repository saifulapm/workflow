//! `hub share mint <runner> <machine>`: the holder of the shared store
//! bearer asks the store for a machine's own token. The store keeps only its
//! hash, ties that machine's reports and dones to the runner name, and its
//! writes to the mem machine name.

use std::io::Read;

use anyhow::{Context, Result, bail};

/// Reads the shared bearer from stdin, so it stays out of `ps` and the
/// shell history, and prints the token the store minted for `runner`
/// writing as `machine`.
pub fn run(runner: &str, machine: &str) -> Result<()> {
    let url = std::env::var("HUB_STORE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .context("hub share mint needs HUB_STORE_URL")?;
    let mut bearer = String::new();
    std::io::stdin()
        .read_to_string(&mut bearer)
        .context("reading the shared store bearer from stdin")?;
    let bearer = bearer.trim();
    if bearer.is_empty() {
        bail!("hub share mint reads the shared store bearer from stdin");
    }
    println!("{}", mint(&url, bearer, runner, machine)?);
    Ok(())
}

/// POSTs to `<store>/store/machines/<runner>/<machine>` with the shared
/// bearer, and the Access service token when the environment has one, as
/// the hub's other store requests do.
fn mint(url: &str, bearer: &str, runner: &str, machine: &str) -> Result<String> {
    // HUB_STORE_URL names the socket, `ws://h:p/store/share`; the store's
    // web address is its scheme and host.
    let (scheme, rest) = match url.split_once("://") {
        Some(("wss", rest)) => ("https", rest),
        Some(("ws", rest)) => ("http", rest),
        _ => bail!("HUB_STORE_URL is neither ws:// nor wss://"),
    };
    let host = rest.split('/').next().unwrap_or(rest);
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent
        .post(format!(
            "{scheme}://{host}/store/machines/{runner}/{machine}"
        ))
        .header("Authorization", format!("Bearer {bearer}"));
    let read = |name| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    // The token lives in the store object the machine will share with, so
    // it is minted there, as every later request of the machine names it.
    if let Some(object) = read("HUB_STORE_NAME") {
        request = request.header("X-Hub-Store", object.trim());
    }
    if let (Some(id), Some(secret)) = (read("HUB_STORE_ACCESS_ID"), read("HUB_STORE_ACCESS_SECRET"))
    {
        request = request
            .header("CF-Access-Client-Id", id.trim())
            .header("CF-Access-Client-Secret", secret.trim());
    }
    let mut answer = request.send_empty().context("reaching the store")?;
    let status = answer.status().as_u16();
    let token = answer.body_mut().read_to_string().unwrap_or_default();
    if status != 200 {
        bail!("the store answered {status} to /store/machines/{runner}/{machine}");
    }
    Ok(token.trim().to_string())
}
