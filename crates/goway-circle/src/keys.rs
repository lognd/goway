//! Node and admin key files: generation with `ssh-keygen`, reading, fingerprints.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::{Digest as _, Sha256};
use tracing::{info, warn};

use crate::crypto::valid_public_key;
use crate::error::{CryptoError, StoreError};

/// A private key file and its public half (the roster identity).
#[derive(Debug, Clone)]
pub struct KeyPair {
    /// The private key file; its contents are never read or logged here.
    pub private: PathBuf,
    /// The bare `ssh-ed25519 AAAA...` public key.
    pub public: String,
}

fn io_err(op: &'static str, path: &Path, source: std::io::Error) -> StoreError {
    StoreError::Io {
        op,
        path: path.to_owned(),
        source,
    }
}

/// The public key path that goes with a private key path (`<path>.pub`).
fn pub_path(private: &Path) -> PathBuf {
    let mut name = private.as_os_str().to_owned();
    name.push(".pub");
    PathBuf::from(name)
}

/// Read a `.pub` file and reduce it to the bare `type base64` form (no comment).
pub fn read_public_key(path: &Path) -> Result<String, StoreError> {
    let text = std::fs::read_to_string(path).map_err(|e| io_err("read", path, e))?;
    let mut fields = text.split_whitespace();
    let bare = match (fields.next(), fields.next()) {
        (Some(kind), Some(blob)) => format!("{kind} {blob}"),
        _ => return Err(CryptoError::BadKey.into()),
    };
    if valid_public_key(&bare) {
        Ok(bare)
    } else {
        Err(CryptoError::BadKey.into())
    }
}

/// The OpenSSH-style `SHA256:` fingerprint of a bare public key; `None` if it is not decodable.
pub fn key_fingerprint(key: &str) -> Option<String> {
    let blob = STANDARD.decode(key.split(' ').nth(1)?).ok()?;
    Some(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(blob))
    ))
}

/// Load the ed25519 key pair at `private`, making it (mode 0600, no passphrase) when absent.
///
/// `restrict` is called on a freshly made private key so a platform can tighten its ACL.
/// Returns the pair and whether it was just created.
pub fn ensure_keypair(
    private: &Path,
    restrict: &dyn Fn(&Path),
) -> Result<(KeyPair, bool), StoreError> {
    let public_file = pub_path(private);
    if private.exists() && public_file.exists() {
        let public = read_public_key(&public_file)?;
        return Ok((
            KeyPair {
                private: private.to_owned(),
                public,
            },
            false,
        ));
    }
    if let Some(dir) = private.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    // A private key without its public half is regenerated from scratch.
    for stale in [private, public_file.as_path()] {
        let _ = std::fs::remove_file(stale);
    }
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "", "-f"])
        .arg(private)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CryptoError::Tool(format!("cannot run ssh-keygen: {e}")))?;
    if !out.status.success() {
        warn!(status = ?out.status, "ssh-keygen could not make a key");
        return Err(
            CryptoError::Tool(String::from_utf8_lossy(&out.stderr).trim().to_owned()).into(),
        );
    }
    restrict(private);
    let public = read_public_key(&public_file)?;
    info!(key = %private.display(), "generated a circle key");
    Ok((
        KeyPair {
            private: private.to_owned(),
            public,
        },
        true,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_rejects_junk() {
        let key =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";
        let fp = key_fingerprint(key).unwrap();
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(key_fingerprint(key).unwrap(), fp);
        assert_eq!(key_fingerprint("ssh-ed25519 !!!"), None);
    }

    #[test]
    fn keypair_is_made_once_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("circle").join("node_ed25519");
        let (a, made) = ensure_keypair(&file, &|_| {}).unwrap();
        assert!(made);
        assert!(valid_public_key(&a.public));
        let (b, made) = ensure_keypair(&file, &|_| {}).unwrap();
        assert!(!made);
        assert_eq!(a.public, b.public);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o077,
                0,
                "private key must not be group/world readable"
            );
        }
    }
}
