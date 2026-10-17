//! Access-certificate issue and accept rules (the model of ssh certificates).

use tracing::info;

use crate::error::CertError;
use crate::state::{Role, State};

/// Default certificate lifetime in days.
pub const CERT_TTL_DAYS: u64 = 14;
/// Default certificate lifetime in seconds.
pub const CERT_TTL_SECS: u64 = CERT_TTL_DAYS * 86_400;

/// The facts of an ssh certificate a helper checks: signer, subject, principal, expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cert {
    /// Admin key that signed it.
    pub signer: String,
    /// Lead node key it was issued to.
    pub subject: String,
    /// Principal, `goway-lead:<circle>:e<epoch>`.
    pub principal: String,
    /// Expiry as Unix seconds.
    pub expires: u64,
}

/// The principal a helper requires for `state`: it carries circle, role and epoch.
pub fn principal(state: &State) -> String {
    format!("goway-lead:{}:e{}", state.circle, state.epoch)
}

/// Mint a lead cert; only an admin, only for a current member with role lead.
pub fn issue_cert(state: &State, admin: &str, subject: &str, now: u64) -> Result<Cert, CertError> {
    if !state.admins.contains(admin) {
        return Err(CertError::NotAdmin);
    }
    let member = state.members.get(subject).ok_or(CertError::NotMember)?;
    if !member.roles.contains(&Role::Lead) {
        return Err(CertError::NotLead);
    }
    info!(circle = %state.circle, epoch = state.epoch, "certificate issued");
    Ok(Cert {
        signer: admin.to_owned(),
        subject: subject.to_owned(),
        principal: principal(state),
        expires: now + CERT_TTL_SECS,
    })
}

/// Model of a helper's `cert-authority` line for one circle: admin signer, current epoch, unexpired.
pub fn helper_accepts(state: &State, cert: &Cert, now: u64) -> bool {
    state.admins.contains(&cert.signer) && cert.principal == principal(state) && now <= cert.expires
}
