//! LAN pairing: a commit-reveal exchange of the card and welcome over one TCP stream,
//! authenticated by a short authentication string (SAS) a person compares on both screens.
//!
//! Design note: `CPace` needs a prime-order group (ristretto255), and no curve crate is in the
//! dependency tree, so this is the closest safe design that needs only SHA-256. The guest commits
//! to its nonce and card before the host reveals anything, so a man in the middle cannot grind
//! for a matching SAS: it has about one chance in a million per attempt, and the listener is
//! single use, so a mismatch ends it. There is no shared code and nothing for an offline guess
//! to attack. Nothing changes on either machine until both people have confirmed the SAS.
//!
//! Flow: guest `commit` -> host `nonce` (+ its admin key fingerprints) -> guest `reveal` (nonce +
//! card) -> both show the SAS -> guest `confirm` -> host admits and sends `welcome`.

use std::io::{BufRead, Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::pairing::{Card, MAX_CARD_CHARS, MAX_WELCOME_CHARS, Welcome};

/// Bytes of each side's random nonce.
const NONCE_BYTES: usize = 16;
/// Most characters of any message before the welcome.
const MAX_SMALL_MSG: usize = MAX_CARD_CHARS + 1024;
/// Most fingerprints a host may present.
const MAX_IDENTS: usize = 4;
/// Longest fingerprint text.
const MAX_IDENT_LEN: usize = 96;
/// Longest abort reason kept.
const MAX_REASON: usize = 200;

/// Why a LAN pairing session failed; none of these changed anything on this machine.
#[derive(Debug, thiserror::Error)]
pub enum LanError {
    /// The stream failed or timed out.
    #[error("connection problem: {0}")]
    Io(#[from] std::io::Error),
    /// The peer sent something outside the protocol.
    #[error("the other machine broke the pairing protocol: {0}")]
    Protocol(String),
    /// The guest's reveal does not match its commitment (a sign of tampering).
    #[error("the commitment does not match the reveal; pairing refused")]
    CommitMismatch,
    /// The local person declined (or typed a different code); nothing was changed.
    #[error("the confirmation code was not accepted here; nothing was changed")]
    Declined,
    /// The other machine stopped the pairing.
    #[error("the other machine stopped the pairing: {0}")]
    PeerAborted(String),
    /// The host could not admit the card.
    #[error("could not admit the card: {0}")]
    Admit(String),
    /// The system random source failed.
    #[error("no random source: {0}")]
    Random(String),
}

/// One message of the pairing exchange (a JSON line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase", deny_unknown_fields)]
pub enum Msg {
    /// Guest: hash of its nonce and card.
    Commit {
        /// Hex of the commitment.
        c: String,
    },
    /// Host: its nonce and the admin key fingerprints it will vouch with.
    Nonce {
        /// Hex of the host nonce.
        n: String,
        /// Admin key fingerprints of the host.
        ident: Vec<String>,
    },
    /// Guest: opens the commitment.
    Reveal {
        /// Hex of the guest nonce.
        n: String,
        /// The `gwc1.` card.
        card: String,
    },
    /// Guest: whether its person accepted the SAS.
    Confirm {
        /// True when accepted.
        ok: bool,
    },
    /// Host: the `gww1.` welcome, sent only after both confirmations.
    Welcome {
        /// The welcome string.
        welcome: String,
    },
    /// Either side: stop.
    Abort {
        /// A short reason.
        reason: String,
    },
}

fn protocol(reason: impl Into<String>) -> LanError {
    LanError::Protocol(reason.into())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// A fresh random nonce.
pub fn new_nonce() -> Result<[u8; NONCE_BYTES], LanError> {
    let mut n = [0u8; NONCE_BYTES];
    getrandom::fill(&mut n).map_err(|e| LanError::Random(e.to_string()))?;
    Ok(n)
}

fn nonce_of(text: &str) -> Result<[u8; NONCE_BYTES], LanError> {
    unhex(text)
        .and_then(|v| <[u8; NONCE_BYTES]>::try_from(v).ok())
        .ok_or_else(|| protocol("bad nonce"))
}

fn feed(h: &mut Sha256, part: &[u8]) {
    h.update((part.len() as u64).to_be_bytes());
    h.update(part);
}

/// The guest's commitment to `nonce` and `card_text`.
pub fn commitment(nonce: &[u8; NONCE_BYTES], card_text: &str) -> String {
    let mut h = Sha256::new();
    feed(&mut h, b"goway-lan-commit-v1");
    feed(&mut h, nonce);
    feed(&mut h, card_text.as_bytes());
    hex(&h.finalize())
}

/// The six-digit SAS ("123 456") both people compare; it binds both nonces, the card and the
/// host's presented fingerprints, so any change in flight changes it.
pub fn sas(
    guest: &[u8; NONCE_BYTES],
    host: &[u8; NONCE_BYTES],
    idents: &[String],
    card_text: &str,
) -> String {
    let mut h = Sha256::new();
    feed(&mut h, b"goway-lan-sas-v1");
    feed(&mut h, guest);
    feed(&mut h, host);
    feed(&mut h, card_text.as_bytes());
    for i in idents {
        feed(&mut h, i.as_bytes());
    }
    let d = h.finalize();
    let n = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// True when `typed` holds the same six digits as `shown`, ignoring spaces and dashes.
pub fn sas_matches(shown: &str, typed: &str) -> bool {
    let digits = |s: &str| -> String { s.chars().filter(char::is_ascii_digit).collect() };
    let (a, b) = (digits(shown), digits(typed));
    a.len() == 6 && a == b && typed.chars().all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '-')
}

/// Write one message as a JSON line.
pub fn write_msg<W: Write>(w: &mut W, msg: &Msg) -> Result<(), LanError> {
    let mut line = serde_json::to_vec(msg).map_err(|e| protocol(e.to_string()))?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()?;
    Ok(())
}

/// Read one capped JSON-line message.
pub fn read_msg<R: BufRead>(r: &mut R, max: usize) -> Result<Msg, LanError> {
    let mut buf = Vec::new();
    r.by_ref().take(max as u64 + 1).read_until(b'\n', &mut buf)?;
    if buf.last() != Some(&b'\n') {
        return Err(if buf.len() > max {
            protocol("message too long")
        } else {
            LanError::Io(std::io::ErrorKind::UnexpectedEof.into())
        });
    }
    serde_json::from_slice(&buf).map_err(|e| protocol(e.to_string()))
}

fn abort_text(reason: &str) -> String {
    reason.chars().filter(char::is_ascii_graphic).take(MAX_REASON).collect()
}

fn check_idents(idents: &[String]) -> Result<(), LanError> {
    let ok = !idents.is_empty()
        && idents.len() <= MAX_IDENTS
        && idents
            .iter()
            .all(|i| !i.is_empty() && i.len() <= MAX_IDENT_LEN && i.is_ascii());
    ok.then_some(()).ok_or_else(|| protocol("bad fingerprint list"))
}

/// What the host's session produced.
#[derive(Debug)]
pub struct HostOutcome {
    /// The card that was admitted.
    pub card: Card,
}

/// Run the host (admin) side on `reader`/`writer`: `confirm` is asked with the SAS and the card
/// and must return true only after a person matched it; `admit` runs only after both people
/// confirmed and returns the welcome to send.
pub fn run_host<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    idents: &[String],
    confirm: &mut dyn FnMut(&str, &Card) -> bool,
    admit: &mut dyn FnMut(&Card) -> Result<Welcome, String>,
) -> Result<HostOutcome, LanError> {
    check_idents(idents)?;
    let Msg::Commit { c } = read_msg(reader, MAX_SMALL_MSG)? else {
        return Err(protocol("expected commit"));
    };
    let host_n = new_nonce()?;
    write_msg(writer, &Msg::Nonce { n: hex(&host_n), ident: idents.to_vec() })?;
    let Msg::Reveal { n, card: card_text } = read_msg(reader, MAX_SMALL_MSG)? else {
        return Err(protocol("expected reveal"));
    };
    let guest_n = nonce_of(&n)?;
    if commitment(&guest_n, &card_text) != c {
        warn!("lan pairing: commitment mismatch");
        let _ = write_msg(writer, &Msg::Abort { reason: "commitment mismatch".into() });
        return Err(LanError::CommitMismatch);
    }
    let card = Card::decode(&card_text).map_err(|e| protocol(e.to_string()))?;
    let code = sas(&guest_n, &host_n, idents, &card_text);
    debug!(name = %card.name, "lan pairing: sas computed on host");
    if !confirm(&code, &card) {
        warn!("lan pairing: host person declined");
        let _ = write_msg(writer, &Msg::Abort { reason: "declined on the admin machine".into() });
        return Err(LanError::Declined);
    }
    match read_msg(reader, MAX_SMALL_MSG)? {
        Msg::Confirm { ok: true } => {}
        Msg::Confirm { ok: false } | Msg::Abort { .. } => {
            warn!("lan pairing: guest declined");
            return Err(LanError::PeerAborted("declined on the new machine".into()));
        }
        _ => return Err(protocol("expected confirm")),
    }
    let welcome = admit(&card).map_err(|e| {
        let _ = write_msg(writer, &Msg::Abort { reason: abort_text(&e) });
        LanError::Admit(e)
    })?;
    write_msg(writer, &Msg::Welcome { welcome: welcome.encode() })?;
    info!(name = %card.name, "lan pairing: welcome sent");
    Ok(HostOutcome { card })
}

/// Run the guest (new machine) side: `confirm` is asked with the SAS and must return true only
/// after a person matched it. Returns the welcome and the fingerprints the host presented, which
/// the caller must check against the admins in the welcome.
pub fn run_guest<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    card: &Card,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<(Welcome, Vec<String>), LanError> {
    let card_text = card.encode();
    let guest_n = new_nonce()?;
    write_msg(writer, &Msg::Commit { c: commitment(&guest_n, &card_text) })?;
    let Msg::Nonce { n, ident } = read_msg(reader, MAX_SMALL_MSG)? else {
        return Err(protocol("expected nonce"));
    };
    let host_n = nonce_of(&n)?;
    check_idents(&ident)?;
    write_msg(writer, &Msg::Reveal { n: hex(&guest_n), card: card_text.clone() })?;
    let code = sas(&guest_n, &host_n, &ident, &card_text);
    if !confirm(&code) {
        warn!("lan pairing: guest person declined");
        let _ = write_msg(writer, &Msg::Confirm { ok: false });
        return Err(LanError::Declined);
    }
    write_msg(writer, &Msg::Confirm { ok: true })?;
    match read_msg(reader, MAX_WELCOME_CHARS + 1024)? {
        Msg::Welcome { welcome } => {
            let w = Welcome::decode(&welcome).map_err(|e| protocol(e.to_string()))?;
            Ok((w, ident))
        }
        Msg::Abort { reason } => Err(LanError::PeerAborted(abort_text(&reason))),
        _ => Err(protocol("expected welcome")),
    }
}

/// Why a connection is not served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The listener's lifetime is over.
    #[error("the pairing window has closed")]
    Expired,
    /// A session already reached the code comparison; the listener is single use.
    #[error("the pairing listener was already used")]
    Used,
    /// Too many failed attempts.
    #[error("too many failed attempts")]
    Strikes,
    /// A connection came too soon after the last.
    #[error("connections are coming too fast")]
    TooFast,
}

/// Policy of the short-lived listener: expiry, single use, a minimum gap, limited failures.
#[derive(Debug, Clone)]
pub struct ListenGuard {
    expires: u64,
    min_gap: u64,
    max_strikes: u32,
    last: Option<u64>,
    strikes: u32,
    used: bool,
}

impl ListenGuard {
    /// A guard open from `now` for `ttl` seconds (seconds on any monotonic clock).
    pub fn new(now: u64, ttl: u64) -> Self {
        Self { expires: now + ttl, min_gap: 1, max_strikes: 5, last: None, strikes: 0, used: false }
    }

    /// Decide whether a connection at `now` may be served.
    pub fn allow(&mut self, now: u64) -> Result<(), Refusal> {
        let verdict = if now >= self.expires {
            Err(Refusal::Expired)
        } else if self.used {
            Err(Refusal::Used)
        } else if self.strikes >= self.max_strikes {
            Err(Refusal::Strikes)
        } else if self.last.is_some_and(|l| now.saturating_sub(l) < self.min_gap) {
            Err(Refusal::TooFast)
        } else {
            Ok(())
        };
        if verdict.is_ok() {
            self.last = Some(now);
        } else {
            warn!(?verdict, "lan pairing: connection refused");
        }
        verdict
    }

    /// Record a session that failed before the code comparison.
    pub fn strike(&mut self) {
        self.strikes += 1;
    }

    /// Record that a session reached the code comparison: the listener is spent either way.
    pub fn consume(&mut self) {
        self.used = true;
    }

    /// True when no further connection can ever be served.
    pub fn is_spent(&self, now: u64) -> bool {
        now >= self.expires || self.used || self.strikes >= self.max_strikes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;
    use std::net::{TcpListener, TcpStream};

    fn card() -> Card {
        Card {
            v: 1,
            key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc".into(),
            roles: "helper".into(),
            name: "box-1".into(),
            host_fp: String::new(),
        }
    }

    fn welcome() -> Welcome {
        let admin = crate::FakeCrypto::new("admin");
        let genesis = crate::Op::signed(
            crate::Kind::Genesis,
            &[("circle", "home"), ("admin", "admin"), ("name", "a-admin")],
            &[],
            &[&admin as &dyn crate::Signer],
        )
        .unwrap();
        Welcome { v: 1, circle: "home".into(), genesis: genesis.id(), ops: vec![genesis] }
    }

    #[test]
    fn sas_changes_with_every_input() {
        let (g, h) = ([1u8; 16], [2u8; 16]);
        let idents = vec!["SHA256:aa".to_owned()];
        let base = sas(&g, &h, &idents, "card");
        assert_eq!(base, sas(&g, &h, &idents, "card"));
        assert_ne!(base, sas(&[3u8; 16], &h, &idents, "card"));
        assert_ne!(base, sas(&g, &[3u8; 16], &idents, "card"));
        assert_ne!(base, sas(&g, &h, &["SHA256:bb".to_owned()], "card"));
        assert_ne!(base, sas(&g, &h, &idents, "card2"));
        assert_eq!(base.len(), 7);
    }

    #[test]
    fn typed_codes_must_match_exactly() {
        assert!(sas_matches("123 456", "123456"));
        assert!(sas_matches("123 456", " 123-456\n"));
        assert!(!sas_matches("123 456", "123457"));
        assert!(!sas_matches("123 456", "12345"));
        assert!(!sas_matches("123 456", "y"));
        assert!(!sas_matches("123 456", "123456abc"));
    }

    #[test]
    fn messages_refuse_unknown_fields_and_oversize() {
        let mut ok = BufReader::new(&b"{\"t\":\"confirm\",\"ok\":true}\n"[..]);
        assert_eq!(read_msg(&mut ok, 100).unwrap(), Msg::Confirm { ok: true });
        let mut extra = BufReader::new(&b"{\"t\":\"confirm\",\"ok\":true,\"x\":1}\n"[..]);
        assert!(read_msg(&mut extra, 100).is_err());
        let mut big = BufReader::new(&b"{\"t\":\"abort\",\"reason\":\"aaaaaaaaaaaaaaaaaaaa\"}\n"[..]);
        assert!(matches!(read_msg(&mut big, 10), Err(LanError::Protocol(_))));
        let mut cut = BufReader::new(&b"{\"t\":\"conf"[..]);
        assert!(matches!(read_msg(&mut cut, 100), Err(LanError::Io(_))));
    }

    #[test]
    fn guard_expires_is_single_use_rate_limited_and_strikes_out() {
        let mut g = ListenGuard::new(100, 60);
        assert_eq!(g.allow(100), Ok(()));
        assert_eq!(g.allow(100), Err(Refusal::TooFast));
        assert_eq!(g.allow(101), Ok(()));
        g.consume();
        assert_eq!(g.allow(103), Err(Refusal::Used));
        assert!(g.is_spent(103));
        let mut g = ListenGuard::new(0, 60);
        assert_eq!(g.allow(60), Err(Refusal::Expired));
        let mut g = ListenGuard::new(0, 600);
        for t in 0..5 {
            assert_eq!(g.allow(t * 2), Ok(()));
            g.strike();
        }
        assert_eq!(g.allow(20), Err(Refusal::Strikes));
    }

    /// Drive host and guest over loopback TCP; `guest_says`/`host_says` decide the confirmations.
    fn session(host_says: bool, guest_says: bool) -> (Result<HostOutcome, LanError>, Result<(Welcome, Vec<String>), LanError>, bool, Vec<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut w = s;
            let mut admitted = false;
            let mut seen = Vec::new();
            let out = run_host(
                &mut r,
                &mut w,
                &["SHA256:adminfp".to_owned()],
                &mut |code, _| {
                    seen.push(code.to_owned());
                    host_says
                },
                &mut |_| {
                    admitted = true;
                    Ok(welcome())
                },
            );
            (out, admitted, seen)
        });
        let s = TcpStream::connect(addr).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut w = s;
        let mut guest_codes = Vec::new();
        let g = run_guest(&mut r, &mut w, &card(), &mut |c| {
            guest_codes.push(c.to_owned());
            guest_says
        });
        let (h, admitted, mut seen) = host.join().unwrap();
        seen.extend(guest_codes);
        (h, g, admitted, seen)
    }

    #[test]
    fn both_confirming_admits_and_codes_agree() {
        let (h, g, admitted, codes) = session(true, true);
        assert!(h.is_ok());
        let (w, idents) = g.unwrap();
        assert_eq!(w, welcome());
        assert_eq!(idents, ["SHA256:adminfp"]);
        assert!(admitted);
        assert_eq!(codes.len(), 2);
        assert_eq!(codes[0], codes[1]);
    }

    #[test]
    fn a_host_decline_admits_nothing() {
        let (h, g, admitted, _) = session(false, true);
        assert!(matches!(h, Err(LanError::Declined)));
        assert!(g.is_err());
        assert!(!admitted);
    }

    #[test]
    fn a_guest_decline_admits_nothing() {
        let (h, g, admitted, _) = session(true, false);
        assert!(matches!(h, Err(LanError::PeerAborted(_))));
        assert!(matches!(g, Err(LanError::Declined)));
        assert!(!admitted);
    }

    #[test]
    fn a_tampered_reveal_is_refused_before_any_code_is_shown() {
        // A man in the middle swaps the card after the commitment.
        let mut input = Vec::new();
        write_msg(&mut input, &Msg::Commit { c: commitment(&[9u8; 16], &card().encode()) }).unwrap();
        let mut other = card();
        other.name = "evil".into();
        write_msg(&mut input, &Msg::Reveal { n: hex(&[9u8; 16]), card: other.encode() }).unwrap();
        let mut out = Vec::new();
        let mut asked = false;
        let res = run_host(
            &mut BufReader::new(&input[..]),
            &mut out,
            &["SHA256:a".to_owned()],
            &mut |_, _| {
                asked = true;
                true
            },
            &mut |_| Ok(welcome()),
        );
        assert!(matches!(res, Err(LanError::CommitMismatch)));
        assert!(!asked);
    }

    #[test]
    fn a_swapped_host_identity_gives_a_different_code() {
        let (g, h) = ([4u8; 16], [5u8; 16]);
        let c = card().encode();
        assert_ne!(
            sas(&g, &h, &["SHA256:real".to_owned()], &c),
            sas(&g, &h, &["SHA256:mitm".to_owned()], &c)
        );
    }

    #[test]
    fn a_guest_refuses_a_bad_nonce_or_fingerprint_list() {
        for msg in [
            Msg::Nonce { n: "zz".into(), ident: vec!["a".into()] },
            Msg::Nonce { n: hex(&[1u8; 16]), ident: Vec::new() },
        ] {
            let mut input = Vec::new();
            write_msg(&mut input, &msg).unwrap();
            let mut out = Vec::new();
            let res = run_guest(&mut BufReader::new(&input[..]), &mut out, &card(), &mut |_| true);
            assert!(matches!(res, Err(LanError::Protocol(_))));
        }
    }
}
