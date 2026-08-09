//! Shared configuration loading for the daemon and CLI.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const DEFAULT_HOURLY_BUCKET_SECS: u64 = 3_600;
pub const DEFAULT_HOURLY_RETENTION_SECS: u64 = 604_800;
pub const DEFAULT_COMPACTION_INTERVAL_SECS: u64 = 3_600;

/// Settings shared by the daemon and CLI.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    pub database: Option<PathBuf>,
    pub watch_root: Option<PathBuf>,
    #[serde(default = "default_hourly_bucket_secs")]
    pub hourly_bucket_secs: u64,
    #[serde(default = "default_hourly_retention_secs")]
    pub hourly_retention_secs: u64,
    #[serde(default = "default_compaction_interval_secs")]
    pub compaction_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: None,
            watch_root: None,
            hourly_bucket_secs: DEFAULT_HOURLY_BUCKET_SECS,
            hourly_retention_secs: DEFAULT_HOURLY_RETENTION_SECS,
            compaction_interval_secs: DEFAULT_COMPACTION_INTERVAL_SECS,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.hourly_bucket_secs == 0 {
            bail!("hourly_bucket_secs must be greater than zero");
        }
        if self.hourly_retention_secs < self.hourly_bucket_secs {
            bail!("hourly_retention_secs must be at least hourly_bucket_secs");
        }
        if self.compaction_interval_secs == 0 {
            bail!("compaction_interval_secs must be greater than zero");
        }
        Ok(())
    }
}

/// Configuration together with the file it was loaded from, if any.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
}

impl LoadedConfig {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let (path, explicit) = match explicit_path {
            Some(path) => (Some(path.to_path_buf()), true),
            None => match env::var_os("WDU_CONFIG") {
                Some(path) => (Some(PathBuf::from(path)), true),
                None => (find_default_config_path(), false),
            },
        };

        let Some(path) = path else {
            return Ok(Self {
                config: Config::default(),
                path: None,
            });
        };

        if !path.exists() && !explicit {
            return Ok(Self {
                config: Config::default(),
                path: None,
            });
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let mut config: Config = toml::from_str(&contents)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config.database = resolve_relative_path(config.database, &path);
        config.watch_root = resolve_relative_path(config.watch_root, &path);
        config.validate()?;

        Ok(Self {
            config,
            path: Some(path),
        })
    }
}

fn find_default_config_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(prefix) = env::var_os("HOMEBREW_PREFIX") {
        candidates.push(PathBuf::from(prefix).join("etc/wdu/config.toml"));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/etc/wdu/config.toml"),
        PathBuf::from("/usr/local/etc/wdu/config.toml"),
    ]);
    if let Some(home) = env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Library/Application Support/wdu/config.toml"));
    }

    candidates.into_iter().find(|path| path.is_file())
}

fn resolve_relative_path(path: Option<PathBuf>, config_path: &Path) -> Option<PathBuf> {
    path.map(|path| {
        if path.is_absolute() {
            path
        } else {
            config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(path)
        }
    })
}

fn default_hourly_bucket_secs() -> u64 {
    DEFAULT_HOURLY_BUCKET_SECS
}

fn default_hourly_retention_secs() -> u64 {
    DEFAULT_HOURLY_RETENTION_SECS
}

fn default_compaction_interval_secs() -> u64 {
    DEFAULT_COMPACTION_INTERVAL_SECS
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn loads_defaults_without_a_config_file() {
        let loaded = LoadedConfig::load(Some(Path::new("/tmp/does-not-exist/wdu.toml")));
        assert!(loaded.is_err());
        assert_eq!(Config::default().hourly_bucket_secs, 3_600);
    }

    #[test]
    fn resolves_relative_paths_and_defaults() {
        let directory = tempfile_directory();
        let path = directory.join("config.toml");
        fs::write(
            &path,
            "database = \"var/wdu.sqlite3\"\nwatch_root = \"data\"\n",
        )
        .unwrap();

        let loaded = LoadedConfig::load(Some(&path)).unwrap();
        assert_eq!(loaded.path, Some(path.clone()));
        assert_eq!(
            loaded.config.database,
            Some(directory.join("var/wdu.sqlite3"))
        );
        assert_eq!(loaded.config.watch_root, Some(directory.join("data")));
        assert_eq!(
            loaded.config.compaction_interval_secs,
            DEFAULT_COMPACTION_INTERVAL_SECS
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_invalid_intervals() {
        let directory = tempfile_directory();
        let path = directory.join("config.toml");
        fs::write(&path, "hourly_bucket_secs = 0\n").unwrap();

        assert!(LoadedConfig::load(Some(&path)).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    fn tempfile_directory() -> PathBuf {
        let directory = env::temp_dir().join(format!(
            "wdu-config-{}-{}",
            std::process::id(),
            NEXT_TEMP_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        directory
    }
}
