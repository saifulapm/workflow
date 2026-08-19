//! `~/.config/hub/config.toml` (spec §6) and the doorbell topic (spec §5).
//!
//! Every key is optional. The file is created on first run so the topic has
//! somewhere to live; after that it is read verbatim and never rewritten, which
//! is what keeps a hand-added comment or a key a later version introduces.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const DEFAULT_PORT: u16 = 8787;
pub const DEFAULT_NTFY_BASE: &str = "https://ntfy.sh";

/// Crockford base32, the alphabet ULIDs use: no I, L, O or U, so a topic read
/// off a screen and typed into a phone survives the trip.
const BASE32: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// Generated on first run; 128 bits from `/dev/urandom`, never a ULID.
    #[serde(default)]
    pub topic: String,
    /// Static links to the other machines' hubs (v1 has no merge-on-read).
    #[serde(default)]
    pub siblings: Vec<String>,
    /// So a self-hosted ntfy — or a test sink — replaces ntfy.sh without code.
    #[serde(default = "default_ntfy_base")]
    pub ntfy_base: String,
    /// Extra origins allowed to POST /answer (spec §9.2).
    #[serde(default)]
    pub origins: Vec<String>,
    /// The URL the doorbell tells the phone to open. Defaults to
    /// `http://<machine>:<port>/`.
    ///
    /// Not in §6's list: added because the machine-name file, `uname -n` and
    /// the MagicDNS name are three different strings on this machine (review
    /// m-10), so a derived click-URL can point at a name the phone cannot
    /// resolve. One optional key is cheaper than a doorbell that opens nothing.
    #[serde(default)]
    pub hub_url: Option<String>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_ntfy_base() -> String {
    DEFAULT_NTFY_BASE.to_string()
}

impl Default for Config {
    fn default() -> Config {
        Config {
            port: DEFAULT_PORT,
            topic: String::new(),
            siblings: Vec::new(),
            ntfy_base: default_ntfy_base(),
            origins: Vec::new(),
            hub_url: None,
        }
    }
}

impl Config {
    /// The full publish URL, with exactly one slash between base and topic.
    pub fn ntfy_url(&self) -> String {
        format!("{}/{}", self.ntfy_base.trim_end_matches('/'), self.topic)
    }

    /// The public subscribe URL for the topic, as `/subscribe` prints it.
    pub fn subscribe_url(&self) -> String {
        self.ntfy_url()
    }
}

/// The default location, honouring `XDG_CONFIG_HOME` so a test never has to go
/// near the real one.
pub fn default_path() -> Result<PathBuf> {
    Ok(config_home()?.join("hub/config.toml"))
}

pub fn config_home() -> Result<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => Ok(PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join(".config")),
    }
}

pub fn state_home() -> Result<PathBuf> {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => Ok(
            PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join(".local/state"),
        ),
    }
}

/// Reads `path`, creating it — and the topic inside it — when either is
/// missing. Returns the effective config.
pub fn load_or_create(path: &Path) -> Result<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };

    let mut config: Config = match &text {
        Some(text) => {
            toml::from_str(text).with_context(|| format!("parsing {}", path.display()))?
        }
        None => Config::default(),
    };

    if config.topic.is_empty() {
        config.topic = new_topic()?;
        let body = match text {
            // Append rather than re-serialise: an existing file may carry
            // comments, and a config we rewrote would lose them.
            Some(text) if !text.is_empty() => {
                let sep = if text.ends_with('\n') { "" } else { "\n" };
                format!("{text}{sep}topic = \"{}\"\n", config.topic)
            }
            _ => rendered(&config),
        };
        write_private(path, &body)?;
    }

    Ok(config)
}

fn rendered(config: &Config) -> String {
    format!(
        "# hub's config (spec §6). Every key is optional; the topic is the one\n\
         # secret here, which is why this file is 0600.\n\
         topic = \"{}\"\n\
         # port = {DEFAULT_PORT}\n\
         # ntfy_base = \"{DEFAULT_NTFY_BASE}\"\n\
         # siblings = [\"http://nuc:{DEFAULT_PORT}\"]\n\
         # origins = []\n\
         # hub_url = \"http://<machine>:{DEFAULT_PORT}/\"\n",
        config.topic
    )
}

/// Creates the file at 0600 from the moment it exists, rather than creating it
/// world-readable and chmod-ing afterwards.
fn write_private(path: &Path, body: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let temp = path.with_extension("toml.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .with_context(|| format!("creating {}", temp.display()))?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temp, path)
        .with_context(|| format!("renaming {} into place", temp.display()))?;
    Ok(())
}

/// `workflow-` plus 128 bits from `/dev/urandom`, Crockford base32.
///
/// Not a ULID: a ULID's first ten characters are a millisecond timestamp with
/// no entropy in them (review M-4), so it would publish the install time to a
/// service that logs topic names, and leave 80 bits rather than 128.
pub fn new_topic() -> Result<String> {
    let bytes = urandom::<16>()?;
    Ok(format!("workflow-{}", base32(&bytes)))
}

pub fn urandom<const N: usize>() -> Result<[u8; N]> {
    use std::io::Read;

    let mut bytes = [0u8; N];
    let mut file = std::fs::File::open("/dev/urandom").context("opening /dev/urandom")?;
    file.read_exact(&mut bytes)
        .context("reading /dev/urandom")?;
    Ok(bytes)
}

/// 16 bytes as 26 base32 characters, big-endian, exactly ULID's layout: the
/// first character carries the top 3 bits, the other 25 carry 5 each.
fn base32(bytes: &[u8; 16]) -> String {
    let value = u128::from_be_bytes(*bytes);
    let mut out = String::with_capacity(26);
    for i in 0..26 {
        let shift = 125 - i * 5;
        let index = ((value >> shift) & 0x1f) as usize;
        out.push(BASE32[index] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_covers_the_whole_alphabet_and_the_edges() {
        assert_eq!(base32(&[0u8; 16]), "0".repeat(26));
        // All bits set: the first character holds only 3 of them, so it is 7.
        assert_eq!(base32(&[0xffu8; 16]), format!("7{}", "Z".repeat(25)));
    }

    #[test]
    fn a_topic_is_the_shape_section_five_asks_for() {
        let topic = new_topic().unwrap();
        assert!(topic.starts_with("workflow-"));
        assert_eq!(topic.len(), "workflow-".len() + 26);
    }
}
