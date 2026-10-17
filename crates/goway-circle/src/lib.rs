//! Circle rosters: signed operations, a deterministic fold, merge, anti-rollback
//! and the certificate rules. The rules mirror `docs/model/circle_model.py`.
//!
//! Signatures go through the [`Signer`] and [`Verifier`] traits; the real
//! backend shells to `ssh-keygen -Y` (namespace `goway-circle-v1`, ed25519 only)
//! and [`FakeCrypto`] stands in for tests.

mod canon;
mod cert;
mod crypto;
mod error;
mod keys;
mod keystore;
mod lan;
mod op;
mod pairing;
mod passphrase;
mod sshcert;
mod state;
mod store;

pub use cert::{CERT_TTL_DAYS, CERT_TTL_SECS, Cert, helper_accepts, issue_cert, principal};
pub use crypto::{
    FakeCrypto, NAMESPACE, Signer, SshKeygenSigner, SshKeygenVerifier, Verifier, valid_public_key,
};
pub use error::{
    CertError, CircleError, CryptoError, KeystoreError, PairingError, SshCertError, StoreError,
};
pub use keys::{
    KeyPair, ensure_keypair, ensure_keypair_with, generate_sk_keypair, key_fingerprint,
    read_public_key,
};
pub use keystore::{FakeKeystore, Keystore, KeystorePassphrase, SecretToolKeystore};
pub use lan::{
    HostOutcome, LanError, ListenGuard, Msg, Refusal, new_code, normalize_code, read_msg,
    run_guest, run_host, write_msg,
};
pub use op::{Kind, MAX_OP_BYTES, MAX_OPS, Op, Sig, parse_ops};
pub use pairing::{Card, Welcome, decode_blob, encode_blob, valid_name, valid_roles};
pub use passphrase::{
    FixedPassphrase, Passphrase, PassphraseSource, ScriptedPassphrase, TtyPrompt,
    ask_new_passphrase, key_is_encrypted, passphrase_unlocks_key,
};
pub use sshcert::{
    ParsedCert, Policy, Rejection, accepts, authority_lines, lead_principal, line_marker,
    merge_authorized_keys, mint_lead_cert, parse_cert, pin_line,
};
pub use state::{Member, Role, State, accept_roster, fold, fold_pinned, heads, merge};
pub use store::{FileSink, PlainSink, Store};
