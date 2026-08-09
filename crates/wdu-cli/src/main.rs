use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use wdu_storage::Store;

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
}

#[derive(Debug, Args)]
struct QueryArgs {
    /// Directory to query.
    #[arg(short, long, value_name = "DIRECTORY")]
    directory: PathBuf,

    /// SQLite database path.
    #[arg(long, value_name = "PATH")]
    database: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Query(args) => query(args),
    }
}

fn query(args: QueryArgs) -> Result<()> {
    let database = match args.database {
        Some(database) => database,
        None => Store::default_database_path()?,
    };
    let directory = query_path(args.directory)?;
    let store = Store::open(&database)?;
    let Some(aggregate) = store.get_aggregate(&directory)? else {
        bail!("no usage aggregate found for {}", directory.display());
    };

    println!("{}", serde_json::to_string(&aggregate)?);
    Ok(())
}

fn query_path(path: PathBuf) -> Result<PathBuf> {
    match std::fs::canonicalize(&path) {
        Ok(canonical_path) => Ok(canonical_path),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let current_directory =
                std::env::current_dir().context("failed to get current directory")?;
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(current_directory.join(path))
            }
        }
        Err(error) => Err(error).with_context(|| format!("failed to resolve {}", path.display())),
    }
}
