//! The write-ahead apply loop and the reverse replay.

use crate::change::Change;
use crate::error::{ApplyError, JournalError};
use crate::journal::{Entry, Journal};
use crate::plan::{Outcome, plan_apply, plan_revert, run};
use crate::system::System;

/// Per-entry results of a revert, in replay (reverse) order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RevertReport {
    /// `(entry index, outcome)` for every entry, last entry first.
    pub outcomes: Vec<(usize, Outcome)>,
}

/// Apply `plan` to `sys`, returning the journal that inverts it.
pub fn apply(plan: &[Change], sys: &mut (impl System + ?Sized)) -> Result<Journal, ApplyError> {
    apply_with(plan, sys, Journal::generate(), &mut |_| Ok(()))
}

/// Apply `plan` into `journal`, calling `sink` after each entry is recorded and before it is applied.
///
/// A consumer persists the journal in `sink` to make the record crash-safe. On failure the
/// returned error carries the journal so the partial work can be reverted.
pub fn apply_with(
    plan: &[Change],
    sys: &mut (impl System + ?Sized),
    journal: Journal,
    sink: &mut dyn FnMut(&Journal) -> Result<(), JournalError>,
) -> Result<Journal, ApplyError> {
    let mut journal = journal;
    tracing::info!(journal = %journal.id, changes = plan.len(), "apply: start");
    for (index, change) in plan.iter().enumerate() {
        let step = (|| {
            let (prior, ops) = plan_apply(change, sys)?;
            tracing::info!(journal = %journal.id, index, ?change, ?prior, ops = ops.len(), "apply: recorded");
            journal.entries.push(Entry {
                change: change.clone(),
                prior,
                reverted: false,
            });
            sink(&journal)?;
            run(sys, &ops)
        })();
        if let Err(source) = step {
            tracing::error!(journal = %journal.id, index, error = %source, "apply: failed");
            return Err(ApplyError {
                index,
                source,
                journal: Box::new(journal),
            });
        }
    }
    tracing::info!(journal = %journal.id, "apply: done");
    Ok(journal)
}

/// Invert `journal` against `sys`, last entry first; safe to repeat and to resume after a failure.
///
/// Each entry is marked `reverted` once undone, so a second call does nothing. Targets that no
/// longer hold what goway wrote are left alone and reported.
pub fn revert(
    journal: &mut Journal,
    sys: &mut (impl System + ?Sized),
) -> Result<RevertReport, JournalError> {
    tracing::info!(journal = %journal.id, entries = journal.entries.len(), "revert: start");
    let mut report = RevertReport::default();
    for index in (0..journal.entries.len()).rev() {
        let entry = &journal.entries[index];
        if entry.reverted {
            report.outcomes.push((index, Outcome::AlreadyReverted));
            continue;
        }
        let (outcome, ops) = plan_revert(entry, sys)?;
        run(sys, &ops)?;
        tracing::info!(journal = %journal.id, index, ?outcome, "revert: entry done");
        journal.entries[index].reverted = true;
        report.outcomes.push((index, outcome));
    }
    tracing::info!(journal = %journal.id, "revert: done");
    Ok(report)
}
