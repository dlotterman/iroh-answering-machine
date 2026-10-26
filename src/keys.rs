//! Secret keys as 64 hex characters, in the environment or in files that are made on first use.
use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result, anyhow};
use iroh::{EndpointId, SecretKey};

pub fn parse_secret_key(hex: &str) -> Result<SecretKey> {
    let bytes =
        data_encoding::HEXLOWER_PERMISSIVE.decode(hex.trim().as_bytes()).map_err(|_| anyhow!("the key is not hex"))?;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("the key is not 32 bytes (64 hex characters)"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

pub fn secret_key_hex(key: &SecretKey) -> String {
    data_encoding::HEXLOWER.encode(&key.to_bytes())
}

pub fn parse_endpoint_id(text: &str) -> Result<EndpointId> {
    text.trim().parse().map_err(|e| anyhow!("{:?} is not an endpoint id: {e}", text.trim()))
}

/// Read the key at `path`, which must exist.
pub fn load(path: &Path) -> Result<SecretKey> {
    parse_secret_key(&fs::read_to_string(path)?).with_context(|| format!("reading {}", path.display()))
}

/// Read the key at `path`, or make one there (mode 600) if there is none, so the identity survives restarts.
pub fn load_or_create(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        return load(path);
    }
    let key = SecretKey::generate();
    write_private(path, secret_key_hex(&key).as_bytes())?;
    Ok(key)
}

/// Write a file only its owner can read. Used for keys and saved invites, which are both secrets.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_file_is_made_once_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/owner.key");
        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();
        assert_eq!(first.public(), second.public());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn malformed_keys_are_rejected() {
        for bad in ["", "abc", &"zz".repeat(32), &"00".repeat(31)] {
            assert!(parse_secret_key(bad).is_err(), "{bad:?} should not parse");
        }
        let key = SecretKey::generate();
        assert_eq!(parse_secret_key(&format!("{}\n", secret_key_hex(&key))).unwrap().public(), key.public());
    }
}
