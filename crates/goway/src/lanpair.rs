//! `goway circle pair-host` / `pair-join`: admit a machine over the local network.
//!
//! The admin listens briefly on a TCP port and prints its address; the new machine connects. Both
//! screens show a six-digit code (see `goway_circle::lan`); each person types the code shown on
//! the OTHER screen. Only when both match does the admin admit the card and send the welcome.
//! The listener is single use, expires, and rate limits. Addresses learned here become unsigned
//! resolve hints: the pinned host key is still verified before any hint is trusted.

use std::io::BufReader;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use goway_circle::{Card, LanError, ListenGuard, Refusal, run_guest, run_host, sas_matches};

use crate::circle::{self, JoinOut};
use crate::cli::{CircleAdmitArgs, CircleCardArgs, CirclePairHostArgs, CirclePairJoinArgs};
use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::render::{self, Renderer};
use crate::state::State;

/// How long one connected session may stall on a read or write (people are comparing codes).
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

fn timed(stream: &TcpStream) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(SESSION_IO_TIMEOUT))?;
    stream.set_write_timeout(Some(SESSION_IO_TIMEOUT))
}

/// Serve the one pairing on `listener` for at most `ttl` seconds; `confirm` gets the code and the
/// card and must return true only after a person matched the code shown on the new machine.
pub fn serve(
    paths: &Paths,
    renderer: Renderer,
    listener: &TcpListener,
    ttl: u64,
    circle: Option<&str>,
    confirm: &mut dyn FnMut(&str, &Card) -> bool,
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
        match session(paths, renderer, stream, &ident, circle, confirm) {
            Ok(card) => {
                guard.consume();
                return Ok(PairedPeer { card, address: peer.ip() });
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
    ident: &str,
    circle: Option<&str>,
    confirm: &mut dyn FnMut(&str, &Card) -> bool,
) -> std::result::Result<Card, LanError> {
    timed(&stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let out = run_host(
        &mut reader,
        &mut writer,
        &[ident.to_owned()],
        confirm,
        &mut |card| {
            let args = CircleAdmitArgs {
                card: card.encode(),
                // The card was bound into the confirmed code, so the code confirms its key.
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

/// Connect to `address`, pair, and join the circle; `confirm` gets the code and must return true
/// only after a person matched the code shown on the admin machine.
pub fn pair_join(
    paths: &Paths,
    renderer: Renderer,
    args: &CirclePairJoinArgs,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<(JoinOut, SocketAddr)> {
    let card = circle::make_card(
        paths,
        renderer,
        &CircleCardArgs {
            role: args.role.clone(),
            name: args.name.clone(),
            host_fp: args.host_fp.clone(),
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
    timed(&stream).map_err(|e| Error::Usage(e.to_string()))?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| Error::Usage(e.to_string()))?);
    let mut writer = stream;
    let (welcome, idents) =
        run_guest(&mut reader, &mut writer, &card.card, confirm).map_err(lan_error)?;
    // The code authenticated `idents`; they must be admins of the circle the welcome founds.
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

/// Ask a person to type the code shown on the other machine; false when they cannot or differ.
fn typed_matches(renderer: Renderer, code: &str) -> bool {
    renderer.headline(format!("confirmation code on this screen: {code}"));
    renderer.next("each person types the code shown on the OTHER machine's screen");
    match render::ask("code from the other machine: ") {
        Some(line) => sas_matches(code, &line),
        None => {
            renderer.warn("cannot ask here; run this in a terminal");
            false
        }
    }
}

/// `goway circle pair-host`.
pub fn pair_host_cli(paths: &Paths, renderer: Renderer, args: &CirclePairHostArgs) -> Result<()> {
    let listener = bind_listener(&args.bind, args.port)?;
    let local = listener.local_addr().map_err(|e| Error::Usage(e.to_string()))?;
    renderer.headline(format!("waiting {} min for a new machine", args.wait));
    renderer.next(format!("on the new machine run `goway circle pair-join <this machine's LAN address>:{}`", local.port()));
    let peer = serve(
        paths,
        renderer,
        &listener,
        args.wait * 60,
        args.circle.as_deref(),
        &mut |code, card| {
            renderer.line(format!("  `{}` asks for {}", card.name, card.roles));
            typed_matches(renderer, code)
        },
    )?;
    renderer.ok(format!("admitted `{}`", peer.card.name));
    remember_hint(paths, &peer.card.name, peer.address);
    Ok(())
}

/// `goway circle pair-join`.
pub fn pair_join_cli(paths: &Paths, renderer: Renderer, args: &CirclePairJoinArgs) -> Result<()> {
    let (out, addr) = pair_join(paths, renderer, args, &mut |code| typed_matches(renderer, code))?;
    renderer.ok(format!("joined circle {} ({} members)", out.id, out.members));
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
    use std::sync::Mutex;

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
        circle::list_rows(p, quiet(), &CircleSelect { circle: None }).ok().map(|r| r.len())
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
            &CircleInitArgs { name: "home".into(), machine: Some("lap".into()) },
        )
        .unwrap();
        Rig { _tmp: tmp, admin, joiner, listener: bind_listener("127.0.0.1", 0).unwrap() }
    }

    /// Run host and guest at once; each side's confirm returns what the closure says.
    fn pair(
        rig: &Rig,
        host_says: bool,
        guest_says: bool,
    ) -> (Result<PairedPeer>, Result<(JoinOut, SocketAddr)>, Vec<String>, Vec<String>) {
        let addr = rig.listener.local_addr().unwrap();
        let host_codes = Mutex::new(Vec::new());
        let guest_codes = Mutex::new(Vec::new());
        let (h, g) = std::thread::scope(|sc| {
            let host = sc.spawn(|| {
                serve(&rig.admin, quiet(), &rig.listener, 2, None, &mut |code, _| {
                    host_codes.lock().unwrap().push(code.to_owned());
                    host_says
                })
            });
            let guest = sc.spawn(|| {
                pair_join(&rig.joiner, quiet(), &join_args(addr), &mut |code| {
                    guest_codes.lock().unwrap().push(code.to_owned());
                    guest_says
                })
            });
            (host.join().unwrap(), guest.join().unwrap())
        });
        (h, g, host_codes.into_inner().unwrap(), guest_codes.into_inner().unwrap())
    }

    // frob:tests crates/goway/src/lanpair.rs::pair_join
    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn both_people_confirming_admits_the_machine_over_loopback() {
        let rig = rig();
        let before = members(&rig.admin).unwrap();
        let (host, guest, hc, gc) = pair(&rig, true, true);
        let peer = host.unwrap();
        assert_eq!(peer.card.name, "newbox");
        assert!(peer.address.is_loopback());
        let (out, _) = guest.unwrap();
        assert_eq!(out.members, 3);
        assert_eq!(hc, gc, "both screens show the same code");
        assert_eq!(members(&rig.admin), Some(before + 1));
        assert!(members(&rig.joiner).is_some());
    }

    // frob:tests crates/goway/src/lanpair.rs::pair_join
    #[test]
    fn a_guest_who_does_not_confirm_changes_nothing_anywhere() {
        let rig = rig();
        let before = members(&rig.admin);
        let (host, guest, ..) = pair(&rig, true, false);
        assert!(guest.is_err());
        assert!(host.is_err(), "the host session fails and the listener times out or strikes");
        assert_eq!(members(&rig.admin), before);
        assert_eq!(members(&rig.joiner), None);
    }

    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn a_host_who_does_not_confirm_changes_nothing_anywhere() {
        let rig = rig();
        let before = members(&rig.admin);
        let (host, guest, ..) = pair(&rig, false, true);
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
        let r = serve(&rig.admin, quiet(), &rig.listener, 0, None, &mut |_, _| true);
        assert!(r.is_err());
        assert_eq!(members(&rig.admin), before);
    }

    // frob:tests crates/goway/src/lanpair.rs::serve
    #[test]
    fn a_machine_that_is_not_an_admin_cannot_host() {
        let rig = rig();
        let r = serve(&rig.joiner, quiet(), &rig.listener, 5, None, &mut |_, _| true);
        assert!(r.is_err());
    }

    // frob:tests crates/goway/src/lanpair.rs::remember_hint
    #[test]
    fn a_paired_address_is_remembered_as_a_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        remember_hint(&p, "newbox", "192.0.2.9".parse().unwrap());
        assert_eq!(State::load(&p.state_file()).unwrap().hints("newbox"), ["192.0.2.9"]);
    }
}
