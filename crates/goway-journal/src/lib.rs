//! Write-ahead change journal: every change records the prior state before it
//! is applied, so replaying the journal backwards provably restores the system.
//!
//! The guarantee is checked by property tests over [`ModelSystem`]:
//! `revert(apply(plan)) == initial` for arbitrary initial states and plans.

mod apply;
mod change;
mod digest;
mod error;
mod journal;
mod linefile;
mod local;
#[cfg(windows)]
mod local_windows;
mod model;
mod plan;
mod system;

pub use apply::{RevertReport, apply, apply_with, revert};
pub use change::{Change, ListPosition, RegValue, ResourceKind};
pub use digest::sha256_hex;
pub use error::{ApplyError, JournalError, SystemError};
pub use journal::{Entry, Journal, Prior};
pub use local::LocalSystem;
pub use model::{DEFAULT_MODE, DEFAULT_SDDL, ModelSystem};
pub use plan::{Outcome, still_applied};
pub use system::{SysResult, System};
