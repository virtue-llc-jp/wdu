mod usage;

use std::path::PathBuf;

#[cfg(target_os = "macos")]
use std::path::Path;

#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use wdu_storage::Store;

#[derive(Debug, Parser)]
#[command(name = "wdu-daemon", about = "Watch macOS file-system changes for wdu")]
struct Args {
    /// Directory to monitor recursively.
    #[arg(value_name = "DIRECTORY", default_value = ".")]
    directory: PathBuf,

    /// SQLite database path.
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let database = match args.database {
        Some(database) => database,
        None => Store::default_database_path()?,
    };
    run(args.directory, database)
}

#[cfg(target_os = "macos")]
fn run(directory: PathBuf, database: PathBuf) -> Result<()> {
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};

    use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
    use usage::scan_tree;

    let directory = std::fs::canonicalize(directory)?;
    ensure_database_outside_watch_root(&directory, &database)?;
    let initial_snapshots = scan_tree(&directory)?;
    let observed_at_unix_secs = unix_now()?;
    let mut store = Store::open(&database)?;
    store.initialize_tree(&directory, &initial_snapshots, observed_at_unix_secs)?;

    let (sender, receiver) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            if let Err(error) = sender.send(result) {
                eprintln!("failed to forward file-system event: {error}");
            }
        },
        Config::default(),
    )?;

    watcher.watch(&directory, RecursiveMode::Recursive)?;
    eprintln!("watching {}", directory.display());
    eprintln!("database {}", database.display());

    while let Ok(result) = receiver.recv() {
        let first_event = result?;
        let mut events = vec![first_event];
        let deadline = Instant::now() + Duration::from_millis(100);

        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            if timeout.is_zero() {
                break;
            }

            match receiver.recv_timeout(timeout) {
                Ok(result) => events.push(result?),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }

        process_event_batch(&directory, &mut store, events)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_database_outside_watch_root(watch_root: &Path, database: &Path) -> Result<()> {
    let watch_root = std::fs::canonicalize(watch_root)?;
    let database_path = if database.exists() {
        std::fs::canonicalize(database)?
    } else {
        let file_name = database
            .file_name()
            .context("database path has no file name")?;
        let parent = database
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::canonicalize(parent)?.join(file_name)
    };

    if database_path == watch_root || database_path.strip_prefix(&watch_root).is_ok() {
        anyhow::bail!(
            "database {} must be outside watch root {}",
            database.display(),
            watch_root.display()
        );
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn process_event_batch(
    watch_root: &std::path::Path,
    store: &mut Store,
    events: Vec<notify::Event>,
) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};

    use usage::affected_directory;
    use wdu_core::{FileChange, FileChangeKind};

    let observed_at_unix_secs = unix_now()?;
    let mut changes = BTreeMap::new();
    for event in events {
        let kind = match event.kind {
            notify::EventKind::Create(_) => FileChangeKind::Created,
            notify::EventKind::Modify(_) => FileChangeKind::Modified,
            notify::EventKind::Remove(_) => FileChangeKind::Removed,
            _ => FileChangeKind::Other,
        };
        for path in event.paths {
            changes.insert(path, kind);
        }
    }

    let mut affected_directories = BTreeSet::new();
    for (path, kind) in &changes {
        if *kind == FileChangeKind::Removed && store.remove_path(path, observed_at_unix_secs)? {
            continue;
        }

        if let Some(directory) = affected_directory(watch_root, path, *kind) {
            affected_directories.insert(directory);
        }
    }

    let mut scan_roots = Vec::new();
    for directory in affected_directories {
        if scan_roots
            .iter()
            .any(|root: &std::path::PathBuf| directory.strip_prefix(root).is_ok())
        {
            continue;
        }
        scan_roots.push(directory);
    }

    for directory in scan_roots {
        reconcile_directory(watch_root, store, &directory, observed_at_unix_secs)?;
    }

    for (path, kind) in changes {
        let change = FileChange::new(path, kind, observed_at_unix_secs);
        println!("{}", serde_json::to_string(&change)?);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn reconcile_directory(
    watch_root: &std::path::Path,
    store: &mut Store,
    directory: &std::path::Path,
    observed_at_unix_secs: u64,
) -> Result<()> {
    use usage::scan_tree_if_present;

    let mut scope = directory;
    loop {
        if let Some(snapshots) = scan_tree_if_present(scope)? {
            store.synchronize_tree(scope, &snapshots, observed_at_unix_secs)?;
            return Ok(());
        }

        if scope == watch_root {
            return Ok(());
        }
        scope = scope.parent().context("affected directory has no parent")?;
    }
}

#[cfg(not(target_os = "macos"))]
fn run(_directory: PathBuf, _database: PathBuf) -> Result<()> {
    anyhow::bail!("wdu-daemon currently supports macOS only")
}

#[cfg(target_os = "macos")]
fn unix_now() -> Result<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};

    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn rejects_database_inside_watch_root() {
        let root = std::env::temp_dir().join(format!("wdu-daemon-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        let result = ensure_database_outside_watch_root(&root, &root.join("wdu.sqlite3"));
        assert!(result.is_err());

        std::fs::remove_dir_all(root).unwrap();
    }
}
