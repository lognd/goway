//! The persisted record of applied changes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::change::Change;
use crate::error::JournalError;

/// What was captured before a change was applied; drives its inverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Prior {
    /// The target already held the desired value; revert does nothing.
    Noop,
    /// Prior file contents (`None` when absent).
    File {
        /// The contents before, if the file existed.
        contents: Option<String>,
    },
    /// A line was appended to a text file.
    Line {
        /// The file did not exist.
        created_file: bool,
        /// The file lacked a final newline and one was added.
        fixed_newline: bool,
    },
    /// Directories that did not exist and were created, outermost first.
    DirsCreated {
        /// Created directories, outermost first.
        created: Vec<PathBuf>,
    },
    /// The destination file did not exist and was copied into place.
    FileInstalled,
    /// The registry key did not exist and was created.
    KeyCreated,
    /// An entry was added to a list variable.
    ListEntry {
        /// The variable was unset.
        created_var: bool,
    },
    /// Prior registry value (`None` when absent).
    Registry {
        /// The value before, if any.
        value: Option<crate::change::RegValue>,
    },
    /// An ini key line was replaced.
    IniReplaced {
        /// The full original line.
        original_line: String,
    },
    /// An ini key line was inserted.
    IniInserted {
        /// The file did not exist.
        created_file: bool,
        /// The section header was added too.
        created_section: bool,
        /// A blank separator line precedes the added header.
        added_blank: bool,
        /// The file lacked a final newline and one was added.
        fixed_newline: bool,
    },
    /// Prior unix mode.
    Mode {
        /// The mode before.
        mode: u32,
    },
    /// Prior SDDL.
    Acl {
        /// The SDDL before.
        sddl: String,
    },
    /// The resource did not exist and was created.
    ResourceCreated,
    /// An outdated resource was replaced; `previous` is the snapshot that restores it exactly.
    ResourceReplaced {
        /// Snapshot taken by `System::resource_outdated` before the replacement.
        previous: String,
    },
}

/// One applied change with the state captured before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The change that was applied.
    pub change: Change,
    /// The prior state captured before applying.
    pub prior: Prior,
    /// Set once the entry has been inverted; makes revert idempotent and resumable.
    #[serde(default)]
    pub reverted: bool,
}

/// An ordered, persistable record of everything an install changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// Unique id of this journal.
    pub id: String,
    /// Creation time, seconds since the unix epoch.
    pub created_unix_secs: u64,
    /// Entries in application order.
    pub entries: Vec<Entry>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Journal {
    /// A new empty journal with the given id, stamped now.
    pub fn new(id: impl Into<String>) -> Self {
        let created_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Self {
            id: id.into(),
            created_unix_secs,
            entries: Vec::new(),
        }
    }

    /// A new empty journal with a generated id (time, pid, counter).
    pub fn generate() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self::new(format!("{nanos:x}-{:x}-{n:x}", std::process::id()))
    }

    /// Write the journal as JSON, atomically (exclusively created temp file, then rename).
    pub fn save(&self, path: &Path) -> Result<(), JournalError> {
        let io = |source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        };
        let json = serde_json::to_string_pretty(self)?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        // A stale temp file is removed and the new one created exclusively, so a file or link
        // planted at the temp path is never written through (the journal may live in a
        // directory only an administrator can write, but the save must not rely on that).
        match std::fs::remove_file(&tmp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(io)?;
            file.write_all(json.as_bytes()).map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)?;
        tracing::debug!(journal = %self.id, entries = self.entries.len(), path = %path.display(), "journal saved");
        Ok(())
    }

    /// Read a journal previously written by `save`.
    pub fn load(path: &Path) -> Result<Self, JournalError> {
        let text = std::fs::read_to_string(path).map_err(|source| JournalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let journal: Self = serde_json::from_str(&text)?;
        tracing::debug!(journal = %journal.id, entries = journal.entries.len(), path = %path.display(), "journal loaded");
        Ok(journal)
    }
}
