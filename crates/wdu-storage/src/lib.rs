//! SQLite persistence for cumulative directory usage aggregates.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use wdu_core::DirectoryUsageAggregate;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS store_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS directory_usage (
    path TEXT PRIMARY KEY,
    parent_path TEXT REFERENCES directory_usage(path),
    current_usage_bytes INTEGER NOT NULL CHECK (current_usage_bytes >= 0),
    cumulative_delta_bytes INTEGER NOT NULL,
    is_present INTEGER NOT NULL CHECK (is_present IN (0, 1)),
    observed_at_unix_secs INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS directory_usage_parent_path_idx
    ON directory_usage(parent_path);
"#;

const WATCH_ROOT_KEY: &str = "watch_root";
const SCHEMA_VERSION: i64 = 1;

/// A recursively measured directory and its inclusive logical byte usage.
#[derive(Debug, Clone, Eq, PartialEq)]
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

/// Persistent store for materialized directory usage aggregates.
pub struct Store {
    connection: Connection,
    watch_root: Option<String>,
}

impl Store {
    /// Opens or creates a SQLite database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create database directory {}", parent.display())
            })?;
        }

        let connection = Connection::open(path)
            .with_context(|| format!("failed to open database {}", path.display()))?;
        Self::from_connection(connection)
    }

    /// Opens an in-memory store for tests and embedders.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Returns the default per-user database path.
    pub fn default_database_path() -> Result<PathBuf> {
        if let Some(path) = env::var_os("WDU_DATABASE") {
            return Ok(PathBuf::from(path));
        }

        let home = env::var_os("HOME").context("HOME is not set")?;

        #[cfg(target_os = "macos")]
        let base_directory = PathBuf::from(home)
            .join("Library")
            .join("Application Support");

        #[cfg(not(target_os = "macos"))]
        let base_directory = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".local").join("state"));

        Ok(base_directory.join("wdu").join("wdu.sqlite3"))
    }

    /// Seeds a new watch root or reconciles an existing one with a full scan.
    pub fn initialize_tree(
        &mut self,
        root: &Path,
        snapshots: &[DirectorySnapshot],
        observed_at_unix_secs: u64,
    ) -> Result<()> {
        let root_key = path_key(root)?;
        validate_snapshots(&root_key, snapshots)?;
        if !snapshots.iter().any(|snapshot| snapshot.directory == root) {
            bail!(
                "directory scan did not include watch root {}",
                root.display()
            );
        }

        let stored_root = self.watch_root.clone();
        if let Some(stored_root) = stored_root.as_deref()
            && stored_root != root_key
        {
            bail!(
                "database is already associated with watch root {stored_root}, not {}",
                root.display()
            );
        }

        let transaction = self.connection.transaction()?;
        let root_exists = directory_exists(&transaction, &root_key)?;

        if stored_root.is_none() {
            if root_exists {
                bail!("database metadata is missing for existing directory data");
            }

            transaction.execute(
                "INSERT INTO store_metadata (key, value) VALUES (?1, ?2)",
                params![WATCH_ROOT_KEY, root_key],
            )?;
            seed_tree(&transaction, &root_key, snapshots, observed_at_unix_secs)?;
        } else {
            synchronize_tree(
                &transaction,
                &root_key,
                &root_key,
                snapshots,
                observed_at_unix_secs,
            )?;
        }

        transaction.commit()?;
        self.watch_root = Some(root_key);
        Ok(())
    }

    /// Reconciles a recursively scanned directory with its stored subtree.
    pub fn synchronize_tree(
        &mut self,
        directory: &Path,
        snapshots: &[DirectorySnapshot],
        observed_at_unix_secs: u64,
    ) -> Result<()> {
        let root_key = self
            .watch_root
            .clone()
            .context("store must be initialized before synchronizing a tree")?;
        let directory_key = path_key(directory)?;
        validate_snapshots(&directory_key, snapshots)?;

        let transaction = self.connection.transaction()?;
        synchronize_tree(
            &transaction,
            &root_key,
            &directory_key,
            snapshots,
            observed_at_unix_secs,
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Applies one inclusive directory observation and returns its delta.
    pub fn observe_directory(
        &mut self,
        directory: &Path,
        current_usage_bytes: u64,
        is_present: bool,
        observed_at_unix_secs: u64,
    ) -> Result<i64> {
        let root_key = self
            .watch_root
            .clone()
            .context("store must be initialized before recording observations")?;
        let directory_key = path_key(directory)?;
        let transaction = self.connection.transaction()?;
        let delta = observe_directory_tx(
            &transaction,
            &root_key,
            &directory_key,
            current_usage_bytes,
            is_present,
            observed_at_unix_secs,
        )?;
        transaction.commit()?;
        Ok(delta)
    }

    /// Marks a stored directory subtree as removed.
    pub fn remove_path(&mut self, path: &Path, observed_at_unix_secs: u64) -> Result<bool> {
        let root_key = self
            .watch_root
            .clone()
            .context("store must be initialized before recording removals")?;
        let path_key = path_key(path)?;
        let transaction = self.connection.transaction()?;
        let mut paths = paths_under(&transaction, &path_key)?;
        if paths.is_empty() {
            transaction.commit()?;
            return Ok(false);
        }

        paths.sort_by_key(|stored_path| Reverse(path_depth(Path::new(stored_path))));
        for stored_path in paths {
            observe_directory_tx(
                &transaction,
                &root_key,
                &stored_path,
                0,
                false,
                observed_at_unix_secs,
            )?;
        }

        transaction.commit()?;
        Ok(true)
    }

    /// Reads one materialized aggregate by canonical path.
    pub fn get_aggregate(&self, directory: &Path) -> Result<Option<DirectoryUsageAggregate>> {
        let directory_key = path_key(directory)?;
        let row = self
            .connection
            .query_row(
                "SELECT current_usage_bytes, cumulative_delta_bytes, is_present, observed_at_unix_secs
                 FROM directory_usage WHERE path = ?1",
                params![directory_key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;

        let Some((current_usage_bytes, cumulative_delta_bytes, is_present, observed_at)) = row
        else {
            return Ok(None);
        };

        let current_usage_bytes = u64::try_from(current_usage_bytes)
            .context("stored current usage is outside the u64 range")?;
        let is_present = match is_present {
            0 => false,
            1 => true,
            value => bail!("stored is_present value is invalid: {value}"),
        };
        let observed_at_unix_secs = u64::try_from(observed_at)
            .context("stored observation time is outside the u64 range")?;

        Ok(Some(DirectoryUsageAggregate::new(
            PathBuf::from(directory_key),
            current_usage_bytes,
            cumulative_delta_bytes,
            is_present,
            observed_at_unix_secs,
        )))
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;",
        )?;

        let schema_version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match schema_version {
            0 => {
                connection.execute_batch(SCHEMA)?;
                connection.execute_batch("PRAGMA user_version = 1;")?;
            }
            SCHEMA_VERSION => connection.execute_batch(SCHEMA)?,
            version => bail!("unsupported SQLite schema version: {version}"),
        }

        let watch_root = connection
            .query_row(
                "SELECT value FROM store_metadata WHERE key = ?1",
                params![WATCH_ROOT_KEY],
                |row| row.get(0),
            )
            .optional()?;

        Ok(Self {
            connection,
            watch_root,
        })
    }
}

fn seed_tree(
    transaction: &Transaction<'_>,
    root_key: &str,
    snapshots: &[DirectorySnapshot],
    observed_at_unix_secs: u64,
) -> Result<()> {
    let mut ordered_snapshots = snapshots.to_vec();
    ordered_snapshots.sort_by_key(|snapshot| path_depth(&snapshot.directory));

    for snapshot in ordered_snapshots {
        let directory_key = path_key(&snapshot.directory)?;
        let parent_key = parent_key(&directory_key, root_key)?;
        let current_usage_bytes = sqlite_bytes(snapshot.usage_bytes)?;
        let observed_at_unix_secs = sqlite_timestamp(observed_at_unix_secs)?;

        transaction.execute(
            "INSERT INTO directory_usage
             (path, parent_path, current_usage_bytes, cumulative_delta_bytes, is_present, observed_at_unix_secs)
             VALUES (?1, ?2, ?3, 0, 1, ?4)",
            params![directory_key, parent_key, current_usage_bytes, observed_at_unix_secs],
        )?;
    }

    Ok(())
}

fn synchronize_tree(
    transaction: &Transaction<'_>,
    root_key: &str,
    scope_key: &str,
    snapshots: &[DirectorySnapshot],
    observed_at_unix_secs: u64,
) -> Result<()> {
    let mut snapshot_keys = HashSet::new();
    for snapshot in snapshots {
        let directory_key = path_key(&snapshot.directory)?;
        if !is_under(scope_key, &directory_key) {
            bail!(
                "snapshot {} is outside scope {scope_key}",
                snapshot.directory.display()
            );
        }
        sqlite_bytes(snapshot.usage_bytes)?;
        snapshot_keys.insert(directory_key);
    }

    let mut stale_paths = paths_under(transaction, scope_key)?;
    stale_paths.retain(|stored_path| !snapshot_keys.contains(stored_path));
    stale_paths.sort_by_key(|stored_path| Reverse(path_depth(Path::new(stored_path))));
    for stale_path in stale_paths {
        observe_directory_tx(
            transaction,
            root_key,
            &stale_path,
            0,
            false,
            observed_at_unix_secs,
        )?;
    }

    let mut ordered_snapshots = snapshots.to_vec();
    ordered_snapshots.sort_by_key(|snapshot| Reverse(path_depth(&snapshot.directory)));
    for snapshot in ordered_snapshots {
        let directory_key = path_key(&snapshot.directory)?;
        observe_directory_tx(
            transaction,
            root_key,
            &directory_key,
            snapshot.usage_bytes,
            true,
            observed_at_unix_secs,
        )?;
    }

    Ok(())
}

fn observe_directory_tx(
    transaction: &Transaction<'_>,
    root_key: &str,
    directory_key: &str,
    current_usage_bytes: u64,
    is_present: bool,
    observed_at_unix_secs: u64,
) -> Result<i64> {
    if !is_under(root_key, directory_key) {
        bail!("directory {directory_key} is outside watch root {root_key}");
    }

    let current_usage_bytes = sqlite_bytes(current_usage_bytes)?;
    let observed_at_unix_secs = sqlite_timestamp(observed_at_unix_secs)?;
    ensure_path_rows(transaction, root_key, directory_key)?;

    let (previous_usage_bytes, previous_cumulative_delta_bytes) = transaction.query_row(
        "SELECT current_usage_bytes, cumulative_delta_bytes
         FROM directory_usage WHERE path = ?1",
        params![directory_key],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let delta_bytes = current_usage_bytes
        .checked_sub(previous_usage_bytes)
        .context("directory usage delta overflowed i64")?;
    previous_cumulative_delta_bytes
        .checked_add(delta_bytes)
        .context("directory cumulative delta overflowed i64")?;

    for ancestor_key in ancestor_keys(transaction, directory_key)? {
        let (ancestor_usage_bytes, ancestor_cumulative_delta_bytes) = transaction.query_row(
            "SELECT current_usage_bytes, cumulative_delta_bytes
             FROM directory_usage WHERE path = ?1",
            params![ancestor_key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        let new_usage_bytes = ancestor_usage_bytes
            .checked_add(delta_bytes)
            .context("ancestor directory usage overflowed i64")?;
        if new_usage_bytes < 0 {
            bail!("directory usage became negative for {ancestor_key}");
        }
        ancestor_cumulative_delta_bytes
            .checked_add(delta_bytes)
            .context("ancestor cumulative delta overflowed i64")?;
    }

    transaction.execute(
        "WITH RECURSIVE ancestors(path) AS (
             SELECT ?1
             UNION ALL
             SELECT directory.parent_path
             FROM directory_usage AS directory
             JOIN ancestors ON directory.path = ancestors.path
             WHERE directory.parent_path IS NOT NULL
         )
         UPDATE directory_usage
         SET current_usage_bytes = current_usage_bytes + ?2,
             cumulative_delta_bytes = cumulative_delta_bytes + ?2,
             observed_at_unix_secs = ?3
         WHERE path IN (SELECT path FROM ancestors)",
        params![directory_key, delta_bytes, observed_at_unix_secs],
    )?;
    transaction.execute(
        "UPDATE directory_usage SET is_present = ?1 WHERE path = ?2",
        params![if is_present { 1 } else { 0 }, directory_key],
    )?;

    Ok(delta_bytes)
}

fn ensure_path_rows(
    transaction: &Transaction<'_>,
    root_key: &str,
    directory_key: &str,
) -> Result<()> {
    let mut missing = Vec::new();
    let mut current_path = PathBuf::from(directory_key);

    loop {
        let current_key = path_key(&current_path)?;
        if !is_under(root_key, &current_key) {
            bail!("directory {directory_key} is outside watch root {root_key}");
        }
        if directory_exists(transaction, &current_key)? {
            break;
        }

        let parent = if current_key == root_key {
            None
        } else {
            let parent = current_path
                .parent()
                .context("directory path has no parent")?;
            Some(path_key(parent)?)
        };
        missing.push((current_key.clone(), parent));

        if current_key == root_key {
            break;
        }
        current_path = current_path
            .parent()
            .context("directory path has no parent")?
            .to_path_buf();
    }

    for (current_key, parent_key) in missing.into_iter().rev() {
        transaction.execute(
            "INSERT OR IGNORE INTO directory_usage
             (path, parent_path, current_usage_bytes, cumulative_delta_bytes, is_present, observed_at_unix_secs)
             VALUES (?1, ?2, 0, 0, 0, 0)",
            params![current_key, parent_key],
        )?;
    }

    Ok(())
}

fn ancestor_keys(transaction: &Transaction<'_>, directory_key: &str) -> Result<Vec<String>> {
    let mut ancestors = Vec::new();
    let mut seen = HashSet::new();
    let mut current_key = directory_key.to_owned();

    loop {
        if !seen.insert(current_key.clone()) {
            bail!("directory hierarchy contains a cycle at {current_key}");
        }

        let parent_key = transaction.query_row(
            "SELECT parent_path FROM directory_usage WHERE path = ?1",
            params![current_key],
            |row| row.get::<_, Option<String>>(0),
        )?;
        ancestors.push(current_key);

        let Some(parent_key) = parent_key else {
            break;
        };
        current_key = parent_key;
    }

    Ok(ancestors)
}

fn paths_under(transaction: &Transaction<'_>, scope_key: &str) -> Result<Vec<String>> {
    let mut statement = transaction.prepare(
        "WITH RECURSIVE descendants(path) AS (
             SELECT path FROM directory_usage WHERE path = ?1
             UNION ALL
             SELECT child.path
             FROM directory_usage AS child
             JOIN descendants AS parent ON child.parent_path = parent.path
         )
         SELECT path FROM descendants",
    )?;
    let paths = statement
        .query_map(params![scope_key], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(paths)
}

fn directory_exists(transaction: &Transaction<'_>, directory_key: &str) -> Result<bool> {
    Ok(transaction
        .query_row(
            "SELECT 1 FROM directory_usage WHERE path = ?1",
            params![directory_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

fn validate_snapshots(root_key: &str, snapshots: &[DirectorySnapshot]) -> Result<()> {
    for snapshot in snapshots {
        let directory_key = path_key(&snapshot.directory)?;
        if !is_under(root_key, &directory_key) {
            bail!(
                "snapshot {} is outside scope {root_key}",
                snapshot.directory.display()
            );
        }
        sqlite_bytes(snapshot.usage_bytes)?;
    }
    Ok(())
}

fn path_key(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .context("path is not valid UTF-8 and cannot be stored in SQLite TEXT")
}

fn parent_key(directory_key: &str, root_key: &str) -> Result<Option<String>> {
    if directory_key == root_key {
        return Ok(None);
    }

    let parent = Path::new(directory_key)
        .parent()
        .context("directory path has no parent")?;
    let parent_key = path_key(parent)?;
    if !is_under(root_key, &parent_key) {
        bail!("directory {directory_key} is outside watch root {root_key}");
    }
    Ok(Some(parent_key))
}

fn is_under(root_key: &str, path_key: &str) -> bool {
    let root = Path::new(root_key);
    let path = Path::new(path_key);
    path == root || path.strip_prefix(root).is_ok()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

fn sqlite_bytes(bytes: u64) -> Result<i64> {
    i64::try_from(bytes).context("directory usage is too large for SQLite INTEGER")
}

fn sqlite_timestamp(timestamp: u64) -> Result<i64> {
    i64::try_from(timestamp).context("observation time is too large for SQLite INTEGER")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(path: &str, usage_bytes: u64) -> DirectorySnapshot {
        DirectorySnapshot::new(PathBuf::from(path), usage_bytes)
    }

    fn seeded_store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store
            .initialize_tree(
                Path::new("/watch"),
                &[
                    snapshot("/watch", 200),
                    snapshot("/watch/data", 100),
                    snapshot("/watch/data/nested", 40),
                ],
                1,
            )
            .unwrap();
        store
    }

    #[test]
    fn propagates_child_delta_to_all_ancestors() {
        let mut store = seeded_store();

        assert_eq!(
            store
                .observe_directory(Path::new("/watch/data/nested"), 70, true, 2)
                .unwrap(),
            30
        );

        let nested = store
            .get_aggregate(Path::new("/watch/data/nested"))
            .unwrap()
            .unwrap();
        let data = store
            .get_aggregate(Path::new("/watch/data"))
            .unwrap()
            .unwrap();
        let root = store.get_aggregate(Path::new("/watch")).unwrap().unwrap();

        assert_eq!(nested.cumulative_delta_bytes, 30);
        assert_eq!(data.cumulative_delta_bytes, 30);
        assert_eq!(root.cumulative_delta_bytes, 30);
        assert_eq!(root.current_usage_bytes, 230);
    }

    #[test]
    fn duplicate_observation_does_not_change_cumulative_values() {
        let mut store = seeded_store();

        store
            .observe_directory(Path::new("/watch/data"), 125, true, 2)
            .unwrap();
        store
            .observe_directory(Path::new("/watch/data"), 125, true, 3)
            .unwrap();

        let root = store.get_aggregate(Path::new("/watch")).unwrap().unwrap();
        assert_eq!(root.cumulative_delta_bytes, 25);
        assert_eq!(root.current_usage_bytes, 225);
    }

    #[test]
    fn removing_subtree_updates_parent_and_keeps_tombstones() {
        let mut store = seeded_store();

        assert!(store.remove_path(Path::new("/watch/data"), 2).unwrap());

        let data = store
            .get_aggregate(Path::new("/watch/data"))
            .unwrap()
            .unwrap();
        let nested = store
            .get_aggregate(Path::new("/watch/data/nested"))
            .unwrap()
            .unwrap();
        let root = store.get_aggregate(Path::new("/watch")).unwrap().unwrap();

        assert!(!data.is_present);
        assert!(!nested.is_present);
        assert_eq!(data.current_usage_bytes, 0);
        assert_eq!(data.cumulative_delta_bytes, -100);
        assert_eq!(nested.cumulative_delta_bytes, -40);
        assert_eq!(root.current_usage_bytes, 100);
        assert_eq!(root.cumulative_delta_bytes, -100);
    }

    #[test]
    fn synchronizing_tree_reconciles_rename_without_changing_parent_total() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .initialize_tree(
                Path::new("/watch"),
                &[
                    snapshot("/watch", 10),
                    snapshot("/watch/data", 10),
                    snapshot("/watch/data/old.bin", 10),
                ],
                1,
            )
            .unwrap();

        store
            .synchronize_tree(
                Path::new("/watch"),
                &[
                    snapshot("/watch", 10),
                    snapshot("/watch/data", 10),
                    snapshot("/watch/data/new.bin", 10),
                ],
                2,
            )
            .unwrap();

        let old = store
            .get_aggregate(Path::new("/watch/data/old.bin"))
            .unwrap()
            .unwrap();
        let new = store
            .get_aggregate(Path::new("/watch/data/new.bin"))
            .unwrap()
            .unwrap();
        let root = store.get_aggregate(Path::new("/watch")).unwrap().unwrap();

        assert!(!old.is_present);
        assert_eq!(old.current_usage_bytes, 0);
        assert_eq!(old.cumulative_delta_bytes, -10);
        assert!(new.is_present);
        assert_eq!(new.current_usage_bytes, 10);
        assert_eq!(new.cumulative_delta_bytes, 10);
        assert_eq!(root.current_usage_bytes, 10);
        assert_eq!(root.cumulative_delta_bytes, 0);
    }
}
