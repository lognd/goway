//! LAN pairing: a `CPace` password-authenticated key exchange over ristretto255, then the card
//! and welcome over one TCP stream.
//!
//! The admin's screen shows a short one-time code; the person on the new machine types it. Both
//! sides feed it into `CPace` (a balanced PAKE): the code never crosses the wire, a man in the
//! middle gets exactly one guess per session and learns nothing to test offline, and a session
//! ends in a shared key only when the codes were equal. Key-confirmation tags under that key
//! then bind the host's presented fingerprints and the guest's card, so neither can be swapped
//! in flight. Nothing changes on either machine until the host's person has also approved the
//! card.
//!
//! Flow: host `pake` -> guest `pake` -> host `proof` (fingerprints + tag) -> guest `card`
//! (card + tag) -> host person approves -> host admits and sends `welcome`.

use std::io::{BufRead, Read, Write};

use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::IsIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tracing::{debug, info, warn};

use crate::pairing::{Card, MAX_CARD_CHARS, MAX_WELCOME_CHARS, Welcome};

/// Bytes of the host's random session id.
const SID_BYTES: usize = 16;
/// Digits in the one-time pairing code.
const CODE_DIGITS: u32 = 8;
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
    /// The key confirmation failed: the codes differed or the exchange was tampered with.
    #[error("the pairing code did not match (or the exchange was tampered with); pairing refused")]
    WrongCode,
    /// The local person declined; nothing was changed.
    #[error("the pairing was not approved here; nothing was changed")]
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
    /// Host: its session id and public point; guest: the echoed id and its point.
    Pake {
        /// Hex of the random session id part.
        s: String,
        /// Hex of the compressed ristretto255 point.
        y: String,
    },
    /// Host: the admin key fingerprints it vouches with, under a key-confirmation tag.
    Proof {
        /// Admin key fingerprints of the host.
        ident: Vec<String>,
        /// Hex of the host confirmation tag.
        tag: String,
    },
    /// Guest: its card under a key-confirmation tag.
    Card {
        /// The `gwc1.` card.
        card: String,
        /// Hex of the guest confirmation tag.
        tag: String,
    },
    /// Host: the `gww1.` welcome, sent only after approval.
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

fn random<const N: usize>() -> Result<[u8; N], LanError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| LanError::Random(e.to_string()))?;
    Ok(b)
}

fn feed<D: Digest>(h: &mut D, part: &[u8]) {
    h.update((part.len() as u64).to_be_bytes());
    h.update(part);
}

/// A fresh one-time pairing code, shown as "1234 5678".
pub fn new_code() -> Result<String, LanError> {
    let n = u64::from_be_bytes(random::<8>()?) % 10u64.pow(CODE_DIGITS);
    let width = CODE_DIGITS as usize;
    let digits = format!("{n:0width$}");
    Ok(format!("{} {}", &digits[..4], &digits[4..]))
}

/// The digits of a typed code when it is well formed (spaces and dashes ignored), else `None`.
pub fn normalize_code(typed: &str) -> Option<String> {
    let ok = typed
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '-');
    let digits: String = typed.chars().filter(char::is_ascii_digit).collect();
    (ok && digits.len() == CODE_DIGITS as usize).then_some(digits)
}

/// One side's `CPace` state: the secret scalar and the code digits.
struct Pake {
    secret: Scalar,
    code: String,
}

impl Pake {
    /// Start with a fresh secret scalar for the typed or displayed `code`.
    fn new(code: &str) -> Result<Self, LanError> {
        let digits = normalize_code(code).ok_or_else(|| protocol("malformed pairing code"))?;
        let secret = Scalar::from_bytes_mod_order_wide(&random::<64>()?);
        Ok(Self {
            secret,
            code: digits,
        })
    }

    /// Our public point over the generator derived from the code and the host's session id.
    fn publish(&self, sid: &[u8]) -> RistrettoPoint {
        let mut h = Sha512::new();
        feed(&mut h, b"goway-cpace-r255-gen-v1");
        feed(&mut h, self.code.as_bytes());
        feed(&mut h, sid);
        RistrettoPoint::from_uniform_bytes(&h.finalize().into()) * self.secret
    }

    /// The shared key from the peer's point; both sides hash the points guest first.
    fn finish(
        &self,
        sid: &[u8],
        peer: &RistrettoPoint,
        guest_y: &RistrettoPoint,
        host_y: &RistrettoPoint,
    ) -> Result<[u8; 32], LanError> {
        let k = peer * self.secret;
        if k.is_identity() {
            return Err(protocol("degenerate key share"));
        }
        let mut h = Sha256::new();
        feed(&mut h, b"goway-cpace-r255-isk-v1");
        feed(&mut h, sid);
        feed(&mut h, k.compress().as_bytes());
        feed(&mut h, guest_y.compress().as_bytes());
        feed(&mut h, host_y.compress().as_bytes());
        Ok(h.finalize().into())
    }
}

fn enc(p: &RistrettoPoint) -> String {
    hex(p.compress().as_bytes())
}

fn point(text: &str) -> Result<RistrettoPoint, LanError> {
    let bytes = unhex(text).ok_or_else(|| protocol("bad point encoding"))?;
    let p = CompressedRistretto::from_slice(&bytes)
        .map_err(|_| protocol("bad point length"))?
        .decompress()
        .ok_or_else(|| protocol("point is not on the group"))?;
    if p.is_identity() {
        return Err(protocol("degenerate public point"));
    }
    Ok(p)
}

fn sid_of(text: &str) -> Result<[u8; SID_BYTES], LanError> {
    unhex(text)
        .and_then(|v| <[u8; SID_BYTES]>::try_from(v).ok())
        .ok_or_else(|| protocol("bad session id"))
}

/// A key-confirmation tag over `role` and `payload` under the shared key.
fn tag(isk: &[u8; 32], role: &[u8], payload: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    feed(&mut h, b"goway-cpace-confirm-v1");
    feed(&mut h, isk);
    feed(&mut h, role);
    for p in payload {
        feed(&mut h, p);
    }
    hex(&h.finalize())
}

fn idents_blob(idents: &[String]) -> Vec<u8> {
    idents.join("\n").into_bytes()
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
    r.by_ref()
        .take(max as u64 + 1)
        .read_until(b'\n', &mut buf)?;
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
    reason
        .chars()
        .filter(char::is_ascii_graphic)
        .take(MAX_REASON)
        .collect()
}

fn check_idents(idents: &[String]) -> Result<(), LanError> {
    let ok = !idents.is_empty()
        && idents.len() <= MAX_IDENTS
        && idents
            .iter()
            .all(|i| !i.is_empty() && i.len() <= MAX_IDENT_LEN && i.is_ascii());
    ok.then_some(())
        .ok_or_else(|| protocol("bad fingerprint list"))
}

/// What the host's session produced.
#[derive(Debug)]
pub struct HostOutcome {
    /// The card that was admitted.
    pub card: Card,
}

/// Run the host (admin) side on `reader`/`writer` with the one-time `code` it displayed:
/// `confirm` is asked to approve the card (after the guest proved it knew the code) and `admit`
/// runs only after approval, returning the welcome to send.
pub fn run_host<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    code: &str,
    idents: &[String],
    confirm: &mut dyn FnMut(&Card) -> bool,
    admit: &mut dyn FnMut(&Card) -> Result<Welcome, String>,
) -> Result<HostOutcome, LanError> {
    check_idents(idents)?;
    let pake = Pake::new(code)?;
    let sid = random::<SID_BYTES>()?;
    let host_y = pake.publish(&sid);
    write_msg(
        writer,
        &Msg::Pake {
            s: hex(&sid),
            y: enc(&host_y),
        },
    )?;
    let Msg::Pake { s, y } = read_msg(reader, MAX_SMALL_MSG)? else {
        return Err(protocol("expected pake"));
    };
    if sid_of(&s)? != sid {
        return Err(protocol("session id mismatch"));
    }
    let guest_y = point(&y)?;
    let isk = pake.finish(&sid, &guest_y, &guest_y, &host_y)?;
    let blob = idents_blob(idents);
    write_msg(
        writer,
        &Msg::Proof {
            ident: idents.to_vec(),
            tag: tag(&isk, b"host", &[&blob]),
        },
    )?;
    let Msg::Card {
        card: card_text,
        tag: guest_tag,
    } = read_msg(reader, MAX_SMALL_MSG)?
    else {
        return Err(protocol("expected card"));
    };
    if tag(&isk, b"guest", &[&blob, card_text.as_bytes()]) != guest_tag {
        warn!("lan pairing: guest confirmation tag mismatch (wrong code?)");
        let _ = write_msg(
            writer,
            &Msg::Abort {
                reason: "pairing code mismatch".into(),
            },
        );
        return Err(LanError::WrongCode);
    }
    let card = Card::decode(&card_text).map_err(|e| protocol(e.to_string()))?;
    debug!(name = %card.name, "lan pairing: guest proved the code");
    if !confirm(&card) {
        warn!("lan pairing: host person declined");
        let _ = write_msg(
            writer,
            &Msg::Abort {
                reason: "declined on the admin machine".into(),
            },
        );
        return Err(LanError::Declined);
    }
    let welcome = admit(&card).map_err(|e| {
        let _ = write_msg(
            writer,
            &Msg::Abort {
                reason: abort_text(&e),
            },
        );
        LanError::Admit(e)
    })?;
    write_msg(
        writer,
        &Msg::Welcome {
            welcome: welcome.encode(),
        },
    )?;
    info!(name = %card.name, "lan pairing: welcome sent");
    Ok(HostOutcome { card })
}

/// Run the guest (new machine) side with the `code` its person typed. Returns the welcome and
/// the fingerprints the host presented (authenticated by the code), which the caller must check
/// against the admins in the welcome.
pub fn run_guest<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    code: &str,
    card: &Card,
) -> Result<(Welcome, Vec<String>), LanError> {
    let card_text = card.encode();
    let pake = Pake::new(code)?;
    let Msg::Pake { s, y } = read_msg(reader, MAX_SMALL_MSG)? else {
        return Err(protocol("expected pake"));
    };
    let sid = sid_of(&s)?;
    let host_y = point(&y)?;
    let guest_y = pake.publish(&sid);
    write_msg(
        writer,
        &Msg::Pake {
            s,
            y: enc(&guest_y),
        },
    )?;
    let isk = pake.finish(&sid, &host_y, &guest_y, &host_y)?;
    let Msg::Proof {
        ident,
        tag: host_tag,
    } = read_msg(reader, MAX_SMALL_MSG)?
    else {
        return Err(protocol("expected proof"));
    };
    check_idents(&ident)?;
    let blob = idents_blob(&ident);
    if tag(&isk, b"host", &[&blob]) != host_tag {
        warn!("lan pairing: host confirmation tag mismatch (wrong code?)");
        let _ = write_msg(
            writer,
            &Msg::Abort {
                reason: "pairing code mismatch".into(),
            },
        );
        return Err(LanError::WrongCode);
    }
    write_msg(
        writer,
        &Msg::Card {
            card: card_text.clone(),
            tag: tag(&isk, b"guest", &[&blob, card_text.as_bytes()]),
        },
    )?;
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
        Self {
            expires: now + ttl,
            min_gap: 1,
            max_strikes: 5,
            last: None,
            strikes: 0,
            used: false,
        }
    }

    /// Decide whether a connection at `now` may be served.
    pub fn allow(&mut self, now: u64) -> Result<(), Refusal> {
        let verdict = if now >= self.expires {
            Err(Refusal::Expired)
        } else if self.used {
            Err(Refusal::Used)
        } else if self.strikes >= self.max_strikes {
            Err(Refusal::Strikes)
        } else if self
            .last
            .is_some_and(|l| now.saturating_sub(l) < self.min_gap)
        {
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
        Welcome {
            v: 1,
            circle: "home".into(),
            genesis: genesis.id(),
            ops: vec![genesis],
        }
    }

    #[test]
    fn codes_are_eight_digits_and_normalize() {
        let c = new_code().unwrap();
        assert_eq!(c.len(), 9);
        assert_eq!(normalize_code(&c).unwrap().len(), 8);
        assert_eq!(normalize_code(" 1234-5678\n").as_deref(), Some("12345678"));
        assert_eq!(normalize_code("1234567"), None);
        assert_eq!(normalize_code("12345678a"), None);
        assert_eq!(normalize_code("y"), None);
    }

    #[test]
    fn messages_refuse_unknown_fields_and_oversize() {
        let mut ok = BufReader::new(&b"{\"t\":\"abort\",\"reason\":\"x\"}\n"[..]);
        assert_eq!(
            read_msg(&mut ok, 100).unwrap(),
            Msg::Abort { reason: "x".into() }
        );
        let mut extra = BufReader::new(&b"{\"t\":\"abort\",\"reason\":\"x\",\"x\":1}\n"[..]);
        assert!(read_msg(&mut extra, 100).is_err());
        let mut big =
            BufReader::new(&b"{\"t\":\"abort\",\"reason\":\"aaaaaaaaaaaaaaaaaaaa\"}\n"[..]);
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

    type Run = (
        Result<HostOutcome, LanError>,
        Result<(Welcome, Vec<String>), LanError>,
        bool,
    );

    /// Drive host and guest over loopback TCP with the given codes and host approval.
    #[allow(clippy::many_single_char_names)] // a, r, w and the like are the conventional names for a test session
    fn session(host_code: &str, guest_code: &str, host_says: bool) -> Run {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host_code = host_code.to_owned();
        let host = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut w = s;
            let mut admitted = false;
            let out = run_host(
                &mut r,
                &mut w,
                &host_code,
                &["SHA256:adminfp".to_owned()],
                &mut |_| host_says,
                &mut |_| {
                    admitted = true;
                    Ok(welcome())
                },
            );
            (out, admitted)
        });
        let s = TcpStream::connect(addr).unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut w = s;
        let g = run_guest(&mut r, &mut w, guest_code, &card());
        let (h, admitted) = host.join().unwrap();
        (h, g, admitted)
    }

    #[test]
    fn equal_codes_admit_and_deliver_the_welcome() {
        let (h, g, admitted) = session("1234 5678", "12345678", true);
        assert!(h.is_ok());
        let (w, idents) = g.unwrap();
        assert_eq!(w, welcome());
        assert_eq!(idents, ["SHA256:adminfp"]);
        assert!(admitted);
    }

    #[test]
    fn a_wrong_code_fails_both_sides_and_admits_nothing() {
        let (h, g, admitted) = session("1234 5678", "1234 5679", true);
        assert!(matches!(g, Err(LanError::WrongCode)));
        assert!(h.is_err());
        assert!(!admitted);
    }

    #[test]
    fn a_host_decline_admits_nothing() {
        let (h, g, admitted) = session("1234 5678", "1234 5678", false);
        assert!(matches!(h, Err(LanError::Declined)));
        assert!(g.is_err());
        assert!(!admitted);
    }

    #[test]
    fn a_swapped_card_after_the_exchange_is_refused() {
        // A guest tag over one card does not authenticate another.
        let isk = [7u8; 32];
        let blob = idents_blob(&["SHA256:a".to_owned()]);
        assert_ne!(
            tag(&isk, b"guest", &[&blob, b"card-a"]),
            tag(&isk, b"guest", &[&blob, b"card-b"])
        );
        assert_ne!(
            tag(&isk, b"guest", &[&blob, b"c"]),
            tag(&isk, b"host", &[&blob, b"c"])
        );
    }

    #[test]
    fn degenerate_and_malformed_points_are_rejected() {
        let identity = hex(&[0u8; 32]);
        assert!(point(&identity).is_err());
        assert!(point("zz").is_err());
        assert!(point(&hex(&[0xffu8; 32])).is_err());
        assert!(point(&hex(&[1u8; 5])).is_err());
    }

    #[test]
    fn a_guest_refuses_a_bad_session_id_or_point() {
        for msg in [
            Msg::Pake {
                s: "zz".into(),
                y: hex(&[0u8; 32]),
            },
            Msg::Pake {
                s: hex(&[1u8; SID_BYTES]),
                y: hex(&[0u8; 32]),
            },
        ] {
            let mut input = Vec::new();
            write_msg(&mut input, &msg).unwrap();
            let mut out = Vec::new();
            let res = run_guest(
                &mut BufReader::new(&input[..]),
                &mut out,
                "12345678",
                &card(),
            );
            assert!(matches!(res, Err(LanError::Protocol(_))));
        }
    }
}
