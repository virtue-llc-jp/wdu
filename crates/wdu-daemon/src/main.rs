mod usage;

use std::path::PathBuf;

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
    use std::sync::mpsc;

    use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use usage::{affected_directory, scan_tree, scan_tree_if_present};
    use wdu_core::{FileChange, FileChangeKind};

    let directory = std::fs::canonicalize(directory)?;
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

    for result in receiver {
        let event = result?;
        let kind = match event.kind {
            EventKind::Create(_) => FileChangeKind::Created,
            EventKind::Modify(_) => FileChangeKind::Modified,
            EventKind::Remove(_) => FileChangeKind::Removed,
            _ => FileChangeKind::Other,
        };
        let observed_at_unix_secs = unix_now()?;

        for path in event.paths {
            let change = FileChange::new(path, kind, observed_at_unix_secs);

            if kind == FileChangeKind::Removed
                && store.remove_path(&change.path, observed_at_unix_secs)?
            {
                println!("{}", serde_json::to_string(&change)?);
                continue;
            }

            let Some(affected) = affected_directory(&directory, &change.path, kind) else {
                continue;
            };
            if let Some(snapshots) = scan_tree_if_present(&affected)? {
                store.synchronize_tree(&affected, &snapshots, observed_at_unix_secs)?;
            } else if affected != directory
                && let Some(parent) = affected.parent()
                && let Some(snapshots) = scan_tree_if_present(parent)?
            {
                store.synchronize_tree(parent, &snapshots, observed_at_unix_secs)?;
            }

            println!("{}", serde_json::to_string(&change)?);
        }
    }

    Ok(())
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
