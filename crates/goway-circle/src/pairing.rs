//! The two pasted strings of pairing: the card (`gwc1.`) and the welcome (`gww1.`).
//!
//! Both are a prefix plus base32 of strict JSON. Only public keys, names and
//! fingerprints travel; parsing is size-capped and refuses unknown fields.

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::crypto::valid_public_key;
use crate::error::PairingError;
use crate::op::{Kind, MAX_OPS, Op};

/// Prefix of a card string.
pub const CARD_PREFIX: &str = "gwc1.";
/// Prefix of a welcome string.
pub const WELCOME_PREFIX: &str = "gww1.";
/// Most characters a card may take.
pub const MAX_CARD_CHARS: usize = 4 * 1024;
/// Most characters a welcome may take.
pub const MAX_WELCOME_CHARS: usize = 4 * 1024 * 1024;
/// Longest member or circle name.
pub const MAX_NAME: usize = 64;

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Base32 (RFC 4648, no padding) of `bytes`.
fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Decode base32 text, ignoring whitespace and case; `None` on any other character.
fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        let v = match c.to_ascii_uppercase() {
            u @ 'A'..='Z' => u as u32 - 'A' as u32,
            d @ '2'..='7' => d as u32 - '2' as u32 + 26,
            _ => return None,
        };
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
            acc &= (1 << bits) - 1;
        }
    }
    // Leftover bits must be zero padding, fewer than a whole character.
    (bits < 5 && acc == 0).then_some(out)
}

/// Build `prefix` + base32 of `payload`.
pub fn encode_blob(prefix: &str, payload: &[u8]) -> String {
    format!("{prefix}{}", base32(payload))
}

/// Strip `prefix` from `text` and decode the base32 rest, refusing text over `max` characters.
pub fn decode_blob(
    kind: &'static str,
    prefix: &'static str,
    max: usize,
    text: &str,
) -> Result<Vec<u8>, PairingError> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if text.len() > max {
        warn!(kind, len = text.len(), "pasted string is over the cap");
        return Err(PairingError::TooLong {
            expected: kind,
            len: text.len(),
            max,
        });
    }
    let body = text.strip_prefix(prefix).ok_or(PairingError::Prefix {
        expected: kind,
        prefix,
    })?;
    unbase32(body).ok_or(PairingError::Base32(kind))
}

/// True for a non-empty name of at most [`MAX_NAME`] letters, digits, `.`, `_` or `-`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// True for a non-empty comma list drawn from `lead` and `helper`, without repeats.
pub fn valid_roles(roles: &str) -> bool {
    let parts: Vec<&str> = roles.split(',').collect();
    !parts.is_empty()
        && parts.iter().all(|r| matches!(*r, "lead" | "helper"))
        && parts
            .iter()
            .enumerate()
            .all(|(i, r)| !parts[..i].contains(r))
}

/// True for an empty fingerprint or `SHA256:` plus unpadded base64.
fn valid_fp(fp: &str) -> bool {
    fp.is_empty()
        || fp.strip_prefix("SHA256:").is_some_and(|b| {
            !b.is_empty()
                && b.len() <= 64
                && b.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
        })
}

fn field(field: &'static str, reason: &str) -> PairingError {
    PairingError::Field {
        field,
        reason: reason.to_owned(),
    }
}

/// What a new machine hands an admin: its node key, host fingerprint, name and wanted roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    /// Card format version (1).
    pub v: u32,
    /// The node public key (`ssh-ed25519 AAAA...`).
    pub key: String,
    /// Wanted roles, comma separated.
    pub roles: String,
    /// Display name.
    pub name: String,
    /// Fingerprint of the machine's sshd host key (empty for a pure lead).
    pub host_fp: String,
}

impl Card {
    /// Check every field; a card that passes can be shown to a human and admitted.
    pub fn validate(&self) -> Result<(), PairingError> {
        if self.v != 1 {
            return Err(field("version", "only card version 1 is understood"));
        }
        if !valid_public_key(&self.key) {
            return Err(field("key", "not a bare ed25519 public key"));
        }
        if !valid_name(&self.name) {
            return Err(field("name", "use 1-64 letters, digits, . _ -"));
        }
        if !valid_roles(&self.roles) {
            return Err(field("roles", "use lead, helper or lead,helper"));
        }
        if !valid_fp(&self.host_fp) {
            return Err(field("host fingerprint", "not a SHA256: fingerprint"));
        }
        Ok(())
    }

    /// The `gwc1.` string for this card.
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        encode_blob(CARD_PREFIX, &json)
    }

    /// Strictly parse and validate a pasted card.
    pub fn decode(text: &str) -> Result<Self, PairingError> {
        let bytes = decode_blob("card", CARD_PREFIX, MAX_CARD_CHARS, text)?;
        let card: Self = serde_json::from_slice(&bytes).map_err(|e| PairingError::Malformed {
            expected: "card",
            reason: e.to_string(),
        })?;
        card.validate()?;
        debug!(name = %card.name, "card decoded");
        Ok(card)
    }
}

/// What an admin hands back: the whole roster, which holds the genesis and the admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Welcome {
    /// Welcome format version (1).
    pub v: u32,
    /// Circle name.
    pub circle: String,
    /// Id of the genesis operation; the circle's identity.
    pub genesis: String,
    /// Every operation, parents before children.
    pub ops: Vec<Op>,
}

impl Welcome {
    /// Check shape: one named genesis among the ops, within the caps.
    pub fn validate(&self) -> Result<(), PairingError> {
        if self.v != 1 {
            return Err(field("version", "only welcome version 1 is understood"));
        }
        if !valid_name(&self.circle) {
            return Err(field("circle", "use 1-64 letters, digits, . _ -"));
        }
        if self.ops.len() > MAX_OPS {
            return Err(field("ops", "too many operations"));
        }
        let genesis_ok = self
            .ops
            .iter()
            .filter(|o| o.kind == Kind::Genesis)
            .map(Op::id)
            .eq([self.genesis.clone()]);
        if !genesis_ok {
            return Err(field(
                "genesis",
                "the ops do not hold exactly the named genesis",
            ));
        }
        Ok(())
    }

    /// The `gww1.` string for this welcome.
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        encode_blob(WELCOME_PREFIX, &json)
    }

    /// Strictly parse and validate a pasted welcome.
    pub fn decode(text: &str) -> Result<Self, PairingError> {
        let bytes = decode_blob("welcome", WELCOME_PREFIX, MAX_WELCOME_CHARS, text)?;
        let welcome: Self =
            serde_json::from_slice(&bytes).map_err(|e| PairingError::Malformed {
                expected: "welcome",
                reason: e.to_string(),
            })?;
        welcome.validate()?;
        debug!(circle = %welcome.circle, ops = welcome.ops.len(), "welcome decoded");
        Ok(welcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Card {
        Card {
            v: 1,
            key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc".into(),
            roles: "helper".into(),
            name: "box-1".into(),
            host_fp: "SHA256:abcDEF123+/".into(),
        }
    }

    #[test]
    fn base32_round_trips_every_length() {
        for n in 0..40u8 {
            let bytes: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(unbase32(&base32(&bytes)), Some(bytes));
        }
    }

    #[test]
    fn base32_matches_rfc_vector() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }

    #[test]
    fn base32_decoding_ignores_case_and_space_but_not_junk() {
        assert_eq!(unbase32("mzxw 6ytb\noi"), Some(b"foobar".to_vec()));
        assert_eq!(unbase32("MZXW6YT!"), None);
        assert_eq!(unbase32("MZXW6YTBOJ"), None);
    }

    #[test]
    fn card_round_trips() {
        let text = card().encode();
        assert!(text.starts_with(CARD_PREFIX));
        assert_eq!(Card::decode(&text).unwrap(), card());
    }

    #[test]
    fn card_survives_a_wrapped_paste() {
        let text = card().encode();
        let wrapped = format!("  {}\n{}\n", &text[..30], &text[30..]);
        assert_eq!(Card::decode(&wrapped).unwrap(), card());
    }

    #[test]
    fn card_refuses_wrong_prefix_junk_and_oversize() {
        assert!(matches!(
            Card::decode("gww1.AAAA"),
            Err(PairingError::Prefix { .. })
        ));
        assert!(matches!(
            Card::decode("gwc1.!!!"),
            Err(PairingError::Base32(_))
        ));
        let big = format!("gwc1.{}", "A".repeat(MAX_CARD_CHARS));
        assert!(matches!(
            Card::decode(&big),
            Err(PairingError::TooLong { .. })
        ));
    }

    #[test]
    fn card_refuses_unknown_fields_and_bad_values() {
        let extra = br#"{"v":1,"key":"k","roles":"helper","name":"a","host_fp":"","x":1}"#;
        let text = encode_blob(CARD_PREFIX, extra);
        assert!(matches!(
            Card::decode(&text),
            Err(PairingError::Malformed { .. })
        ));
        for mutate in [
            |c: &mut Card| c.key = "ssh-rsa AAAA".into(),
            |c: &mut Card| c.name = "bad name".into(),
            |c: &mut Card| c.roles = "root".into(),
            |c: &mut Card| c.roles = "lead,lead".into(),
            |c: &mut Card| c.host_fp = "MD5:aa".into(),
            |c: &mut Card| c.v = 2,
        ] {
            let mut c = card();
            mutate(&mut c);
            assert!(c.validate().is_err(), "{c:?}");
        }
    }

    #[test]
    fn welcome_needs_exactly_its_named_genesis() {
        let w = Welcome {
            v: 1,
            circle: "home".into(),
            genesis: "00".into(),
            ops: Vec::new(),
        };
        assert!(w.validate().is_err());
        assert!(Welcome::decode(&w.encode()).is_err());
    }
}
