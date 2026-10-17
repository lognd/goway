//! OS keystore integration: remember a key passphrase in the platform secret store so the
//! user is prompted once. The store is a trait so tests use [`FakeKeystore`], never a real one.

use std::collections::HashMap;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::error::{CryptoError, KeystoreError};
use crate::passphrase::{Passphrase, PassphraseSource, passphrase_unlocks_key};

/// The service name every goway-circle secret is filed under.
const SERVICE: &str = "goway-circle";

/// A secret store keyed by account (normally the key file path).
pub trait Keystore {
    /// The passphrase saved for `account`, or `None` when nothing is saved.
    fn get(&self, account: &str) -> Result<Option<Passphrase>, KeystoreError>;
    /// Save `pass` for `account`, replacing any earlier one.
    fn set(&self, account: &str, pass: &Passphrase) -> Result<(), KeystoreError>;
    /// Forget the passphrase for `account`; forgetting nothing is fine.
    fn delete(&self, account: &str) -> Result<(), KeystoreError>;
}

/// An in-memory keystore for tests; it never touches the machine's secret store.
#[derive(Debug, Default)]
pub struct FakeKeystore {
    items: Mutex<HashMap<String, Passphrase>>,
}

impl FakeKeystore {
    /// An empty fake keystore.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Passphrase>> {
        self.items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Keystore for FakeKeystore {
    fn get(&self, account: &str) -> Result<Option<Passphrase>, KeystoreError> {
        Ok(self.lock().get(account).cloned())
    }

    fn set(&self, account: &str, pass: &Passphrase) -> Result<(), KeystoreError> {
        self.lock().insert(account.to_owned(), pass.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), KeystoreError> {
        self.lock().remove(account);
        Ok(())
    }
}

// The freedesktop Secret Service (GNOME Keyring, KWallet) via the `secret-tool` command.
///
/// The secret travels on the child's stdin, never argv. Other platforms report `Unavailable`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SecretToolKeystore;

impl Keystore for SecretToolKeystore {
    fn get(&self, account: &str) -> Result<Option<Passphrase>, KeystoreError> {
        ProgramKeystore::new("secret-tool").get(account)
    }

    fn set(&self, account: &str, pass: &Passphrase) -> Result<(), KeystoreError> {
        ProgramKeystore::new("secret-tool").set(account, pass)
    }

    fn delete(&self, account: &str) -> Result<(), KeystoreError> {
        ProgramKeystore::new("secret-tool").delete(account)
    }
}

/// The `secret-tool` protocol against a configurable program, so tests can use a fake.
#[derive(Debug, Clone)]
struct ProgramKeystore {
    program: std::ffi::OsString,
}

impl ProgramKeystore {
    fn new(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }

    fn base(&self, verb: &str, account: &str) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.arg(verb);
        if verb == "store" {
            cmd.args(["--label", "goway circle key passphrase"]);
        }
        cmd.args(["service", SERVICE, "account", account]);
        cmd
    }

    fn spawn_err(e: &std::io::Error) -> KeystoreError {
        KeystoreError::Unavailable(format!("cannot run secret-tool: {e}"))
    }
}

impl Keystore for ProgramKeystore {
    fn get(&self, account: &str) -> Result<Option<Passphrase>, KeystoreError> {
        let out = self
            .base("lookup", account)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Self::spawn_err(&e))?;
        let text = Zeroizing::new(String::from_utf8_lossy(&out.stdout).into_owned());
        if !out.status.success() || text.is_empty() {
            debug!(account, "no passphrase in the OS keystore");
            return Ok(None);
        }
        let secret = text.trim_end_matches(['\r', '\n']).to_owned();
        Ok(Some(Passphrase::new(secret)?))
    }

    fn set(&self, account: &str, pass: &Passphrase) -> Result<(), KeystoreError> {
        let mut child = self
            .base("store", account)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Self::spawn_err(&e))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(pass.expose().as_bytes())
                .map_err(|e| KeystoreError::Secret(CryptoError::Io(e)))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| KeystoreError::Secret(CryptoError::Io(e)))?;
        if !out.status.success() {
            warn!(account, "OS keystore refused to store the passphrase");
            return Err(KeystoreError::Refused(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ));
        }
        info!(account, "stored the passphrase in the OS keystore");
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), KeystoreError> {
        self.base("clear", account)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| Self::spawn_err(&e))?;
        info!(account, "cleared the passphrase from the OS keystore");
        Ok(())
    }
}

/// Decides whether a passphrase is right before it is trusted or saved.
type Verifier = Box<dyn Fn(&Passphrase) -> bool>;

/// A [`PassphraseSource`] that checks a [`Keystore`] first and falls back to prompting.
///
/// A passphrase is saved only after it is verified against the key; a saved one that no
/// longer verifies is dropped and the user is prompted. A keystore failure is logged and
/// never blocks signing, since the prompt still works.
pub struct KeystorePassphrase<'a, K: Keystore, S: PassphraseSource> {
    store: &'a K,
    prompt: S,
    account: String,
    verify: Verifier,
}

impl<'a, K: Keystore, S: PassphraseSource> KeystorePassphrase<'a, K, S> {
    /// Look up `account` (the key path) in `store`, else ask `prompt`; verify with ssh-keygen.
    pub fn new(store: &'a K, prompt: S, account: impl Into<String>) -> Self {
        let account = account.into();
        let key = std::path::PathBuf::from(&account);
        let verify: Verifier = Box::new(move |p| passphrase_unlocks_key(&key, p));
        Self {
            store,
            prompt,
            account,
            verify,
        }
    }

    /// Replace how a passphrase is verified (tests use this to avoid a real key).
    #[must_use]
    pub fn with_verifier(mut self, verify: impl Fn(&Passphrase) -> bool + 'static) -> Self {
        self.verify = Box::new(verify);
        self
    }
}

impl<K: Keystore, S: PassphraseSource> PassphraseSource for KeystorePassphrase<'_, K, S> {
    fn passphrase(&self, prompt: &str) -> Result<Passphrase, CryptoError> {
        match self.store.get(&self.account) {
            Ok(Some(pass)) if (self.verify)(&pass) => {
                debug!(account = %self.account, "passphrase came from the OS keystore");
                return Ok(pass);
            }
            Ok(Some(_)) => {
                warn!(account = %self.account, "saved passphrase no longer unlocks the key; dropping it");
                if let Err(e) = self.store.delete(&self.account) {
                    warn!(account = %self.account, error = %e, "could not drop the stale passphrase");
                }
            }
            Ok(None) => {}
            Err(e) => warn!(account = %self.account, error = %e, "keystore lookup failed"),
        }
        let pass = self.prompt.passphrase(prompt)?;
        if !(self.verify)(&pass) {
            warn!(account = %self.account, "typed passphrase did not unlock the key; not saving it");
            return Ok(pass);
        }
        if let Err(e) = self.store.set(&self.account, &pass) {
            warn!(account = %self.account, error = %e, "could not remember the passphrase");
        }
        Ok(pass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passphrase::FixedPassphrase;
    use std::cell::Cell;

    fn pass(s: &str) -> Passphrase {
        Passphrase::new(s.into()).unwrap()
    }

    struct Counting(Cell<u32>, Passphrase);
    impl PassphraseSource for Counting {
        fn passphrase(&self, _p: &str) -> Result<Passphrase, CryptoError> {
            self.0.set(self.0.get() + 1);
            Ok(self.1.clone())
        }
    }

    /// A keystore whose every call fails, as when no secret service is running.
    struct Broken;
    impl Keystore for Broken {
        fn get(&self, _a: &str) -> Result<Option<Passphrase>, KeystoreError> {
            Err(KeystoreError::Unavailable("none".into()))
        }
        fn set(&self, _a: &str, _p: &Passphrase) -> Result<(), KeystoreError> {
            Err(KeystoreError::Unavailable("none".into()))
        }
        fn delete(&self, _a: &str) -> Result<(), KeystoreError> {
            Err(KeystoreError::Unavailable("none".into()))
        }
    }

    // frob:tests crates/goway-circle/src/keystore.rs::Keystore.get
    // frob:tests crates/goway-circle/src/keystore.rs::Keystore.set
    // frob:tests crates/goway-circle/src/keystore.rs::Keystore.delete
    #[test]
    fn keystore_round_trips_and_deletes() {
        let ks = FakeKeystore::new();
        assert!(ks.get("k").unwrap().is_none());
        ks.set("k", &pass("one two")).unwrap();
        assert_eq!(ks.get("k").unwrap().unwrap().expose(), "one two");
        ks.set("k", &pass("three")).unwrap();
        assert_eq!(ks.get("k").unwrap().unwrap().expose(), "three");
        ks.delete("k").unwrap();
        assert!(ks.get("k").unwrap().is_none());
        ks.delete("k").unwrap();
    }

    // frob:tests crates/goway-circle/src/keystore.rs::KeystorePassphrase.passphrase
    #[test]
    fn saved_passphrase_skips_the_prompt() {
        let ks = FakeKeystore::new();
        ks.set("acct", &pass("saved")).unwrap();
        let src = KeystorePassphrase::new(&ks, Counting(Cell::new(0), pass("typed")), "acct")
            .with_verifier(|_| true);
        assert_eq!(src.passphrase("p: ").unwrap().expose(), "saved");
        assert_eq!(src.prompt.0.get(), 0);
    }

    // frob:tests crates/goway-circle/src/keystore.rs::KeystorePassphrase.passphrase
    #[test]
    fn prompted_passphrase_is_remembered() {
        let ks = FakeKeystore::new();
        let src = KeystorePassphrase::new(&ks, Counting(Cell::new(0), pass("typed")), "acct")
            .with_verifier(|_| true);
        assert_eq!(src.passphrase("p: ").unwrap().expose(), "typed");
        assert_eq!(src.passphrase("p: ").unwrap().expose(), "typed");
        assert_eq!(src.prompt.0.get(), 1);
        assert_eq!(ks.get("acct").unwrap().unwrap().expose(), "typed");
    }

    // frob:tests crates/goway-circle/src/keystore.rs::KeystorePassphrase.passphrase
    #[test]
    fn broken_keystore_falls_back_to_the_prompt() {
        let src = KeystorePassphrase::new(&Broken, FixedPassphrase(pass("typed")), "acct")
            .with_verifier(|_| true);
        assert_eq!(src.passphrase("p: ").unwrap().expose(), "typed");
    }

    // frob:ticket 01M55SHEZJMS47VT8RXNXTGZVG
    // frob:tests crates/goway-circle/src/keystore.rs::KeystorePassphrase.passphrase
    #[test]
    fn mistyped_passphrase_is_not_saved() {
        let ks = FakeKeystore::new();
        let src = KeystorePassphrase::new(&ks, Counting(Cell::new(0), pass("typo")), "acct")
            .with_verifier(|p| p.expose() == "right");
        assert_eq!(src.passphrase("p: ").unwrap().expose(), "typo");
        assert!(ks.get("acct").unwrap().is_none());
    }

    // frob:ticket 01M55SHEZJMS47VT8RXNXTGZVG
    // frob:tests crates/goway-circle/src/keystore.rs::KeystorePassphrase.passphrase
    #[test]
    fn stale_saved_passphrase_is_dropped_and_reprompted() {
        let ks = FakeKeystore::new();
        ks.set("acct", &pass("wrong")).unwrap();
        let src = KeystorePassphrase::new(&ks, Counting(Cell::new(0), pass("right")), "acct")
            .with_verifier(|p| p.expose() == "right");
        assert_eq!(src.passphrase("p: ").unwrap().expose(), "right");
        assert_eq!(src.prompt.0.get(), 1);
        assert_eq!(ks.get("acct").unwrap().unwrap().expose(), "right");
    }

    // frob:tests crates/goway-circle/src/keystore.rs::KeystorePassphrase.passphrase
    #[test]
    fn accounts_are_isolated() {
        let ks = FakeKeystore::new();
        ks.set("a", &pass("aaa")).unwrap();
        assert!(ks.get("b").unwrap().is_none());
    }

    /// Writes a fake `secret-tool` that keeps one file per account under `dir`.
    #[cfg(unix)]
    fn fake_secret_tool(dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let script = dir.join("secret-tool");
        let body = format!(
            "#!/bin/sh\nverb=$1\nfor a; do acct=$a; done\nf='{}'/\"$acct\"\n\
             case $verb in\n store) cat > \"$f\" ;;\n lookup) cat \"$f\" 2>/dev/null || exit 1 ;;\n\
             clear) rm -f \"$f\" ;;\n *) exit 2 ;;\nesac\n",
            dir.display()
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    // frob:ticket 01M5556M6B1K0X435YP2H934R3
    // frob:tests crates/goway-circle/src/keystore.rs::SecretToolKeystore.get
    // frob:tests crates/goway-circle/src/keystore.rs::SecretToolKeystore.set
    // frob:tests crates/goway-circle/src/keystore.rs::SecretToolKeystore.delete
    #[cfg(unix)]
    #[test]
    fn secret_tool_protocol_stores_loads_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let ks = ProgramKeystore::new(fake_secret_tool(dir.path()));
        assert!(ks.get("acct").unwrap().is_none());
        ks.set("acct", &pass("hunter2")).unwrap();
        assert_eq!(ks.get("acct").unwrap().unwrap().expose(), "hunter2");
        assert!(ks.get("other").unwrap().is_none());
        ks.delete("acct").unwrap();
        assert!(ks.get("acct").unwrap().is_none());
    }

    // frob:ticket 01M5556M6B1K0X435YP2H934R3
    // frob:tests crates/goway-circle/src/keystore.rs::SecretToolKeystore.get
    #[test]
    fn missing_secret_tool_is_unavailable() {
        let ks = ProgramKeystore::new("/nonexistent/secret-tool");
        assert!(matches!(ks.get("a"), Err(KeystoreError::Unavailable(_))));
    }
}
