//! Signing and verification behind traits; the real backend is `ssh-keygen -Y`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use goway_journal::sha256_hex;
use tracing::{debug, warn};

use crate::error::CryptoError;

/// The `ssh-keygen -Y` namespace that binds signatures to roster operations.
// frob:invariant INV-CIRCLE-002
pub const NAMESPACE: &str = "goway-circle-v1";

/// Something that can sign a message as one public key.
pub trait Signer {
    /// The public key this signer signs as (the roster identity).
    fn public_key(&self) -> &str;
    /// Sign `message`, returning the signature as text.
    fn sign(&self, message: &[u8]) -> Result<String, CryptoError>;
}

/// Something that can check a signature against a public key.
pub trait Verifier {
    /// Return true only when `signature` is valid for `message` under `public_key`.
    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> bool;
}

/// Return true for a single-line `ssh-ed25519` (or FIDO `sk-`) key without a comment.
pub fn valid_public_key(key: &str) -> bool {
    let mut parts = key.split(' ');
    let (Some(kind), Some(blob), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    matches!(kind, "ssh-ed25519" | "sk-ssh-ed25519@openssh.com")
        && !blob.is_empty()
        && blob
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

/// Signs by shelling to `ssh-keygen -Y sign` with a private key file or FIDO handle.
#[derive(Debug, Clone)]
pub struct SshKeygenSigner {
    public_key: String,
    key_file: PathBuf,
}

impl SshKeygenSigner {
    /// Bind a private key file to its `type base64` public key; rejects non-ed25519 keys.
    pub fn new(key_file: PathBuf, public_key: String) -> Result<Self, CryptoError> {
        if !valid_public_key(&public_key) {
            return Err(CryptoError::BadKey);
        }
        Ok(Self {
            public_key,
            key_file,
        })
    }
}

impl SshKeygenSigner {
    /// The file handed to `ssh-keygen -f`: the private key, or its `.pub` when the key lives in
    /// ssh-agent or on a FIDO token (ssh-keygen then asks the agent to sign).
    fn signing_target(&self) -> PathBuf {
        if self.key_file.exists() {
            return self.key_file.clone();
        }
        let mut pubfile = self.key_file.as_os_str().to_owned();
        pubfile.push(".pub");
        let pubfile = PathBuf::from(pubfile);
        if pubfile.exists() {
            debug!(key = %pubfile.display(), "no private key file; signing through ssh-agent");
            return pubfile;
        }
        self.key_file.clone()
    }
}

impl Signer for SshKeygenSigner {
    fn public_key(&self) -> &str {
        &self.public_key
    }

    fn sign(&self, message: &[u8]) -> Result<String, CryptoError> {
        let mut cmd = Command::new("ssh-keygen");
        cmd.args(["-Y", "sign", "-n", NAMESPACE, "-f"])
            .arg(self.signing_target());
        let out = run(cmd, message)?;
        if !out.status.success() {
            warn!(status = ?out.status, "ssh-keygen sign failed");
            return Err(CryptoError::Tool(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ));
        }
        String::from_utf8(out.stdout).map_err(|_| CryptoError::Tool("non-utf8 signature".into()))
    }
}

/// Verifies by shelling to `ssh-keygen -Y verify` against a one-key allowed-signers file.
#[derive(Debug, Default, Clone, Copy)]
pub struct SshKeygenVerifier;

impl Verifier for SshKeygenVerifier {
    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> bool {
        if !valid_public_key(public_key) {
            debug!("verify refused: not an ed25519 key");
            return false;
        }
        let Ok(dir) = tempfile::tempdir() else {
            warn!("verify failed: no temporary directory");
            return false;
        };
        let allowed = dir.path().join("allowed_signers");
        let sig = dir.path().join("sig");
        let line = format!("goway-circle namespaces=\"{NAMESPACE}\" {public_key}\n");
        if std::fs::write(&allowed, line)
            .and_then(|()| std::fs::write(&sig, signature))
            .is_err()
        {
            warn!("verify failed: cannot stage files");
            return false;
        }
        let mut cmd = Command::new("ssh-keygen");
        cmd.args(["-Y", "verify", "-I", "goway-circle", "-n", NAMESPACE, "-f"])
            .arg(&allowed)
            .arg("-s")
            .arg(&sig);
        run(cmd, message).is_ok_and(|o| o.status.success())
    }
}

/// Run `cmd` with `input` on stdin and collect its output.
fn run(mut cmd: Command, input: &[u8]) -> Result<std::process::Output, CryptoError> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CryptoError::Tool(format!("cannot run ssh-keygen: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    Ok(child.wait_with_output()?)
}

/// Deterministic, NON-cryptographic signer for tests: signature = sha256(message + key).
#[derive(Debug, Clone)]
pub struct FakeCrypto(pub String);

impl FakeCrypto {
    /// A fake signer whose public key is `name`.
    pub fn new(name: &str) -> Self {
        Self(name.to_owned())
    }
}

fn fake_sig(message: &[u8], key: &str) -> String {
    let mut bytes = message.to_vec();
    bytes.extend_from_slice(key.as_bytes());
    sha256_hex(&bytes)
}

impl Signer for FakeCrypto {
    fn public_key(&self) -> &str {
        &self.0
    }

    fn sign(&self, message: &[u8]) -> Result<String, CryptoError> {
        Ok(fake_sig(message, &self.0))
    }
}

impl Verifier for FakeCrypto {
    fn verify(&self, public_key: &str, message: &[u8], signature: &str) -> bool {
        fake_sig(message, public_key) == signature
    }
}

#[cfg(test)]
mod signer_tests {
    use super::*;

    // frob:tests crates/goway-circle/src/crypto.rs::SshKeygenSigner
    #[test]
    fn a_missing_private_file_falls_back_to_the_pub_for_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("k");
        std::fs::write(dir.path().join("k.pub"), "ssh-ed25519 AAAA\n").unwrap();
        let s = SshKeygenSigner::new(key.clone(), "ssh-ed25519 AAAA".into()).unwrap();
        assert_eq!(s.signing_target(), dir.path().join("k.pub"));
        std::fs::write(&key, "x").unwrap();
        assert_eq!(s.signing_target(), key);
    }
}
