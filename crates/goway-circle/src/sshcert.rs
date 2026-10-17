//! Real OpenSSH user certificates: minting with `ssh-keygen -s`, parsing, the helper's
//! acceptance rule and the `authorized_keys` lines that make a helper trust a circle.
//!
//! A lead cert carries the principals `goway-lead:<circle>:e<epoch>` (open policy) and
//! `goway-lead:<circle>:e<epoch>:lead:<name>` (listed policy). A helper's `cert-authority`
//! line lists the principals it accepts, so the epoch lives in the line: a revoke bumps the
//! epoch, helpers refresh their lines, and every older cert stops matching.
//!
//! sshd checks the signature itself; [`ParsedCert`] carries the signing key so the rule can be
//! tested here against real certs, and [`accepts`] mirrors what sshd does with the line.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tracing::{debug, info, warn};

use crate::crypto::valid_public_key;
use crate::cert::{CERT_TTL_DAYS, issue_cert, principal};
use crate::error::{CertError, CryptoError, SshCertError};
use crate::pairing::valid_name;
use crate::state::State;

/// The certificate key type written by `ssh-keygen -s` for an ed25519 key.
const CERT_KIND: &str = "ssh-ed25519-cert-v01@openssh.com";
/// Backdating, so a little clock skew between machines does not refuse a fresh cert.
const BACKDATE: &str = "-5m";
/// `authorized_keys` options on a cert-authority line: no forwarding of any kind.
const LINE_OPTIONS: &str = "no-agent-forwarding,no-port-forwarding,no-X11-forwarding";
/// Prefix of the trailing comment that marks goway's lines in `authorized_keys`.
const MARKER_PREFIX: &str = "goway-circle:";

/// The principal that names one lead, used by the `listed` policy: `<principal>:lead:<name>`.
pub fn lead_principal(state: &State, name: &str) -> String {
    format!("{}:lead:{name}", principal(state))
}

/// Which leads a helper accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    /// Every lead of the circle (a new lead needs no helper edit).
    Open,
    /// Only the named leads.
    Listed(BTreeSet<String>),
}

impl Policy {
    /// The principals a helper's line must list for `state`; empty means the line accepts nobody.
    pub fn principals(&self, state: &State) -> Vec<String> {
        match self {
            Self::Open => vec![principal(state)],
            Self::Listed(names) => names
                .iter()
                .filter(|n| valid_name(n))
                .map(|n| lead_principal(state, n))
                .collect(),
        }
    }
}

/// A parsed OpenSSH user certificate (the fields a helper's rule looks at).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCert {
    /// The certified node key, bare `ssh-ed25519 AAAA...`.
    pub subject: String,
    /// The signing (admin) key, bare `ssh-ed25519 AAAA...`.
    pub signer: String,
    /// Principals the cert names.
    pub principals: Vec<String>,
    /// The cert's key id.
    pub key_id: String,
    /// Valid from, Unix seconds.
    pub valid_after: u64,
    /// Valid until, Unix seconds.
    pub valid_before: u64,
    /// True for a user cert (type 1); a host cert is never accepted.
    pub user: bool,
}

/// Why a helper refuses a cert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// It is a host certificate.
    #[error("not a user certificate")]
    NotUser,
    /// The signer is not a current admin of the circle.
    #[error("signed by a key that is not an admin")]
    Signer,
    /// No principal matches the helper's line (wrong circle, old epoch, or not listed).
    #[error("no principal matches (revoked epoch, other circle, or not listed)")]
    Principal,
    /// It is not valid yet.
    #[error("not valid yet (check the clocks)")]
    NotYetValid,
    /// It has expired.
    #[error("expired")]
    Expired,
}

/// Cursor over an SSH wire-format blob.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SshCertError> {
        if self.0.len() < n {
            return Err(SshCertError::Malformed("truncated"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u32(&mut self) -> Result<u32, SshCertError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, SshCertError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }

    fn string(&mut self) -> Result<&'a [u8], SshCertError> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    fn text(&mut self) -> Result<String, SshCertError> {
        String::from_utf8(self.string()?.to_vec())
            .map_err(|_| SshCertError::Malformed("not utf-8"))
    }
}

/// `ssh-ed25519 <base64>` for a wire-format ed25519 public key blob.
fn bare_key(blob: &[u8]) -> String {
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

/// Wire blob of an ed25519 key from its 32 raw bytes.
fn ed25519_blob(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(51);
    out.extend_from_slice(&11u32.to_be_bytes());
    out.extend_from_slice(b"ssh-ed25519");
    out.extend_from_slice(&(raw.len() as u32).to_be_bytes());
    out.extend_from_slice(raw);
    out
}

/// Parse the text of a `*-cert.pub` file (an ed25519 user or host certificate).
pub fn parse_cert(text: &str) -> Result<ParsedCert, SshCertError> {
    let mut fields = text.split_whitespace();
    if fields.next() != Some(CERT_KIND) {
        return Err(SshCertError::Malformed("not an ed25519 certificate"));
    }
    let b64 = fields
        .next()
        .ok_or(SshCertError::Malformed("no certificate data"))?;
    let blob = STANDARD
        .decode(b64)
        .map_err(|_| SshCertError::Malformed("bad base64"))?;
    let mut r = Reader(&blob);
    if r.text()? != CERT_KIND {
        return Err(SshCertError::Malformed("type mismatch"));
    }
    r.string()?; // nonce
    let pk = r.string()?;
    if pk.len() != 32 {
        return Err(SshCertError::Malformed("bad subject key"));
    }
    let subject = bare_key(&ed25519_blob(pk));
    r.u64()?; // serial
    let kind = r.u32()?;
    let key_id = r.text()?;
    let mut principals = Vec::new();
    let mut list = Reader(r.string()?);
    while !list.0.is_empty() {
        principals.push(list.text()?);
    }
    let valid_after = r.u64()?;
    let valid_before = r.u64()?;
    r.string()?; // critical options
    r.string()?; // extensions
    r.string()?; // reserved
    let signer_blob = r.string()?;
    let mut s = Reader(signer_blob);
    // A FIDO2 admin key (ed25519-sk) signs certificates with its own key type.
    let signer_kind = match s.string()? {
        b"ssh-ed25519" => "ssh-ed25519",
        b"sk-ssh-ed25519@openssh.com" => "sk-ssh-ed25519@openssh.com",
        _ => return Err(SshCertError::Malformed("signer is not ed25519")),
    };
    let signer = format!("{signer_kind} {}", STANDARD.encode(signer_blob));
    Ok(ParsedCert {
        subject,
        signer,
        principals,
        key_id,
        valid_after,
        valid_before,
        user: kind == 1,
    })
}

/// What a helper with `policy` does with `cert` at `now`: `Ok` only when sshd would let it in.
pub fn accepts(
    state: &State,
    policy: &Policy,
    cert: &ParsedCert,
    now: u64,
) -> Result<(), Rejection> {
    if !cert.user {
        return Err(Rejection::NotUser);
    }
    if !state.admins.contains(&cert.signer) {
        return Err(Rejection::Signer);
    }
    let wanted = policy.principals(state);
    if !cert.principals.iter().any(|p| wanted.contains(p)) {
        return Err(Rejection::Principal);
    }
    if now < cert.valid_after {
        return Err(Rejection::NotYetValid);
    }
    if now >= cert.valid_before {
        return Err(Rejection::Expired);
    }
    Ok(())
}

/// Mint a lead cert for `subject` (a bare node key) signed with the admin key file.
///
/// The model rules decide first (admin signer, member subject, lead role); only then does
/// `ssh-keygen -s` run in a scratch directory. Returns the text of the `-cert.pub` file.
pub fn mint_lead_cert(
    state: &State,
    admin_pub: &str,
    admin_key: &Path,
    subject: &str,
) -> Result<String, SshCertError> {
    issue_cert(state, admin_pub, subject, 0)?;
    let name = state
        .members
        .get(subject)
        .map(|m| m.name.clone())
        .ok_or(CertError::NotMember)?;
    let principals = [principal(state), lead_principal(state, &name)].join(",");
    let key_id = format!("goway:{}:{name}", state.circle);
    let dir = tempfile::tempdir().map_err(CryptoError::from)?;
    let pub_file = dir.path().join("subject.pub");
    std::fs::write(&pub_file, format!("{subject}\n")).map_err(CryptoError::from)?;
    // With no private file the admin key lives in ssh-agent or on a FIDO token: hand
    // ssh-keygen the public half and `-U` so it asks the agent to sign.
    let agent = !admin_key.exists();
    let ca_file = dir.path().join("ca.pub");
    if agent {
        std::fs::write(&ca_file, format!("{admin_pub}\n")).map_err(CryptoError::from)?;
        info!("admin key has no private file; signing through ssh-agent");
    }
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-q", "-s"]).arg(if agent { &ca_file } else { admin_key });
    if agent {
        cmd.arg("-U");
    }
    let out = cmd
        .args(["-I", &key_id, "-n", &principals, "-V"])
        .arg(format!("{BACKDATE}:+{CERT_TTL_DAYS}d"))
        .arg(&pub_file)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| CryptoError::Tool(format!("cannot run ssh-keygen: {e}")))?;
    if !out.status.success() {
        warn!(status = ?out.status, "ssh-keygen could not sign a certificate");
        return Err(
            CryptoError::Tool(String::from_utf8_lossy(&out.stderr).trim().to_owned()).into(),
        );
    }
    let text =
        std::fs::read_to_string(dir.path().join("subject-cert.pub")).map_err(CryptoError::from)?;
    info!(circle = %state.circle, epoch = state.epoch, lead = %name, "lead certificate minted");
    Ok(text)
}

/// The marker (trailing comment) of goway's lines for `state`: `goway-circle:<circle>:e<epoch>`.
pub fn line_marker(state: &State) -> String {
    format!("{MARKER_PREFIX}{}:e{}", state.circle, state.epoch)
}

/// The `cert-authority` lines a helper needs for `state` under `policy`: one per admin key.
pub fn authority_lines(state: &State, policy: &Policy) -> Vec<String> {
    let principals = policy.principals(state);
    if principals.is_empty() {
        return Vec::new();
    }
    let marker = line_marker(state);
    state
        .admins
        .iter()
        .filter(|admin| valid_public_key(admin))
        .map(|admin| {
            format!(
                "cert-authority,principals=\"{}\",{LINE_OPTIONS} {admin} {marker}",
                principals.join(",")
            )
        })
        .collect()
}

/// `existing` with this circle's old goway lines replaced by `lines`; other lines are kept.
pub fn merge_authorized_keys(existing: &str, state: &State, lines: &[String]) -> String {
    let mine = format!("{MARKER_PREFIX}{}:", state.circle);
    let mut out: String = existing
        .split_inclusive('\n')
        .filter(|l| {
            let stale = l
                .split_whitespace()
                .last()
                .is_some_and(|last| last.starts_with(&mine));
            if stale {
                debug!("dropping a stale circle line");
            }
            !stale
        })
        .collect();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A scanned host key line that matches the pinned fingerprint, as a `known_hosts` line.
///
/// `scanned` is `ssh-keyscan` output; only an ed25519 key whose fingerprint equals `expected`
/// is returned, under `alias`.
pub fn pin_line(alias: &str, scanned: &str, expected: &str) -> Result<String, SshCertError> {
    let mut seen = Vec::new();
    for line in scanned.lines().filter(|l| !l.starts_with('#')) {
        let mut f = line.split_whitespace();
        let (Some(_host), Some("ssh-ed25519"), Some(blob)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let key = format!("ssh-ed25519 {blob}");
        let Some(fp) = crate::keys::key_fingerprint(&key) else {
            continue;
        };
        if fp == expected {
            return Ok(format!("{alias} {key}"));
        }
        seen.push(fp);
    }
    // frob:invariant INV-CIRCLE-003
    warn!(alias, expected, ?seen, "host key does not match the roster");
    Err(SshCertError::PinMismatch {
        expected: expected.to_owned(),
        seen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::ensure_keypair;
    use crate::state::{Member, Role};

    struct Rig {
        _dir: tempfile::TempDir,
        admin: crate::keys::KeyPair,
        lead: crate::keys::KeyPair,
        other: crate::keys::KeyPair,
        state: State,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let mk = |n: &str| ensure_keypair(&dir.path().join(n), &|_| {}).unwrap().0;
        let (admin, lead, other) = (mk("admin"), mk("lead"), mk("other"));
        let mut state = State {
            circle: "home".to_owned(),
            ..State::default()
        };
        state.admins.insert(admin.public.clone());
        let member = |name: &str, role| Member {
            name: name.to_owned(),
            roles: [role].into(),
            host_fp: String::new(),
        };
        state
            .members
            .insert(admin.public.clone(), member("lap", Role::Lead));
        state
            .members
            .insert(lead.public.clone(), member("alice", Role::Lead));
        state
            .members
            .insert(other.public.clone(), member("box", Role::Helper));
        Rig {
            _dir: dir,
            admin,
            lead,
            other,
            state,
        }
    }

    fn mint(r: &Rig) -> ParsedCert {
        let text =
            mint_lead_cert(&r.state, &r.admin.public, &r.admin.private, &r.lead.public).unwrap();
        parse_cert(&text).unwrap()
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::accepts
    #[test]
    fn a_real_cert_is_accepted_by_an_open_helper() {
        let r = rig();
        let cert = mint(&r);
        assert_eq!(cert.subject, r.lead.public);
        assert_eq!(cert.signer, r.admin.public);
        assert!(cert.user);
        assert!(
            cert.principals
                .contains(&"goway-lead:home:e0".to_owned())
        );
        assert_eq!(accepts(&r.state, &Policy::Open, &cert, NOW_REAL()), Ok(()));
    }

    #[allow(non_snake_case)]
    fn NOW_REAL() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::accepts
    #[test]
    fn a_new_lead_needs_no_helper_edit() {
        let mut r = rig();
        let before = authority_lines(&r.state, &Policy::Open);
        let newcomer = ensure_keypair(&r._dir.path().join("new"), &|_| {})
            .unwrap()
            .0;
        r.state.members.insert(
            newcomer.public.clone(),
            Member {
                name: "carol".to_owned(),
                roles: [Role::Lead].into(),
                host_fp: String::new(),
            },
        );
        let text =
            mint_lead_cert(&r.state, &r.admin.public, &r.admin.private, &newcomer.public).unwrap();
        let cert = parse_cert(&text).unwrap();
        assert_eq!(authority_lines(&r.state, &Policy::Open), before);
        assert_eq!(accepts(&r.state, &Policy::Open, &cert, NOW_REAL()), Ok(()));
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::accepts
    #[test]
    fn a_bumped_epoch_refuses_old_certs() {
        let mut r = rig();
        let cert = mint(&r);
        r.state.epoch = 1;
        assert_eq!(
            accepts(&r.state, &Policy::Open, &cert, NOW_REAL()),
            Err(Rejection::Principal)
        );
        assert_ne!(authority_lines(&r.state, &Policy::Open), {
            r.state.epoch = 0;
            authority_lines(&r.state, &Policy::Open)
        });
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::accepts
    #[test]
    fn expired_and_early_certs_are_refused() {
        let r = rig();
        let cert = mint(&r);
        let day = 86_400;
        assert_eq!(
            accepts(&r.state, &Policy::Open, &cert, cert.valid_before),
            Err(Rejection::Expired)
        );
        assert_eq!(
            accepts(&r.state, &Policy::Open, &cert, cert.valid_before + day),
            Err(Rejection::Expired)
        );
        assert_eq!(
            accepts(&r.state, &Policy::Open, &cert, cert.valid_after - 1),
            Err(Rejection::NotYetValid)
        );
        assert_eq!(cert.valid_before - cert.valid_after, (14 * day) + 300);
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::accepts
    #[test]
    fn a_cert_from_a_non_admin_signer_is_refused() {
        let r = rig();
        // Sign with a key the roster does not list as admin.
        let text = Command::new("ssh-keygen")
            .args(["-q", "-s"])
            .arg(&r.other.private)
            .args(["-I", "x", "-n", "goway-lead:home:e0", "-V", "-5m:+14d"])
            .arg({
                let p = r._dir.path().join("subj.pub");
                std::fs::write(&p, format!("{}\n", r.lead.public)).unwrap();
                p
            })
            .output()
            .unwrap();
        assert!(text.status.success());
        let cert = parse_cert(
            &std::fs::read_to_string(r._dir.path().join("subj-cert.pub")).unwrap(),
        )
        .unwrap();
        assert_eq!(cert.signer, r.other.public);
        assert_eq!(
            accepts(&r.state, &Policy::Open, &cert, NOW_REAL()),
            Err(Rejection::Signer)
        );
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::mint_lead_cert
    #[test]
    fn only_a_member_lead_gets_a_cert_from_an_admin() {
        let r = rig();
        let mint = |admin: &crate::keys::KeyPair, who: &str| {
            mint_lead_cert(&r.state, &admin.public, &admin.private, who)
        };
        assert!(matches!(
            mint(&r.other, &r.lead.public),
            Err(SshCertError::Cert(CertError::NotAdmin))
        ));
        assert!(matches!(
            mint(&r.admin, &r.other.public),
            Err(SshCertError::Cert(CertError::NotLead))
        ));
        assert!(matches!(
            mint(&r.admin, "ssh-ed25519 AAAAnotamember"),
            Err(SshCertError::Cert(CertError::NotMember))
        ));
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::accepts
    #[test]
    fn the_listed_policy_names_leads() {
        let r = rig();
        let cert = mint(&r);
        let only = |n: &str| Policy::Listed([n.to_owned()].into());
        assert_eq!(accepts(&r.state, &only("alice"), &cert, NOW_REAL()), Ok(()));
        assert_eq!(
            accepts(&r.state, &only("bob"), &cert, NOW_REAL()),
            Err(Rejection::Principal)
        );
        assert!(authority_lines(&r.state, &Policy::Listed(BTreeSet::new())).is_empty());
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::Policy
    #[test]
    fn listed_names_that_could_inject_options_are_dropped() {
        let r = rig();
        let evil: BTreeSet<String> = ["x\",command=\"id".to_owned(), "a b".to_owned()].into();
        assert!(Policy::Listed(evil).principals(&r.state).is_empty());
    }

    fn put(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::parse_cert
    #[test]
    fn a_fido_signed_cert_names_its_sk_signer() {
        let mut signer = Vec::new();
        put(&mut signer, b"sk-ssh-ed25519@openssh.com");
        put(&mut signer, &[7u8; 32]);
        put(&mut signer, b"ssh:");
        let mut blob = Vec::new();
        put(&mut blob, CERT_KIND.as_bytes());
        put(&mut blob, &[0u8; 32]);
        put(&mut blob, &[1u8; 32]);
        blob.extend_from_slice(&0u64.to_be_bytes());
        blob.extend_from_slice(&1u32.to_be_bytes());
        put(&mut blob, b"id");
        let mut names = Vec::new();
        put(&mut names, b"p1");
        put(&mut blob, &names);
        blob.extend_from_slice(&0u64.to_be_bytes());
        blob.extend_from_slice(&10u64.to_be_bytes());
        for _ in 0..3 {
            put(&mut blob, b"");
        }
        put(&mut blob, &signer);
        put(&mut blob, b"sig");
        let text = format!("{CERT_KIND} {}", STANDARD.encode(&blob));
        let cert = parse_cert(&text).unwrap();
        assert!(cert.signer.starts_with("sk-ssh-ed25519@openssh.com "));
        assert!(crate::crypto::valid_public_key(&cert.signer));
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::authority_lines
    #[test]
    fn an_admin_key_with_a_newline_never_reaches_authorized_keys() {
        let mut r = rig();
        r.state.admins.insert("ssh-ed25519 AAAA\nssh-rsa EVIL".to_owned());
        let lines = authority_lines(&r.state, &Policy::Open);
        assert!(lines.iter().all(|l| !l.contains('\n') && !l.contains("EVIL")));
    }

    #[test]
    fn a_host_or_garbage_cert_does_not_parse_as_user() {
        assert!(parse_cert("ssh-ed25519 AAAA").is_err());
        assert!(parse_cert("ssh-ed25519-cert-v01@openssh.com !!").is_err());
        assert!(parse_cert("ssh-ed25519-cert-v01@openssh.com AAAA").is_err());
        assert!(parse_cert("").is_err());
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::authority_lines
    #[test]
    fn lines_are_restricted_and_replace_the_old_epoch() {
        let mut r = rig();
        let lines = authority_lines(&r.state, &Policy::Open);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with(
            "cert-authority,principals=\"goway-lead:home:e0\",no-agent-forwarding,no-port-forwarding,no-X11-forwarding ssh-ed25519 "
        ));
        assert!(lines[0].ends_with(" goway-circle:home:e0"));
        let file = merge_authorized_keys("ssh-ed25519 KEEP me@x\n", &r.state, &lines);
        r.state.epoch = 1;
        let next = authority_lines(&r.state, &Policy::Open);
        let file = merge_authorized_keys(&file, &r.state, &next);
        assert!(file.contains("ssh-ed25519 KEEP me@x\n"));
        assert!(file.contains("goway-lead:home:e1"));
        assert!(!file.contains("goway-lead:home:e0"));
        assert_eq!(file.lines().count(), 2);
        assert_eq!(merge_authorized_keys(&file, &r.state, &next), file);
    }

    // frob:tests crates/goway-circle/src/sshcert.rs::pin_line
    #[test]
    fn pinning_needs_the_roster_fingerprint() {
        let r = rig();
        let blob = r.other.public.split(' ').nth(1).unwrap();
        let scan = format!("# host:22 SSH-2.0\nbox ssh-rsa AAAA\nbox ssh-ed25519 {blob}\n");
        let fp = crate::keys::key_fingerprint(&r.other.public).unwrap();
        assert_eq!(
            pin_line("goway-box", &scan, &fp).unwrap(),
            format!("goway-box {}", r.other.public)
        );
        let wrong = crate::keys::key_fingerprint(&r.lead.public).unwrap();
        assert!(matches!(
            pin_line("goway-box", &scan, &wrong),
            Err(SshCertError::PinMismatch { .. })
        ));
        assert!(pin_line("goway-box", "", &fp).is_err());
    }
}
