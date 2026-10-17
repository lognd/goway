//! Signed roster operations: content id, strict parsing and size caps.

use std::collections::{BTreeMap, BTreeSet};

use goway_journal::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::canon::canonical;
use crate::crypto::{Signer, Verifier};
use crate::error::CircleError;

/// Most operations a roster may hold.
pub const MAX_OPS: usize = 4096;
/// Most encoded bytes one operation may take.
pub const MAX_OP_BYTES: usize = 64 * 1024;

/// What an operation does to the roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Found the circle with its first admin.
    Genesis,
    /// Add a member.
    Admit,
    /// Remove a member forever.
    Revoke,
    /// Change a member's roles.
    SetRoles,
    /// Promote a lead to admin.
    AddAdmin,
    /// Demote an admin.
    RemoveAdmin,
    /// A node replaces its own host-key fingerprint.
    RotateHostKey,
}

impl Kind {
    /// True for kinds applied first inside a causal layer so removal wins over concurrent grants.
    pub fn is_removing(self) -> bool {
        matches!(self, Self::Revoke | Self::RemoveAdmin)
    }
}

/// A signature by one signer over an operation id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sig {
    /// Public key of the signer.
    pub signer: String,
    /// Signature text from the signing backend.
    pub sig: String,
}

/// One signed roster operation; its id covers kind, body, parents and signers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "WireOp", into = "WireOp")]
pub struct Op {
    /// What the operation does.
    pub kind: Kind,
    /// String fields interpreted by `kind`.
    pub body: BTreeMap<String, String>,
    /// Ids of the operations the author had seen, sorted.
    pub parents: Vec<String>,
    /// Public keys that signed; the first is the author.
    pub signers: Vec<String>,
    /// Signatures, one per signer; excluded from the id.
    pub sigs: Vec<Sig>,
}

/// The on-disk shape: the same fields plus the (verified, never trusted) id.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireOp {
    id: Option<String>,
    kind: Kind,
    body: BTreeMap<String, String>,
    parents: Vec<String>,
    signers: Vec<String>,
    sigs: Vec<Sig>,
}

impl From<Op> for WireOp {
    fn from(op: Op) -> Self {
        Self {
            id: Some(op.id()),
            kind: op.kind,
            body: op.body,
            parents: op.parents,
            signers: op.signers,
            sigs: op.sigs,
        }
    }
}

impl TryFrom<WireOp> for Op {
    type Error = String;

    fn try_from(w: WireOp) -> Result<Self, String> {
        let op = Self {
            kind: w.kind,
            body: w.body,
            parents: w.parents,
            signers: w.signers,
            sigs: w.sigs,
        };
        match w.id {
            Some(id) if id != op.id() => Err("id does not match content".into()),
            _ => Ok(op),
        }
    }
}

impl Op {
    /// Content id: SHA-256 of the canonical JSON of everything except the signatures.
    pub fn id(&self) -> String {
        let value = json!({
            "kind": self.kind,
            "body": self.body,
            "parents": self.parents,
            "signers": self.signers,
        });
        sha256_hex(canonical(&value).as_bytes())
    }

    /// Build an op and attach a signature from each signer over its id.
    pub fn signed(
        kind: Kind,
        body: &[(&str, &str)],
        parents: &[String],
        signers: &[&dyn Signer],
    ) -> Result<Self, CircleError> {
        let mut parents = parents.to_vec();
        parents.sort();
        parents.dedup();
        let mut op = Self {
            kind,
            body: body
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            parents,
            signers: signers.iter().map(|s| s.public_key().to_owned()).collect(),
            sigs: Vec::new(),
        };
        let id = op.id();
        for s in signers {
            op.sigs.push(Sig {
                signer: s.public_key().to_owned(),
                sig: s.sign(id.as_bytes())?,
            });
        }
        Ok(op)
    }

    /// Return true when there is at least one signer and each distinct signer has a valid signature.
    pub fn sig_ok(&self, verifier: &dyn Verifier) -> bool {
        let signers: BTreeSet<&str> = self.signers.iter().map(String::as_str).collect();
        let signed: BTreeSet<&str> = self.sigs.iter().map(|s| s.signer.as_str()).collect();
        if signers.is_empty()
            || signers.len() != self.signers.len()
            || signers != signed
            || self.sigs.len() != signers.len()
        {
            return false;
        }
        let id = self.id();
        self.sigs
            .iter()
            .all(|s| verifier.verify(&s.signer, id.as_bytes(), &s.sig))
    }

    /// Encoded size in bytes, as checked against [`MAX_OP_BYTES`].
    pub fn encoded_len(&self) -> usize {
        serde_json::to_string(self).map_or(usize::MAX, |s| s.len())
    }

    /// One compact JSON line (no newline) for `ops.jsonl`.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Strictly parse `ops.jsonl` text: unknown fields, bad ids and oversize lines are errors.
pub fn parse_ops(text: &str) -> Result<Vec<Op>, CircleError> {
    let mut ops = Vec::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let line_no = i + 1;
        if line.len() > MAX_OP_BYTES {
            return Err(CircleError::OpTooLarge {
                bytes: line.len(),
                max: MAX_OP_BYTES,
            });
        }
        if ops.len() >= MAX_OPS {
            return Err(CircleError::TooManyOps {
                count: ops.len() + 1,
                max: MAX_OPS,
            });
        }
        let value: Value = serde_json::from_str(line).map_err(|source| CircleError::Parse {
            line: line_no,
            source,
        })?;
        let op = serde_json::from_value::<Op>(value).map_err(|source| {
            if source.to_string().contains("id does not match") {
                CircleError::IdMismatch { line: line_no }
            } else {
                CircleError::Parse {
                    line: line_no,
                    source,
                }
            }
        })?;
        ops.push(op);
    }
    Ok(ops)
}
