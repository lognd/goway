# Circles: peer-to-peer trust for goway

Status: phases 1 to 4 are implemented (the `goway-circle` crate,
`goway circle init|card|admit|join|list|revoke|roles`,
`goway circle access|cert|pin`, LAN pairing, and
`goway circle admin|renew|policy`) and shipped in 0.2.0; `goway-setup --join`
is not yet. The executable model of the rules in this
document is `docs/model/circle_model.py` (run its tests with
`python -I -m unittest discover -s docs/model`). Tickets are under the
`circles` label (`frob ticket list --label circles`).

## 1. Why

Today every main laptop (a **lead**) is set up by hand against every helper:
it creates a key, runs `goway host add` per helper (confirming each host-key
fingerprint), authorizes its key on each helper and keeps a `[[host]]` list.
With N leads and M helpers that is N x M manual trust edges. goway has no
daemon; the data plane is the system `ssh` plus a script on the helper. So
the cost to remove is trust distribution, not the transport.

A **circle** replaces the N x M edges: one signed membership record that
every machine holds. Joining a circle once makes a machine usable by, and able
to use, every other member, subject to roles and per-helper policy.

Non-goals: a daemon or mesh network, a shared private key, transitive trust,
sandboxing of the commands you run (still out of scope, see SECURITY.md).

## 2. Trust model

1. Only public keys and host-key fingerprints are shared. A private key never
   leaves its machine. A circle-wide shared key is rejected: one theft would
   compromise everything and nothing could be revoked per machine.
2. Trust is not transitive. Nothing grants access because a trusted machine
   vouches for another, except an **admin** acting through a signed operation.
3. The two directions are distinct. Trusting a lead lets it run code as my
   user on my machine. Trusting a helper means I send it my source tree and
   `--env` values. The roster records each separately (roles).
4. The root of trust is a human comparing a fingerprint on two screens they
   control. The LAN is assumed hostile (the measured network is a /13).
5. Membership changes need an admin. There is no leaderless design in which
   "this key may log in" needs no one's decision. The data plane is fully
   peer-to-peer: any lead runs on any helper with no admin online, until its
   certificate expires.

## 3. Roles

Roles are per machine, written in the roster and enforced by the receiver.

| Role | Needs | Holds | Attack surface |
|---|---|---|---|
| `lead` | goway binary | outbound node key | no sshd, no inbound port, no root, no firewall change |
| `helper` | sshd plus the host component | no credential that authenticates anywhere | inbound sshd |
| `lead,helper` | both | both | both |
| `admin` (flag) | a lead, plus a separate admin key | can mint access | see threat 1 |

A helper-only machine cannot be used to pivot to a lead, and a lead-only
machine is never a target. Only `lead` members are issued access certificates.
An admin is always a lead; a helper-only machine can never be admin.

## 4. Mechanism

### 4.1 Identities

- **Node key**: ed25519, one per machine, kept in the config dir
  (`circle/node_ed25519`), mode 0600 (Windows: user-only ACL via the existing
  `restrict_key_acl`). For a lead it is also the ssh login key. For a
  helper-only machine it only signs and is never placed in any
  `authorized_keys`.
- **Admin key**: a second ed25519 key, used only to sign roster operations and
  certificates. Never the daily login key. `ed25519-sk` (FIDO2) is
  recommended; goway shells out to `ssh-keygen`, so a hardware key works
  without goway code.
- **Host key**: the machine's sshd host key. The roster carries its
  fingerprint, so leads pin it without a TOFU prompt.
- Only ed25519 is accepted anywhere. There is no negotiation.
- Signing uses `ssh-keygen -Y sign/verify` with namespace `goway-circle-v1`
  (operations) and ssh certificates (access). No crypto library is added.
  `goway doctor` checks `ssh-keygen -Y` support (OpenSSH 8.0+).

### 4.2 Roster

An append-only DAG of signed operations. An operation is
`{id, kind, body, parents, signers, sigs}` where `id` is the SHA-256 of the
canonical encoding of everything but `sigs`, and `parents` are the ids the
author had seen. Canonical encoding: JCS (RFC 8785) JSON, so the same bytes
are hashed and signed on every platform.

| Kind | Body | Valid when signed by |
|---|---|---|
| `genesis` | circle name, founding admin key | that admin (self) |
| `admit` | node key, name, roles, host fingerprint | an admin |
| `revoke` | node key | an admin; same quorum as `add_admin` when the key is an admin and there are 3 or more |
| `set_roles` | node key, roles | an admin |
| `add_admin` | node key | an admin; two distinct admins once there are 3 or more |
| `remove_admin` | node key | same rule as `add_admin` |
| `rotate_host_key` | new fingerprint | the node itself |

State is computed by deterministic replay (the `fold` function of the model):

- Operations are applied in causal layers. Inside a layer, `revoke` and
  `remove_admin` apply first, then the rest in id order. The result depends
  only on the set of operations, never on arrival order (convergence).
- Each operation is validated against the state at its own point of replay.
  An invalid or unauthorized operation is ignored, not an error, so a bad
  operation cannot block the circle.
- A revoked node key is tombstoned forever. A later `admit` of that key is
  ignored (remove wins). Re-joining takes a new key.
- Merging two rosters is set union followed by replay. Operations
  authenticate themselves, so they may arrive over ssh, a pasted string or a
  USB stick; the transport is not trusted.
- Anti-rollback: each node stores the heads it has seen per admin and refuses
  a roster that lacks any of them. An explicit `--accept-reset` is the only
  way past it.
- Size caps: at most 4096 operations and 64 KiB per operation. Parsing is
  strict (unknown fields rejected), as with helper answers today.

### 4.3 Access: ssh certificates

An admin signs a certificate for a lead's node key:

    ssh-keygen -s <admin key> -I goway:<circle>:<name> -n goway-lead:<circle>:e<epoch> \
        -V -5m:+14d -O no-agent-forwarding -O no-port-forwarding -O no-x11-forwarding <node.pub>

A helper has, once, one line per admin in `authorized_keys`:

    cert-authority,principals="goway-lead:<circle>:e<epoch>",no-agent-forwarding,no-port-forwarding,no-X11-forwarding ssh-ed25519 <admin pub> goway:<id>

- A new lead works on every helper at once, with no helper edit.
- No root is needed; `cert-authority` works in a user's `authorized_keys`.
  The line is tagged `goway:<id>` and managed through `goway-journal`, so
  uninstall removes it. Windows uses `administrators_authorized_keys` through
  the existing elevated `goway-setup` path.
- **Epoch**: bumping the epoch (on `revoke`, `remove_admin`) invalidates every
  outstanding certificate; admins reissue to the remaining leads. A helper
  learns the new epoch the next time any trusted member contacts it. The
  revocation window for an offline helper is therefore at most the
  certificate TTL (default 14 days).
- **Backdating** `-V -5m`: WSL2 clocks drift after suspend. `goway doctor`
  reports skew against the helper.
- **Renewal**: a lead asks for a new certificate when half the TTL has
  passed and any admin is reachable; failing that, the old one keeps working
  until it expires.
- **Policy** on the helper (`policy.toml`): `open` (all leads of the circle,
  default) or `listed` (only named nodes, each with the extra principal
  `goway-node:<name>`, placed in the line's `principals=`).

Implemented (phase 3, `goway-circle/src/sshcert.rs`, `goway/src/access.rs`):

- `goway circle cert [NAME]` has an admin mint a real OpenSSH user
  certificate (14-day TTL, backdated 5 minutes) for a member lead, after the
  model checks of `issue_cert`. Renewal is rerunning `cert`.
- Principals: `goway-lead:<circle>:e<epoch>` and
  `goway-lead:<circle>:e<epoch>:lead:<name>`. Policy `open` accepts the
  first; policy `listed` accepts only the `:lead:<name>` principals of the
  named leads. This replaces the `goway-node:<name>` wording above; the
  circle and epoch stay bound in both.
- `goway circle access [--lead NAME].. [--open] [--file F] [--windows-admin]`
  writes the helper's `cert-authority` lines (tagged
  `goway-circle:<circle>:e<epoch>`). Lines of older epochs are replaced, so
  after a revoke every helper must rerun `access` to refuse old certificates;
  a new lead needs no helper edit. `--windows-admin` targets
  `%ProgramData%\ssh\administrators_authorized_keys`. The policy is kept in
  `access-<id>.policy` in the circle directory.
- `goway circle pin NAME [--address A] [--port P]` scans the host key and
  writes it to `known_hosts` as `goway-<name>` only if its fingerprint equals
  the roster's `host_fp`; a mismatch is refused.
- Roster helpers (other members) are merged into the host config as hosts
  labelled `circle`; a manual `[[host]]` of the same name wins.

`goway doctor` checks local `ssh-keygen -Y` support (OpenSSH 8.0+), a sane
local clock, and warns when a helper's clock is off by more than the 5
minute certificate backdate.

- `goway circle admin card [--name N]` prints a card for this machine's admin
  key (named `<machine>-admin`); an existing admin admits it with
  `goway circle admit`. A new admin must first be a member with the `lead`
  role under its admin key.
- `goway circle admin add NAME` / `remove NAME` appends an AddAdmin /
  RemoveAdmin op. With 1 or 2 admins one signature applies it at once. With 3
  or more admins the op needs 2 distinct admin signers: pass
  `--cosigner OTHER [--out FILE]` to write a half-signed proposal, then the
  other admin runs `goway circle admin cosign FILE` to finish and append it.
  Without a cosigner the command is refused. Removing an admin bumps the
  roster epoch.
- `goway circle renew [--force]` reissues the node certificate when it is
  missing or past half of its 14 day TTL; `--force` reissues regardless.
- `goway circle policy open` accepts every circle member;
  `goway circle policy listed --lead NAME ...` accepts only the named leads
  (at least one known lead; `open` takes none). `--file` overrides the
  `authorized_keys` target and `--windows-admin` targets the Windows
  administrators file.

### 4.4 Pairing (no listener, no TOFU)

Pairing exchanges two strings. The fingerprint and name are displayed for the
human to confirm at both ends.

1. New machine: `goway circle card [--role lead|helper|both]` prints a
   **card**: `gwc1.` plus base32 of `{node pubkey, host fingerprint, name,
   roles}` (about 150 characters; also a QR code on request).
2. Admin: `goway circle admit <card>` shows name, roles and fingerprint,
   asks to confirm, appends an `admit` operation and prints a **welcome**:
   `gww1.` plus base32 of `{circle name, genesis id, every operation of the
   roster}` (the first certificate joins it in phase 3).
3. New machine: `goway circle join <welcome>` verifies the genesis hash it is
   shown against the admin's fingerprint (printed by step 2, compared by
   eye), stores the circle and, on first contact, pulls the full roster.
   `goway-setup install --host --join <welcome> --join-fingerprint <fp>` does
   the same for a WSL helper: after the host install it runs `goway circle
   join <welcome> --fingerprint <fp>` and then `goway circle access` inside
   the distro, so the access lines are installed. A bad paste is rejected
   before anything changes; a failed join prints the commands to retry. With
   `--native` the same runs the installed `goway.exe` on the laptop itself.
   Both first run `goway circle card`, which creates the node key when none
   exists, and print the card. For WSL, goway must be on the distro's PATH.

#### LAN pairing (`pair-host`, `pair-join`)

`goway circle pair-host [--port N] [--bind ADDR] [--wait MIN]` on an admin
and `goway circle pair-join <addr:port> [--peer NAME]` on the new machine do
steps 1 to 3 over one short TCP session. The card and welcome are the same
bytes; the fingerprint comparison is replaced by a six digit code:

    guest -> host   commit    SHA-256(guest nonce, card)
    host  -> guest  nonce     host nonce, admin key fingerprint(s)
    guest -> host   reveal    guest nonce, card (host checks it against the commit)
    both            code      digits from SHA-256(both nonces, fingerprints, card)
    guest -> host   confirm   only after the new machine's person typed the host's code
    host  -> guest  welcome   only after the admin's person typed the guest's code too

Each person types the code shown on the OTHER screen. Nothing is admitted,
stored or sent before both match; a mismatch, a declined prompt, a closed
terminal or a timeout changes nothing on either machine. The guest commits
to its card before it sees the host's nonce, so an attacker in the middle
who swaps the card or the host identity has about a one in a million chance
per attempt of producing equal codes, and the listener allows few attempts.
The new machine also requires the admin that paired with it to be an admin
of the circle in the welcome, then verifies the genesis as in `join`.

Deviation from the design: CPace (a PAKE) needs a prime-order group, and no
such crate is in the dependency tree, so a hand-rolled one was refused.
The substitute is the commit-reveal short authentication string above, the
closest safe design without a group. It has no offline-guessing exposure
because no secret code is typed into the channel; the codes are only
compared. A real CPace upgrade is a separate ticket.

The listener is open only during pairing: it expires (`--wait`, default 5,
at most 30 minutes), is single use (it closes after the first session that
reaches the comparison), enforces a one second gap between connections,
and closes after five failed sessions. It reads one JSON line per message
with fixed size caps and unknown fields refused. mDNS announcements never
carry circle identity: nothing is announced, the admin prints its port and
the person gives the new machine the admin's LAN address.

On success the admin remembers the new machine's address, and
`pair-join --peer NAME` remembers the admin's, as peer address hints (4.5).
The QR code for the card is not built yet.

### 4.5 Discovery and resolution

Hosts stay "a name plus a pinned host key". The roster supplies the name and
the key, so the pin comes from an admin-countersigned fingerprint, not a
prompt. The resolution order of `docs/hosts.md` gains one stage after
`cached`: **peer hints**, unsigned last-seen addresses learned from other
leads. A hint is safe because the pinned host key is verified before use.
Addresses are never written into the roster.

A **roster** stage (`Source::Roster`, shown as `circle roster`) runs right
after the hint stage. It applies only to roster hosts: those labelled `circle`
with no address of their own. It resolves the roster name through the system
resolver, then through Windows (WSL interop) when that finds nothing. The host
key is still verified against the roster fingerprint before the address is
used, so a wrong answer is rejected like any other stage.

### 4.6 Sync

New remote verbs `circle-heads` and `circle-exchange` (in `remote.sh` and
`remote.ps1`) ship operations both ways on every ssh call goway already makes,
so rosters spread by use. Every received operation is verified before it is
applied.

### 4.7 Config and files

    <config dir>/circle/node_ed25519{,.pub}   node key
    <config dir>/circle/admin_ed25519{,.pub}  admin key (admins only; or a *-sk handle)
    <config dir>/circle/circles/<id>/ops.jsonl   the operations (<id> = first 16 hex of the genesis id)
    <config dir>/circle/circles/<id>/heads.json  anti-rollback heads
    <config dir>/circle/circles/<id>/certs/      issued certificate (leads; phase 3)
    <config dir>/policy.toml                  helper policy (helpers)

Existing `[[host]]` entries keep working as a "manual" source and win over a
roster entry with the same name. Per-host tuning (labels, `max_jobs`,
`priority`) stays in local `config.toml`, keyed by name. `goway circle init`
can adopt hosts the user already trusts, with the user attesting as admin.
Every file change is recorded through `goway-journal` and shown in
`goway changes`. Every state change, branch and error path logs through the
module logger; tokens, cards and welcomes are never logged beyond a
fingerprint.

Implemented in phase 2: the confirmation is a fingerprint a person types back
(or passes as `--fingerprint SHA256:...`); `admit` compares the card's
node-key fingerprint, `join` compares an admin-key fingerprint of the
welcome. A wrong fingerprint changes nothing. `revoke` asks unless `--yes`.
`revoke --now` and the certificate expiry and last seen columns of `list` are implemented (see section 5).

## 5. Commands

    goway circle init [NAME]            create a circle; this machine becomes admin 1
    goway circle card [--role ...]      print this machine's card
    goway circle admit <card>           admit a machine (admin)
    goway circle join <welcome>         join with a welcome
    goway circle list                   members, roles, cert expiry, last seen
    goway circle revoke <name>          revoke; bump epoch; reissue; list unreachable helpers
    goway circle roles <name> <roles>   change roles (admin)
    goway circle sync <ops>             merge a pushed roster (run by revoke --now)
    goway circle access [--lead N].. [--open] [--windows-admin]   write cert-authority lines
    goway circle cert [NAME]            mint a lead certificate (admin)
    goway circle pin <name>             pin a helper's host key from the roster
    goway circle admin add|remove <name>
    goway circle renew                  renew this machine's certificate
    goway circle policy open|listed     helper policy

`revoke --now` sends the roster to every other helper of the roster (found
like any pool host) and runs, over ssh, `goway circle sync --circle ID OPS &&
goway circle access --circle ID`. `sync` merges the offered operations into the
helper's own (union by operation id, then replay) and refuses any result that
lacks a head the helper already saw, so a stale push cannot roll it back; then
`access` moves its `cert-authority` lines to the new epoch. Helpers it could
not reach are printed and stay exposed until they hear. Each helper that
answers is recorded as last seen. The roster is sent on ssh stdin (`sync` with
`-` as its operations argument reads stdin), so its size is not limited by the
command line. `cert`/`renew` record the expiry of every certificate this
machine issues, so `list` shows it for those members too (marked "issued
here"); certificates issued by another admin are not known here, and last seen
comes only from these pushes.

## 6. Security audit

| # | Threat | Impact | Mitigation | Residual |
|---|---|---|---|---|
| 1 | Admin key theft | Mint a login to every helper | Separate admin key; FIDO2 recommended; 1-3 admins; two signatures to change admins at 3+, including revoking an admin | Highest risk; documented |
| 2 | Stolen or compromised lead | RCE on trusting helpers | Short TTL; epoch bump on revoke; key file 0600 and refused if looser, or ssh-agent / FIDO2 (section 6.2) | Window until helpers hear, at most TTL |
| 3 | Hostile-LAN pairing: MITM, spoofing | Attacker joins or impersonates | Offline blobs need no network trust; PAKE plus SAS for LAN; single use, expiring listener | User skips the comparison |
| 4 | Roster rollback or replay | Removed member returns | Remove wins; tombstones permanent; heads refuse regression | Node offline since before the removal |
| 5 | Compromised helper | Source and `--env` leak, false results | Out of scope (SECURITY.md); helper-only nodes hold no credential, so no pivot | Data already sent |
| 6 | Principal or circle confusion | Cert reused across circle or role | Principal carries circle, role, epoch; line pins `principals=` | None expected |
| 7 | Clock skew | Rejected or over-long certs | Backdate 5 minutes; generous TTL; doctor reports skew | Annoyance |
| 8 | Roster privacy | Outsiders map the fleet | No broadcast; roster served to members only; mDNS carries nothing | Members see names |
| 9 | Malicious invited human | RCE as my user | `listed` policy; v1 is own machines only; no cross-circle guests | Access to a person is RCE |
| 10 | Local tamper, other local users | Forged roster | 0600; signatures re-verified every load; journaled | Same-user malware already wins |
| 11 | Secrets in logs | Key or token leak | Never log cards, welcomes, keys | None expected |
| 12 | Enrollment DoS | Exposed port | Nothing listens by default | None |
| 13 | Parser abuse | Crash or memory exhaustion | Strict parse; size and count caps; ops verified before use | None expected |
| 14 | Supply chain | Malicious installer | Unchanged: checksums, provenance | Existing |
| 15 | Lead that is also a helper | Lateral movement | Lead-only and helper-only roles; `listed` policy | One theft reaches all both-role machines |
| 16 | Name squatting | Impersonation | Identity is the key; names are labels; fingerprints always shown | User ignores fingerprint |

### 6.1 Enforcement per threat (audited ~MV967CF)

| # | Verdict | Enforced by |
|---|---|---|
| 1 | Mitigated | Quorum rules in `state.rs`; admin key may be agent or FIDO2 only (6.2) |
| 2 | Fixed | `keys.rs::ensure_keypair` refuses loose modes, symlinks, writable key dirs (INV-CIRCLE-001); tests `loose_key_mode_is_refused`, `symlinked_key_is_refused`, `writable_key_dir_is_refused` |
| 3 | Enforced | `lan.rs` size and count caps, commit-reveal SAS, single-use `ListenGuard`, timeouts |
| 4 | Enforced | `Store::commit` and `accept_roster`; tests `rollback_is_refused_and_nothing_is_written`, `rollback_is_refused` |
| 5 | Out of scope | SECURITY.md |
| 6 | Enforced | Principal `goway-lead:<circle>:e<epoch>` plus `principals=` pin; `sshcert.rs` `accepts` tests; pin mismatch refused (INV-CIRCLE-003) |
| 7 | Mitigated | 5 minute backdate in `sshcert.rs`; doctor skew check |
| 8 | Enforced | No listener by default; roster served to members only |
| 9 | Policy | `Policy::Listed` now drops any principal that is not a valid name; test `listed_names_that_could_inject_options_are_dropped` |
| 10 | Fixed | Same as row 2; signatures re-verified on load |
| 11 | Swept | No key, card, welcome or token reaches a log call |
| 12 | Enforced | Nothing listens by default |
| 13 | Fixed | Per-op and op-count caps; the whole ops file is now size-checked before it is read (`OpsTooLarge`, test `an_oversized_ops_file_is_refused_unread`) |
| 14 | Existing | Installer checksums |
| 15 | Policy | Role split and `listed` policy |
| 16 | Fixed | `state.rs` rejects invalid circle and member names and any host fingerprint with quote, comma, backslash or non-graphic bytes (tests `unsafe_names_and_fingerprints_never_enter_the_roster`, `rotate_host_key_with_unsafe_fingerprint_is_ignored`); `authority_lines` drops admin keys with embedded newlines |

Signatures use the fixed namespace `goway-circle-v1` (INV-CIRCLE-002), so a roster
signature cannot be replayed as any other ssh signature.

### 6.2 Keys at rest, ssh-agent and FIDO2

- A private key file must be a regular file (not a symlink), mode with no group or
  other bits, in a directory not group or world writable. Anything else is refused with
  `KeyUnsafe`; a private file without its `.pub` is `KeyIncomplete` and is never
  regenerated, so a key is never destroyed.
- A public key with no private file is a public-only identity: signing and minting use
  `ssh-keygen -Y sign` / `-s ... -U`, so the ssh-agent (or a hardware token) holds the
  secret.
- FIDO2: generate with `ssh-keygen -t ed25519-sk -f <admin key>`, keep only the handle file
  or load into the agent. Certificates signed by an `sk-ssh-ed25519@openssh.com` key are
  parsed and accepted as admin signatures. Generation is manual (~SKGF7C1).
- Passphrase-protected keys work through the ssh-keygen prompt.
- Deferred: OS keystore (~WNW4H3Y), built-in passphrase prompt and encrypted node key
  (~7JMKV3B).

## 7. Alternatives rejected

- Shared circle-wide private key: no per-machine revocation.
- Per-pair `authorized_keys` synced from the roster: a new lead cannot reach a
  helper that has not yet heard of it.
- Flat web of mutual introductions: same end set per helper as an admin
  design, harder to audit.
- Mesh or daemon (libp2p, WireGuard, Tailscale-like): a daemon per machine
  is the friction being removed. A mesh VPN may still serve as an underlay
  across networks; circles are independent of it.

## 8. Phases

Phase 1 crate note: `crates/goway-circle` implements section 4.2 and the
certificate rules of 4.3 (operations, canonical-JSON ids, signing through
`Signer`/`Verifier` traits with an `ssh-keygen -Y` backend, `fold`, `merge`,
`heads`/`accept_roster`, `issue_cert`/`helper_accepts`). Beyond the model it
adds `fold_pinned` (only the genesis the user verified may found the circle)
and requires distinct signers. Its tests mirror `docs/model/` and add property
tests for convergence and for unauthorized operations never changing state.


1. Spec, model, `goway-circle` crate (operations, signing, fold, merge,
   anti-rollback, property tests).
2. `goway circle` card, admit, join, list, revoke; `goway-setup --join`.
3. Certificates, `authorized_keys` lines, roster-fed pinning, config merge,
   Windows path.
4. LAN pairing (commit-reveal SAS; CPace and QR are follow-ups), peer address hints. Done except QR and CPace.
5. Later: forced-command gate for instant revocation, guest sandbox,
   cross-circle federation.
