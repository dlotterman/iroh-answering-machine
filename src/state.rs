//! The local state directory (a podman volume mounted at `/state` in the container). Layout:
//!
//! - `owner.key`, `owner.json`: the owner identity, their name, and the answering machine they own.
//! - `messages.jsonl`: every message the owner has fetched. The server forgets messages once acked; this does not.
//! - `issued.jsonl`: invites this owner has made (ids, labels, expiry; not the invites themselves).
//! - `sender.key`: the identity used to leave messages elsewhere, kept apart from the owner key.
//! - `invites/<name>`: invites to other people's answering machines, saved by name.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{invite, keys};

/// Saved invite names are file names, so they stay short and plain.
const MAX_INVITE_NAME_CHARS: usize = 64;

pub struct State {
    pub dir: PathBuf,
}

/// `owner.json`. Everything the manifest needs besides keys lives here, so re-applying it is always complete.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnerConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Endpoint ids of other owners (another machine or agent of the same human).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_owners: Vec<String>,
    /// Invite ids the server must refuse before they expire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked_invites: Vec<String>,
}

impl State {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn has_owner(&self) -> bool {
        self.path("owner.key").exists() && self.path("owner.json").exists()
    }

    fn require_owner(&self) -> Result<()> {
        if !self.has_owner() {
            bail!(
                "there is no owner identity in {}; run `am owner init --name <your human's name>` first",
                self.dir.display()
            );
        }
        Ok(())
    }

    pub fn owner_key(&self) -> Result<iroh::SecretKey> {
        self.require_owner()?;
        keys::load(&self.path("owner.key"))
    }

    pub fn owner_config(&self) -> Result<OwnerConfig> {
        self.require_owner()?;
        let path = self.path("owner.json");
        serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("reading {}", path.display()))
    }

    pub fn save_owner_config(&self, config: &OwnerConfig) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.path("owner.json"), serde_json::to_vec_pretty(config)?)?;
        Ok(())
    }

    pub fn sender_key(&self) -> Result<iroh::SecretKey> {
        keys::load_or_create(&self.path("sender.key"))
    }

    pub fn invites_dir(&self) -> PathBuf {
        self.dir.join("invites")
    }

    pub fn invite_path(&self, name: &str) -> Result<PathBuf> {
        let valid = !name.is_empty()
            && name.chars().count() <= MAX_INVITE_NAME_CHARS
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            && !name.starts_with('.');
        if !valid {
            bail!("{name:?} is not a usable invite name; use letters, digits, '-', '_' and '.', like `human-a`");
        }
        Ok(self.invites_dir().join(name))
    }

    /// An invite given literally (`am1.…`) or by the name it was saved under.
    pub fn resolve_invite(&self, name_or_invite: &str) -> Result<String> {
        let trimmed = name_or_invite.trim();
        if trimmed.starts_with(invite::PREFIX) {
            return Ok(trimmed.to_string());
        }
        let path = self.invite_path(trimmed)?;
        fs::read_to_string(&path).map(|s| s.trim().to_string()).map_err(|_| {
            anyhow!(
                "there is no saved invite named {trimmed:?}; save one with `am invites save {trimmed}` (the invite on stdin)"
            )
        })
    }
}

pub fn append_jsonl<T: Serialize>(path: &Path, items: &[T]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut buf = Vec::new();
    for item in items {
        serde_json::to_writer(&mut buf, item)?;
        buf.push(b'\n');
    }
    file.write_all(&buf)?;
    file.sync_all()?;
    Ok(())
}

pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(i, line)| serde_json::from_str(line).with_context(|| format!("{} line {}", path.display(), i + 1)))
        .collect()
}
