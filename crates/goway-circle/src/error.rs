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
