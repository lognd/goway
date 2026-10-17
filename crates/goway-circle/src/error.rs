//! Typed errors for the circle crate.

/// A failure of the signing or verification backend.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// The public key is not a single-line ed25519 key.
    #[error("not an ed25519 public key")]
    BadKey,
    /// `ssh-keygen` could not run or refused to sign.
    #[error("ssh-keygen failed: {0}")]
    Tool(String),
    /// A temporary file for `ssh-keygen` could not be written.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// A failure of an OS keystore holding key passphrases.
#[derive(Debug, thiserror::Error)]
pub enum KeystoreError {
    /// No keystore tool is installed or usable on this machine.
    #[error("no usable OS keystore: {0}")]
    Unavailable(String),
    /// The keystore ran but refused the request.
    #[error("keystore refused: {0}")]
    Refused(String),
    /// The stored secret is not usable as a passphrase.
    #[error(transparent)]
    Secret(#[from] CryptoError),
}

/// A failure to build, parse or fold roster operations.
#[derive(Debug, thiserror::Error)]
pub enum CircleError {
    /// More operations than [`crate::MAX_OPS`] were offered.
    #[error("too many operations: {count} (max {max})")]
    TooManyOps {
        /// Number offered.
        count: usize,
        /// The cap.
        max: usize,
    },
    /// One operation exceeds [`crate::MAX_OP_BYTES`].
    #[error("operation of {bytes} bytes exceeds the cap of {max}")]
    OpTooLarge {
        /// Encoded size.
        bytes: usize,
        /// The cap.
        max: usize,
    },
    /// An operation line is not valid strict JSON for an operation.
    #[error("malformed operation on line {line}: {source}")]
    Parse {
        /// 1-based line number.
        line: usize,
        /// The underlying parse failure.
        source: serde_json::Error,
    },
    /// A stated operation id does not match the content hash.
    #[error("operation id mismatch on line {line}")]
    IdMismatch {
        /// 1-based line number.
        line: usize,
    },
    /// Signing an operation failed.
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// An offered roster lacks heads this node has already seen.
    #[error("rollback refused: offered roster lacks {} known head(s)", missing.len())]
    Rollback {
        /// The known head ids absent from the offer.
        missing: Vec<String>,
    },
}

/// A refusal to mint a certificate.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CertError {
    /// The issuing key is not a current admin.
    #[error("issuer is not an admin")]
    NotAdmin,
    /// The subject is not a current member.
    #[error("subject is not a member")]
    NotMember,
    /// The subject lacks the `lead` role, so it gets no access certificate.
    #[error("subject is not a lead")]
    NotLead,
}

/// A failure to mint, parse or pin an OpenSSH certificate or host key.
#[derive(Debug, thiserror::Error)]
pub enum SshCertError {
    /// The model refused to issue the certificate.
    #[error("{0}")]
    Cert(#[from] CertError),
    /// `ssh-keygen` or a scratch file failed.
    #[error("{0}")]
    Crypto(#[from] CryptoError),
    /// The certificate text is not a well-formed ed25519 certificate.
    #[error("malformed certificate: {0}")]
    Malformed(&'static str),
    /// No scanned host key matches the fingerprint pinned in the roster.
    #[error("host key does not match the roster (expected {expected}, saw {seen:?})")]
    PinMismatch {
        /// Fingerprint recorded in the roster.
        expected: String,
        /// Fingerprints the host actually offered.
        seen: Vec<String>,
    },
}

/// A card or welcome that cannot be accepted.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// The text lacks the expected `gwc1.` or `gww1.` prefix.
    #[error("not a {expected} string (expected the prefix `{prefix}`)")]
    Prefix {
        /// What was being decoded: "card" or "welcome".
        expected: &'static str,
        /// The required prefix.
        prefix: &'static str,
    },
    /// The text is longer than the cap for its kind.
    #[error("the {expected} is {len} characters, over the cap of {max}")]
    TooLong {
        /// What was being decoded.
        expected: &'static str,
        /// Length offered.
        len: usize,
        /// The cap.
        max: usize,
    },
    /// The text after the prefix is not base32.
    #[error("the {0} is not valid base32 (was it cut when pasted?)")]
    Base32(&'static str),
    /// The decoded payload is not the strict JSON shape for its kind.
    #[error("the {expected} is malformed: {reason}")]
    Malformed {
        /// What was being decoded.
        expected: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// A field of the card or welcome is refused.
    #[error("invalid {field}: {reason}")]
    Field {
        /// The field name.
        field: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// A failure reading or writing the circle files.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A file operation failed.
    #[error("{op} {path}: {source}")]
    Io {
        /// What was being done.
        op: &'static str,
        /// The file or directory.
        path: std::path::PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// No circle is stored here.
    #[error("no circle is set up on this machine")]
    NoCircle,
    /// More than one circle is stored; the caller must pick.
    #[error("more than one circle is stored ({0}); pass --circle")]
    Ambiguous(String),
    /// The stored `heads.json` cannot be read.
    #[error("heads file {path} is malformed: {reason}")]
    Heads {
        /// The file.
        path: std::path::PathBuf,
        /// Why.
        reason: String,
    },
    /// The operations are refused (parse, genesis or rollback).
    #[error(transparent)]
    Circle(#[from] CircleError),
    /// The operations do not contain exactly one genesis operation.
    #[error("the operations do not hold exactly one genesis")]
    NoGenesis,
    /// `ssh-keygen` failed to make or read a key.
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// A private key file exists without its `.pub`; it is never overwritten.
    #[error("private key {path} has no .pub next to it; restore it or move the key aside")]
    KeyIncomplete {
        /// The private key file.
        path: std::path::PathBuf,
    },
    /// A key file or its directory is too open, a symlink, or not a plain file.
    #[error("refusing key {path}: {reason}")]
    KeyUnsafe {
        /// The offending file or directory.
        path: std::path::PathBuf,
        /// Why it was refused.
        reason: String,
    },
    /// The stored operations file is larger than the cap.
    #[error("operations file {path} is {len} bytes, over the cap of {max}")]
    OpsTooLarge {
        /// The file.
        path: std::path::PathBuf,
        /// Its size.
        len: u64,
        /// The cap.
        max: u64,
    },
}
