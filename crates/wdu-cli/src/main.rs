use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use wdu_config::{Config, LoadedConfig};
use wdu_storage::Store;
use wdu_usage::scan_tree;

#[derive(Debug, Parser)]
#[command(
    name = "wdu",
    about = "Query cumulative directory disk-usage changes recorded by wdu"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Query the cumulative usage change below a directory.
    Query(QueryArgs),
    /// Measure a directory now and record its current usage.
    Record(RecordArgs),
}

#[derive(Debug, Args)]
struct QueryArgs {
    /// Directory to query.
    #[arg(short, long, value_name = "DIRECTORY")]
    directory: PathBuf,

    /// SQLite database path.
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,

    /// TOML configuration path.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Include buckets overlapping this Unix timestamp.
    #[arg(long)]
    since: Option<u64>,

    /// Include buckets ending at or before this Unix timestamp.
    #[arg(long)]
    until: Option<u64>,
}

#[derive(Debug, Args)]
struct RecordArgs {
    /// Directory to measure.
    #[arg(short, long, value_name = "DIRECTORY")]
    directory: PathBuf,

    /// SQLite database path.
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,

    /// TOML configuration path.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct RangeQueryOutput {
    directory: PathBuf,
    current_usage_bytes: u64,
    cumulative_delta_bytes: i64,
    delta_bytes: i64,
    is_present: bool,
    observed_at_unix_secs: u64,
    since_unix_secs: u64,
    until_unix_secs: Option<u64>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Query(args) => query(args),
        Command::Record(args) => record(args),
    }
}

fn query(args: QueryArgs) -> Result<()> {
    if args.until.is_some() && args.since.is_none() {
        bail!("--until requires --since");
    }
    let loaded_config = LoadedConfig::load(args.config.as_deref())?;
    let database = resolve_database(&loaded_config.config, args.database)?;
    let directory = query_path(args.directory)?;
    let store = Store::open_with_history(
        &database,
        loaded_config.config.hourly_bucket_secs,
        loaded_config.config.hourly_retention_secs,
    )?;
    let Some(aggregate) = store.get_aggregate(&directory)? else {
        bail!("no usage aggregate found for {}", directory.display());
    };

    if let Some(since_unix_secs) = args.since {
        let delta_bytes = store.query_delta_since(&directory, since_unix_secs, args.until)?;
        let output = RangeQueryOutput {
            directory: aggregate.directory.clone(),
            current_usage_bytes: aggregate.current_usage_bytes,
            cumulative_delta_bytes: aggregate.cumulative_delta_bytes,
            delta_bytes,
            is_present: aggregate.is_present,
            observed_at_unix_secs: aggregate.observed_at_unix_secs,
            since_unix_secs,
            until_unix_secs: args.until,
        };
        println!("{}", serde_json::to_string(&output)?);
    } else {
        println!("{}", serde_json::to_string(&aggregate)?);
    }
    Ok(())
}

fn record(args: RecordArgs) -> Result<()> {
    let loaded_config = LoadedConfig::load(args.config.as_deref())?;
    let database = resolve_database(&loaded_config.config, args.database)?;
    let directory = std::fs::canonicalize(&args.directory)
        .with_context(|| format!("failed to resolve {}", args.directory.display()))?;
    let snapshots = scan_tree(&directory)?;
    let observed_at_unix_secs = unix_now()?;
    let mut store = Store::open_with_history(
        &database,
        loaded_config.config.hourly_bucket_secs,
        loaded_config.config.hourly_retention_secs,
    )?;
    store.initialize_tree(&directory, &snapshots, observed_at_unix_secs)?;
    store.compact_history(observed_at_unix_secs)?;

    let aggregate = store
        .get_aggregate(&directory)?
        .context("recorded directory aggregate is missing")?;
    println!("{}", serde_json::to_string(&aggregate)?);
    Ok(())
}

fn resolve_database(config: &Config, cli_database: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(database) = cli_database {
        return Ok(database);
    }
    if let Some(database) = std::env::var_os("WDU_DATABASE") {
        return Ok(PathBuf::from(database));
    }
    if let Some(database) = &config.database {
        return Ok(database.clone());
    }
    Store::default_database_path()
}

fn unix_now() -> Result<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};

    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn query_path(path: PathBuf) -> Result<PathBuf> {
    match std::fs::canonicalize(&path) {
        Ok(canonical_path) => Ok(canonical_path),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let current_directory =
                std::env::current_dir().context("failed to get current directory")?;
            let absolute_path = if path.is_absolute() {
                path
            } else {
                current_directory.join(path)
            };
            let file_name = absolute_path
                .file_name()
                .context("query path has no file name")?;
            let parent = absolute_path.parent().context("query path has no parent")?;
            Ok(std::fs::canonicalize(parent)?.join(file_name))
        }
        Err(error) => Err(error).with_context(|| format!("failed to resolve {}", path.display())),
    }
}
