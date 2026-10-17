//! Node and admin key files: generation with `ssh-keygen`, reading, fingerprints.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use sha2::{Digest as _, Sha256};
use tracing::{info, warn};

use crate::crypto::valid_public_key;
use crate::error::{CryptoError, StoreError};
use crate::passphrase::{Passphrase, keygen_encrypted};

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

/// Refuse a key file that is a symlink, not a regular file, or readable by group/other (Unix).
///
/// Uses `symlink_metadata` so a swapped-in link is seen, not followed.
#[cfg(unix)]
// frob:invariant INV-CIRCLE-001
fn check_private_file(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    let unsafe_key = |reason: &str| StoreError::KeyUnsafe {
        path: path.to_owned(),
        reason: reason.to_owned(),
    };
    let meta = std::fs::symlink_metadata(path).map_err(|e| io_err("stat", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(unsafe_key("it is a symlink"));
    }
    if !meta.is_file() {
        return Err(unsafe_key("it is not a regular file"));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        warn!(key = %path.display(), mode = format!("{mode:o}"), "key file is too open");
        return Err(unsafe_key(&format!(
            "mode {mode:o} lets others read it; run chmod 600"
        )));
    }
    Ok(())
}

/// Refuse a key directory that group or other can write into (Unix).
#[cfg(unix)]
fn check_key_dir(dir: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;
    // Follow a symlinked directory so the real target's mode is checked.
    let meta = std::fs::metadata(dir).map_err(|e| io_err("stat", dir, e))?;
    if meta.permissions().mode() & 0o022 != 0 {
        return Err(StoreError::KeyUnsafe {
            path: dir.to_owned(),
            reason: "the key directory is group or world writable".to_owned(),
        });
    }
    Ok(())
}

/// Load the ed25519 key pair at `private`, making it (mode 0600, no passphrase) when absent.
///
/// An existing private key must be a plain 0600 file or it is refused (Unix). A `.pub` with no
/// private file is an agent or hardware identity and is used as is; a private file with no
/// `.pub` is an error and is never regenerated. `restrict` tightens a fresh key's ACL.
/// Returns the pair and whether it was just created.
pub fn ensure_keypair(
    private: &Path,
    restrict: &dyn Fn(&Path),
) -> Result<(KeyPair, bool), StoreError> {
    ensure_keypair_with(private, restrict, None)
}

/// Like [`ensure_keypair`], but a fresh key is encrypted with `pass` when one is given.
///
/// An existing key is loaded as is (encrypted or not); the passphrase is only used to make it.
pub fn ensure_keypair_with(
    private: &Path,
    restrict: &dyn Fn(&Path),
    pass: Option<&Passphrase>,
) -> Result<(KeyPair, bool), StoreError> {
    let public_file = pub_path(private);
    let has_private = std::fs::symlink_metadata(private).is_ok();
    let has_public = public_file.exists();
    #[cfg(unix)]
    if let Some(dir) = private.parent().filter(|d| d.exists()) {
        check_key_dir(dir)?;
    }
    match (has_private, has_public) {
        (true, true) => {
            #[cfg(unix)]
            check_private_file(private)?;
            let public = read_public_key(&public_file)?;
            return Ok((
                KeyPair {
                    private: private.to_owned(),
                    public,
                },
                false,
            ));
        }
        (false, true) => {
            let public = read_public_key(&public_file)?;
            info!(key = %public_file.display(), "using a public-only (agent or hardware) key");
            return Ok((
                KeyPair {
                    private: private.to_owned(),
                    public,
                },
                false,
            ));
        }
        (true, false) => {
            warn!(key = %private.display(), "private key has no .pub; refusing to regenerate");
            return Err(StoreError::KeyIncomplete {
                path: private.to_owned(),
            });
        }
        (false, false) => {}
    }
    if let Some(dir) = private.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    if let Some(pass) = pass {
        keygen_encrypted(private, pass)?;
    } else {
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

/// Generate an `ed25519-sk` (FIDO2) admin key at `private` by running `program` (`ssh-keygen`).
///
/// The token is touched (and its PIN asked) on the caller's terminal, so stdio is inherited;
/// `program` is a parameter so tests can substitute a fake. Refuses to overwrite an existing key.
// frob:ticket 01M5493X9E88JC1ECXBSKGF7C1
pub fn generate_sk_keypair(
    program: &Path,
    private: &Path,
    restrict: &dyn Fn(&Path),
) -> Result<KeyPair, StoreError> {
    if std::fs::symlink_metadata(private).is_ok() || pub_path(private).exists() {
        warn!(key = %private.display(), "refusing to overwrite an existing key with a hardware key");
        return Err(StoreError::KeyUnsafe {
            path: private.to_owned(),
            reason: "a key already exists there".to_owned(),
        });
    }
    if let Some(dir) = private.parent() {
        #[cfg(unix)]
        if dir.exists() {
            check_key_dir(dir)?;
        }
        std::fs::create_dir_all(dir).map_err(|e| io_err("create", dir, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    info!(key = %private.display(), "generating an ed25519-sk admin key; touch the token when it blinks");
    let status = Command::new(program)
        .args(["-t", "ed25519-sk", "-N", "", "-C", "", "-f"])
        .arg(private)
        .status()
        .map_err(|e| CryptoError::Tool(format!("cannot run ssh-keygen: {e}")))?;
    if !status.success() {
        warn!(?status, "ssh-keygen could not make an ed25519-sk key");
        return Err(CryptoError::Tool(format!("ssh-keygen exited with {status}")).into());
    }
    restrict(private);
    let public = read_public_key(&pub_path(private))?;
    info!(key = %private.display(), "generated an ed25519-sk admin key");
    Ok(KeyPair {
        private: private.to_owned(),
        public,
    })
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
        let (before, made) = ensure_keypair(&file, &|_| {}).unwrap();
        assert!(made);
        assert!(valid_public_key(&before.public));
        let (after, made) = ensure_keypair(&file, &|_| {}).unwrap();
        assert!(!made);
        assert_eq!(before.public, after.public);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let perms = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(
                perms & 0o077,
                0,
                "private key must not be group/world readable"
            );
        }
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair_with
    #[test]
    fn encrypted_key_needs_the_passphrase_to_sign() {
        use crate::crypto::{Signer as _, SshKeygenSigner};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("k");
        let pass = Passphrase::new("correct horse battery".into()).unwrap();
        let (pair, made) = ensure_keypair_with(&file, &|_| {}, Some(&pass)).unwrap();
        assert!(made);
        assert!(crate::passphrase::key_is_encrypted(&file));
        let wrong = Passphrase::new("wrong wrong wrong".into()).unwrap();
        let bad = SshKeygenSigner::new(file.clone(), pair.public.clone())
            .unwrap()
            .with_passphrase(wrong);
        assert!(bad.sign(b"m").is_err());
        let good = SshKeygenSigner::new(file, pair.public)
            .unwrap()
            .with_passphrase(pass);
        assert!(good.sign(b"m").unwrap().contains("BEGIN SSH SIGNATURE"));
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair_with
    #[test]
    fn plain_key_is_not_reported_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let file = make_key(dir.path());
        assert!(!crate::passphrase::key_is_encrypted(&file));
    }

    // frob:tests crates/goway-circle/src/passphrase.rs::Passphrase
    #[test]
    fn passphrase_debug_is_redacted_and_empty_refused() {
        let p = Passphrase::new("s3cret-value".into()).unwrap();
        assert!(!format!("{p:?}").contains("s3cret"));
        assert!(Passphrase::new(String::new()).is_err());
    }

    fn make_key(dir: &Path) -> PathBuf {
        let file = dir.join("k");
        ensure_keypair(&file, &|_| {}).unwrap();
        file
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair
    #[cfg(unix)]
    #[test]
    fn loose_key_mode_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let file = make_key(dir.path());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = ensure_keypair(&file, &|_| {}).unwrap_err();
        assert!(matches!(err, StoreError::KeyUnsafe { .. }), "{err}");
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair
    #[cfg(unix)]
    #[test]
    fn symlinked_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = make_key(dir.path());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        std::os::unix::fs::symlink(pub_path(&real), pub_path(&link)).unwrap();
        let err = ensure_keypair(&link, &|_| {}).unwrap_err();
        assert!(matches!(err, StoreError::KeyUnsafe { .. }), "{err}");
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair
    #[cfg(unix)]
    #[test]
    fn writable_key_dir_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let file = make_key(dir.path());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = ensure_keypair(&file, &|_| {}).unwrap_err();
        assert!(matches!(err, StoreError::KeyUnsafe { .. }), "{err}");
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair
    #[test]
    fn private_without_pub_is_never_regenerated() {
        let dir = tempfile::tempdir().unwrap();
        let file = make_key(dir.path());
        let before = std::fs::read(&file).unwrap();
        std::fs::remove_file(pub_path(&file)).unwrap();
        let err = ensure_keypair(&file, &|_| {}).unwrap_err();
        assert!(matches!(err, StoreError::KeyIncomplete { .. }), "{err}");
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair
    #[test]
    fn public_only_identity_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let file = make_key(dir.path());
        let (kept, _) = ensure_keypair(&file, &|_| {}).unwrap();
        std::fs::remove_file(&file).unwrap();
        let (again, made) = ensure_keypair(&file, &|_| {}).unwrap();
        assert!(!made);
        assert_eq!(kept.public, again.public);
        assert!(!file.exists(), "no private key may be invented");
    }

    /// Write an executable fake `ssh-keygen` that records its args and makes a key pair.
    #[cfg(unix)]
    fn fake_keygen(dir: &Path, succeed: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let script = dir.join("fake-ssh-keygen");
        let body = if succeed {
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in -f) f=\"$2\";; esac; echo \"$1\" >> \"$0.args\"; shift; done\n\
             echo secret > \"$f\"; echo 'sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29t comment' > \"$f.pub\"\n"
        } else {
            "#!/bin/sh\nexit 1\n"
        };
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    // frob:tests crates/goway-circle/src/keys.rs::generate_sk_keypair
    #[cfg(unix)]
    #[test]
    fn sk_keygen_runs_the_tool_and_returns_the_pair() {
        let dir = tempfile::tempdir().unwrap();
        let tool = fake_keygen(dir.path(), true);
        let key = dir.path().join("circle").join("admin_ed25519");
        let pair = generate_sk_keypair(&tool, &key, &|_| {}).unwrap();
        assert!(pair.public.starts_with("sk-ssh-ed25519@openssh.com "));
        assert!(!pair.public.contains("comment"));
        let args = std::fs::read_to_string(dir.path().join("fake-ssh-keygen.args")).unwrap();
        assert!(args.lines().any(|l| l == "ed25519-sk"));
    }

    // frob:tests crates/goway-circle/src/keys.rs::generate_sk_keypair
    #[cfg(unix)]
    #[test]
    fn sk_keygen_failure_and_existing_key_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bad = fake_keygen(dir.path(), false);
        let key = dir.path().join("admin");
        assert!(generate_sk_keypair(&bad, &key, &|_| {}).is_err());
        let good = fake_keygen(dir.path(), true);
        generate_sk_keypair(&good, &key, &|_| {}).unwrap();
        assert!(generate_sk_keypair(&good, &key, &|_| {}).is_err());
    }

    // frob:tests crates/goway-circle/src/keys.rs::ensure_keypair
    #[cfg(unix)]
    #[test]
    fn symlinked_writable_key_dir_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o777)).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = ensure_keypair(&link.join("k"), &|_| {}).unwrap_err();
        assert!(matches!(err, StoreError::KeyUnsafe { .. }), "{err}");
    }
}
