//! Shared domain types for wdu.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The broad kind of file-system change reported by the monitor.
#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
pub enum FileChangeKind {
    Created,
    Modified,
    Removed,
    Other,
}

/// A file-system event that may cause a directory usage recalculation.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: FileChangeKind,
    pub observed_at_unix_secs: u64,
}

impl FileChange {
    pub fn new(path: PathBuf, kind: FileChangeKind, observed_at_unix_secs: u64) -> Self {
        Self {
            path,
            kind,
            observed_at_unix_secs,
        }
    }
}

/// A recursively measured directory and its inclusive logical byte usage.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectorySnapshot {
    pub directory: PathBuf,
    pub usage_bytes: u64,
}

impl DirectorySnapshot {
    pub fn new(directory: PathBuf, usage_bytes: u64) -> Self {
        Self {
            directory,
            usage_bytes,
        }
    }
}

/// The materialized usage state for one directory.
///
/// Both usage values include the directory's descendants. The cumulative value
/// is relative to the first successful observation, not to the previous event.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectoryUsageAggregate {
    pub directory: PathBuf,
    pub current_usage_bytes: u64,
    pub cumulative_delta_bytes: i64,
    pub is_present: bool,
    pub observed_at_unix_secs: u64,
}

impl DirectoryUsageAggregate {
    pub fn new(
        directory: PathBuf,
        current_usage_bytes: u64,
        cumulative_delta_bytes: i64,
        is_present: bool,
        observed_at_unix_secs: u64,
    ) -> Self {
        Self {
            directory,
            current_usage_bytes,
            cumulative_delta_bytes,
            is_present,
            observed_at_unix_secs,
        }
    }

    pub fn is_increase(&self) -> bool {
        self.cumulative_delta_bytes > 0
    }

    pub fn is_decrease(&self) -> bool {
        self.cumulative_delta_bytes < 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_cumulative_delta_direction() {
        let increase = DirectoryUsageAggregate::new(PathBuf::from("/tmp"), 2, 1, true, 0);
        let decrease = DirectoryUsageAggregate::new(PathBuf::from("/tmp"), 0, -1, false, 0);
        let unchanged = DirectoryUsageAggregate::new(PathBuf::from("/tmp"), 1, 0, true, 0);

        assert!(increase.is_increase());
        assert!(!increase.is_decrease());
        assert!(decrease.is_decrease());
        assert!(!decrease.is_increase());
        assert!(!unchanged.is_increase());
        assert!(!unchanged.is_decrease());
    }
}
