//! `goway circle pair-host` / `pair-join`: admit a machine over the local network.
//!
//! The admin listens briefly on a TCP port and prints its address; the new machine connects.
//! The admin's screen shows an eight-digit one-time code that the person on the new
//! machine types (see `goway_circle::lan`, a `CPace` exchange). Only when the codes match and the
//! admin has approved the card does the admin admit it and send the welcome.
//! The listener is single use, expires, and rate limits. Addresses learned here become unsigned
//! resolve hints: the pinned host key is still verified before any hint is trusted.

use std::io::BufReader;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use goway_circle::{
    Card, LanError, ListenGuard, Refusal, new_code, normalize_code, run_guest, run_host,
};

use crate::circle::{self, JoinOut};
use crate::cli::{CircleAdmitArgs, CircleCardArgs, CirclePairHostArgs, CirclePairJoinArgs};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::{self, Renderer};
use crate::state::State;

/// How long a connected peer may stall before it has proved the code (a silent peer must not hold the listener).
const PRE_PROOF_IO_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one session may stall once the peer proved the code (people are comparing codes).
const SESSION_IO_TIMEOUT: Duration = Duration::from_secs(180);
/// How long the new machine waits to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the idle listener checks its guard.
const POLL: Duration = Duration::from_millis(100);

/// What a finished host session produced.
#[derive(Debug)]
pub struct PairedPeer {
    /// The card that was admitted.
    pub card: Card,
    /// Where the new machine connected from.
    pub address: IpAddr,
}

#[allow(clippy::needless_pass_by_value)] // used as a `map_err` callback, which hands the error over by value
fn lan_error(e: LanError) -> Error {
    Error::Usage(e.to_string())
}

/// Open the pairing listener on `bind:port`.
pub fn bind_listener(bind: &str, port: u16) -> Result<TcpListener> {
    let listener = TcpListener::bind((bind, port))
        .map_err(|e| Error::Usage(format!("cannot listen on {bind}:{port}: {e}")))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| Error::Usage(format!("cannot listen on {bind}:{port}: {e}")))?;
    Ok(listener)
}

fn timed(stream: &TcpStream, limit: Duration) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(limit))?;
    stream.set_write_timeout(Some(limit))
}

/// Serve the one pairing on `listener` for at most `ttl` seconds, authenticated by `code`;
/// `confirm` gets the card (whose holder already proved the code) and must return true only
/// after the admin's person approved it.
pub fn serve(
    paths: &Paths,
    renderer: Renderer,
    listener: &TcpListener,
    ttl: u64,
    code: &str,
    circle: Option<&str>,
    confirm: &mut dyn FnMut(&Card) -> bool,
) -> Result<PairedPeer> {
    let ident = circle::admin_ident(paths, renderer, circle)?;
    let started = Instant::now();
    let mut guard = ListenGuard::new(0, ttl);
    loop {
        let now = started.elapsed().as_secs();
        if guard.is_spent(now) {
            tracing::warn!("lan pairing: listener closed without a pairing");
            return Err(Error::Usage(
                "no machine paired before the listener closed; nothing was changed".into(),
            ));
        }
        let (stream, peer) = match listener.accept() {
            Ok(x) => x,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(e) => return Err(Error::Usage(format!("accept failed: {e}"))),
        };
        if let Err(refusal) = guard.allow(now) {
            tracing::warn!(%peer, %refusal, "lan pairing: connection dropped");
            if matches!(refusal, Refusal::TooFast) {
                continue;
            }
            return Err(Error::Usage(format!("{refusal}; nothing was changed")));
        }
        renderer.note(format!("connection from {peer}"));
        match session(paths, renderer, stream, code, &ident, circle, confirm) {
            Ok(card) => {
                guard.consume();
                return Ok(PairedPeer {
                    card,
                    address: peer.ip(),
                });
            }
            Err(e) => {
                tracing::warn!(%peer, error = %e, "lan pairing: session failed");
                renderer.warn(format!("pairing with {peer} failed: {e}"));
                guard.strike();
            }
        }
    }
}

fn session(
    paths: &Paths,
    renderer: Renderer,
    stream: TcpStream,
    code: &str,
    ident: &str,
    circle: Option<&str>,
    confirm: &mut dyn FnMut(&Card) -> bool,
) -> std::result::Result<Card, LanError> {
    timed(&stream, PRE_PROOF_IO_TIMEOUT)?;
    let relax = stream.try_clone()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let out = run_host(
        &mut reader,
        &mut writer,
        code,
        &[ident.to_owned()],
        &mut |card| {
            // The guest proved the code: allow the admin's person time to compare and approve.
            if let Err(e) = timed(&relax, SESSION_IO_TIMEOUT) {
                tracing::warn!(error = %e, "lan pairing: could not extend the session timeout");
            }
            confirm(card)
        },
        &mut |card| {
            let args = CircleAdmitArgs {
                card: card.encode(),
                // The card arrived under the code-keyed confirmation tag, so the code vouches for its key.
                fingerprint: Some(circle::fingerprint_of(&card.key)),
                circle: circle.map(str::to_owned),
            };
            circle::admit(paths, renderer, &args)
                .map(|o| o.welcome)
                .map_err(|e| e.to_string())
        },
    )?;
    Ok(out.card)
}

/// Connect to `address`, pair using the `code` shown on the admin machine, and join the circle.
pub fn pair_join(
    paths: &Paths,
    renderer: Renderer,
    args: &CirclePairJoinArgs,
    code: &str,
) -> Result<(JoinOut, SocketAddr)> {
    let card = circle::make_card(
        paths,
        renderer,
        &CircleCardArgs {
            role: args.role.clone(),
            name: args.name.clone(),
            host_fp: args.host_fp.clone(),
            qr: false,
        },
    )?;
    let addr = args
        .address
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| Error::Usage(format!("`{}` is not an address:port", args.address)))?;
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| Error::Usage(format!("cannot reach {addr}: {e}")))?;
    timed(&stream, SESSION_IO_TIMEOUT).map_err(|e| Error::Usage(e.to_string()))?;
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|e| Error::Usage(e.to_string()))?,
    );
    let mut writer = stream;
    let (welcome, idents) =
        run_guest(&mut reader, &mut writer, code, &card.card).map_err(lan_error)?;
    // The code-keyed exchange authenticated `idents`; they must be admins of the circle the welcome founds.
    let out = circle::join_welcome(paths, renderer, &welcome, &|admin_fps| {
        if idents.iter().all(|i| admin_fps.contains(i)) {
            Ok(())
        } else {
            tracing::warn!("lan pairing: presented admin is not in the welcome");
            Err(Error::Usage(
                "the admin that paired with this machine is not an admin of that circle; nothing was stored"
                    .into(),
            ))
        }
    })?;
    Ok((out, addr))
}

/// Ask the admin's person to approve admitting `card`; false when they decline or cannot answer.
fn approve(renderer: Renderer, card: &Card) -> bool {
    renderer.line(format!("  `{}` asks for {}", card.name, card.roles));
    if let Some(line) = render::ask("admit this machine? [y/N]: ") {
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    } else {
        renderer.warn("cannot ask here; run this in a terminal");
        false
    }
}

/// Ask the new machine's person for the code shown on the admin machine.
fn ask_code(renderer: Renderer) -> Option<String> {
    renderer.next("type the one-time code shown on the admin machine");
    let line = render::ask("pairing code: ")?;
    let code = normalize_code(&line);
    if code.is_none() {
        renderer.warn("the code is eight digits");
    }
    code
}

/// The `host:port` the new machine should dial, when it is known without guessing.
fn advertised(args: &CirclePairHostArgs, port: u16) -> Option<String> {
    let host = match &args.advertise {
        Some(host) => host.clone(),
        None => match args.bind.parse::<IpAddr>() {
            Ok(ip) if !ip.is_unspecified() => ip.to_string(),
            _ => return None,
        },
    };
    Some(join_host_port(&host, port))
}

/// Ask the OS which local address routes outward (UDP connect sends no packets).
fn probe_lan_ip() -> std::io::Result<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.connect("192.0.2.1:9")?;
    Ok(sock.local_addr()?.ip())
}

/// Detect the LAN address through `probe`, rejecting loopback and unspecified results.
fn detect_lan_ip(probe: impl FnOnce() -> std::io::Result<IpAddr>) -> Option<IpAddr> {
    match probe() {
        Ok(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
            tracing::info!(%ip, "detected LAN address");
            Some(ip)
        }
        Ok(ip) => {
            tracing::warn!(%ip, "ignoring unusable detected address");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "LAN address detection failed");
            None
        }
    }
}

/// Format `host` and `port` as a dialable `host:port`, bracketing bare IPv6.
fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Render `text` as an ASCII QR code ('#' dark, ' ' light) with a quiet zone.
pub fn qr_ascii(text: &str) -> Result<String> {
    let code = qrcode::QrCode::new(text.as_bytes())
        .map_err(|e| Error::Usage(format!("cannot make a QR code: {e}")))?;
    tracing::debug!(width = code.width(), "QR code rendered");
    Ok(code
        .render::<char>()
        .dark_color('#')
        .light_color(' ')
        .module_dimensions(2, 1)
        .build())
}

/// Print `text` as a QR code, one renderer line per row so the rows stay aligned.
pub fn print_qr(renderer: Renderer, text: &str) -> Result<()> {
    for row in qr_rows(text)? {
        renderer.line(row);
    }
    Ok(())
}

/// The rows `print_qr` emits for `text`: the ASCII QR code split into lines.
fn qr_rows(text: &str) -> Result<Vec<String>> {
    Ok(qr_ascii(text)?.lines().map(str::to_owned).collect())
}

/// `goway circle pair-host`.
pub fn pair_host_cli(paths: &Paths, renderer: Renderer, args: &CirclePairHostArgs) -> Result<()> {
    let listener = bind_listener(&args.bind, args.port)?;
    let local = listener
        .local_addr()
        .map_err(|e| Error::Usage(e.to_string()))?;
    renderer.headline(format!("waiting {} min for a new machine", args.wait));
    let addr = advertised(args, local.port()).or_else(|| {
        if args.qr {
            detect_lan_ip(probe_lan_ip).map(|ip| join_host_port(&ip.to_string(), local.port()))
        } else {
            None
        }
    });
    match addr {
        Some(addr) => {
            renderer.next(format!(
                "on the new machine run `goway circle pair-join {addr}`"
            ));
            if args.qr {
                print_qr(renderer, &addr)?;
            }
        }
        None if args.qr => {
            return Err(Error::Usage(
                "--qr could not detect the LAN address; pass --advertise ADDRESS".to_owned(),
            ));
        }
        None => renderer.next(format!(
            "on the new machine run `goway circle pair-join <this machine's LAN address>:{}`",
            local.port()
        )),
    }
    let code = new_code().map_err(lan_error)?;
    renderer.headline(format!("one-time pairing code: {code}"));
    let peer = serve(
        paths,
        renderer,
        &listener,
        args.wait * 60,
        &code,
        args.circle.as_deref(),
        &mut |card| approve(renderer, card),
    )?;
    renderer.ok(format!("admitted `{}`", peer.card.name));
    remember_hint(paths, &peer.card.name, peer.address);
    Ok(())
}

/// `goway circle pair-join`.
pub fn pair_join_cli(paths: &Paths, renderer: Renderer, args: &CirclePairJoinArgs) -> Result<()> {
    let code = ask_code(renderer).ok_or_else(|| {
        Error::Usage("no valid pairing code was entered; nothing was changed".into())
    })?;
    let (out, addr) = pair_join(paths, renderer, args, &code)?;
    renderer.ok(format!(
        "joined circle {} ({} members)",
        out.id, out.members
    ));
    if let Some(peer) = &args.peer {
        remember_hint(paths, peer, addr.ip());
    }
    Ok(())
}

/// Record `address` as an unsigned resolve hint for `name`; a failure only loses the hint.
fn remember_hint(paths: &Paths, name: &str, address: IpAddr) {
    let file = paths.state_file();
    let saved = State::load(&file).and_then(|mut s| {
        s.add_hint(name, &address.to_string());
        s.save(&file)
    });
    if let Err(e) = saved {
        tracing::warn!(error = %e, "could not save the peer address hint");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{CircleInitArgs, CircleSelect};
    use crate::render::ColorWhen;
    use std::path::Path;

    #[test]
    fn qr_ascii_is_ascii_square_with_quiet_zone() {
        let qr = qr_ascii("192.168.1.20:41234").unwrap();
        assert!(qr.is_ascii());
        assert!(qr.contains('#'));
        let rows: Vec<&str> = qr.lines().collect();
        assert!(rows.len() >= 21);
        assert!(rows[0].trim().is_empty());
    }

    #[test]
    fn print_qr_rows_are_aligned_ascii_and_print_succeeds() {
        let text = "192.168.1.20:41234";
        let rows = qr_rows(text).unwrap();
        assert!(rows.len() >= 21);
        assert!(rows.iter().all(|r| r.is_ascii()));
        let width = rows[0].len();
        assert!(rows.iter().all(|r| r.len() == width));
        assert!(rows.iter().any(|r| r.contains('#')));
        assert!(print_qr(Renderer::new(ColorWhen::Never), text).is_ok());
    }

    #[test]
    fn print_qr_rejects_text_too_long_for_a_qr_code() {
        let err = print_qr(Renderer::new(ColorWhen::Never), &"x".repeat(5000)).unwrap_err();
        assert!(matches!(err, Error::Usage(_)));
    }

    #[test]
    fn qr_ascii_encodes_a_full_card_length_text() {
        assert!(qr_ascii(&"gwc1.".repeat(40)).is_ok());
    }

    // frob:tests crates/goway/src/lanpair.rs::detect_lan_ip
    #[test]
    fn detect_lan_ip_accepts_lan_and_rejects_loopback_or_failure() {
        let lan: IpAddr = "192.168.1.7".parse().unwrap();
        assert_eq!(detect_lan_ip(|| Ok(lan)), Some(lan));
        assert_eq!(detect_lan_ip(|| Ok("127.0.0.1".parse().unwrap())), None);
        assert_eq!(detect_lan_ip(|| Ok("0.0.0.0".parse().unwrap())), None);
        let err = std::io::Error::other("no route");
        assert_eq!(detect_lan_ip(|| Err(err)), None);
        assert_eq!(join_host_port(&lan.to_string(), 5), "192.168.1.7:5");
    }

    #[test]
    fn advertised_prefers_flag_then_specific_bind() {
        let mk = |bind: &str, adv: Option<&str>| CirclePairHostArgs {
            port: 0,
            bind: bind.to_owned(),
            wait: 5,
            circle: None,
            advertise: adv.map(str::to_owned),
            qr: true,
        };
        assert_eq!(advertised(&mk("0.0.0.0", None), 9), None);
        assert_eq!(
            advertised(&mk("192.168.1.2", None), 9).as_deref(),
            Some("192.168.1.2:9")
        );
        assert_eq!(
            advertised(&mk("0.0.0.0", Some("10.0.0.5")), 9).as_deref(),
            Some("10.0.0.5:9")
        );
        assert_eq!(
            advertised(&mk("0.0.0.0", Some("fe80::1")), 9).as_deref(),
            Some("[fe80::1]:9")
        );
    }

    fn paths(dir: &Path) -> Paths {
        Paths {
            config_dir: dir.join("config"),
            state_dir: dir.join("state"),
            runtime_dir: None,
        }
    }

    fn quiet() -> Renderer {
        Renderer::new(ColorWhen::Never)
    }

    fn join_args(addr: SocketAddr) -> CirclePairJoinArgs {
        CirclePairJoinArgs {
            address: addr.to_string(),
            role: "helper".into(),
            name: Some("newbox".into()),
            host_fp: Some("SHA256:abc".into()),
            peer: None,
        }
    }

    fn members(p: &Paths) -> Option<usize> {
        circle::list_rows(p, quiet(), &CircleSelect { circle: None })
            .ok()
            .map(|r| r.len())
    }

    /// An admin machine with a circle, a fresh joiner, and a loopback listener.
    struct Rig {
        _tmp: tempfile::TempDir,
        admin: Paths,
        joiner: Paths,
        listener: TcpListener,
    }

    fn rig() -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let admin = paths(&tmp.path().join("a"));
        let joiner = paths(&tmp.path().join("b"));
        circle::init(
            &admin,
            quiet(),
            &CircleInitArgs {
                name: "home".into(),
                machine: Some("lap".into()),
                passphrase: false,
            },
        )
        .unwrap();
        Rig {
            _tmp: tmp,
            admin,
            joiner,
            listener: bind_listener("127.0.0.1", 0).unwrap(),
        }
    }

    /// Run host and guest at once; the host approves per `host_says`, the guest types `guest_code`.
    fn pair(
        rig: &Rig,
        host_says: bool,
        guest_code: &str,
    ) -> (Result<PairedPeer>, Result<(JoinOut, SocketAddr)>) {
        let addr = rig.listener.local_addr().unwrap();
        std::thread::scope(|sc| {
            let host = sc.spawn(|| {
                serve(
                    &rig.admin,
                    quiet(),
                    &rig.listener,
                    2,
                    "1234 5678",
                    None,
                    &mut |_| host_says,
                )
            });
            let guest = sc.spawn(|| pair_join(&rig.joiner, quiet(), &join_args(addr), guest_code));
            (host.join().unwrap(), guest.join().unwrap())
        })
    }

    // frob:tests crates/goway/src/lanpair.rs::pair_join
    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn the_right_code_and_an_approving_admin_admit_the_machine_over_loopback() {
        let rig = rig();
        let before = members(&rig.admin).unwrap();
        let (host, guest) = pair(&rig, true, "1234 5678");
        let peer = host.unwrap();
        assert_eq!(peer.card.name, "newbox");
        assert!(peer.address.is_loopback());
        let (out, _) = guest.unwrap();
        assert_eq!(out.members, 3);
        assert_eq!(members(&rig.admin), Some(before + 1));
        assert!(members(&rig.joiner).is_some());
    }

    // frob:tests crates/goway/src/lanpair.rs::pair_join
    #[test]
    fn a_wrong_code_changes_nothing_anywhere() {
        let rig = rig();
        let before = members(&rig.admin);
        let (host, guest) = pair(&rig, true, "1234 5679");
        assert!(guest.is_err());
        assert!(
            host.is_err(),
            "the host session fails and the listener times out or strikes"
        );
        assert_eq!(members(&rig.admin), before);
        assert_eq!(members(&rig.joiner), None);
    }

    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn an_admin_who_does_not_approve_changes_nothing_anywhere() {
        let rig = rig();
        let before = members(&rig.admin);
        let (host, guest) = pair(&rig, false, "1234 5678");
        assert!(guest.is_err());
        assert!(host.is_err());
        assert_eq!(members(&rig.admin), before);
        assert_eq!(members(&rig.joiner), None);
    }

    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn an_expired_listener_admits_nobody() {
        let rig = rig();
        let before = members(&rig.admin);
        let r = serve(
            &rig.admin,
            quiet(),
            &rig.listener,
            0,
            "1234 5678",
            None,
            &mut |_| true,
        );
        assert!(r.is_err());
        assert_eq!(members(&rig.admin), before);
    }

    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn a_machine_that_is_not_an_admin_cannot_host() {
        let rig = rig();
        let r = serve(
            &rig.joiner,
            quiet(),
            &rig.listener,
            5,
            "1234 5678",
            None,
            &mut |_| true,
        );
        assert!(r.is_err());
    }

    // frob:tests crates/goway/src/lanpair.rs::remember_hint
    #[test]
    fn a_paired_address_is_remembered_as_a_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        remember_hint(&p, "newbox", "192.0.2.9".parse().unwrap());
        assert_eq!(
            State::load(&p.state_file()).unwrap().hints("newbox"),
            ["192.0.2.9"]
        );
    }
}
