//! Filesystem scanning shared by the daemon and CLI.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use wdu_core::{DirectorySnapshot, FileChangeKind};

pub fn scan_tree(root: &Path) -> Result<Vec<DirectorySnapshot>> {
    let metadata = fs::symlink_metadata(root)
        .with_context(|| format!("failed to inspect directory {}", root.display()))?;
    if !metadata.is_dir() {
        bail!("watch path is not a directory: {}", root.display());
    }

    let (_, snapshots) = scan_directory_tree(root)?;
    Ok(snapshots)
}

pub fn scan_tree_if_present(root: &Path) -> Result<Option<Vec<DirectorySnapshot>>> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => scan_tree(root).map(Some),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to inspect directory {}", root.display()))
        }
    }
}

pub fn affected_directory(watch_root: &Path, path: &Path, kind: FileChangeKind) -> Option<PathBuf> {
    if !is_under(watch_root, path) {
        return None;
    }

    if path == watch_root {
        return Some(watch_root.to_path_buf());
    }

    let directory = match kind {
        FileChangeKind::Removed => path.parent(),
        _ => match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => Some(path),
            _ => path.parent(),
        },
    }?;

    is_under(watch_root, directory).then(|| directory.to_path_buf())
}

fn scan_directory_tree(directory: &Path) -> Result<(u64, Vec<DirectorySnapshot>)> {
    let mut usage_bytes = 0_u64;
    let mut snapshots = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((0, snapshots));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read directory {}", directory.display()));
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| "failed to read directory entry"),
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
            }
        };

        if file_type.is_symlink() {
            continue;
        }

        if file_type.is_dir() {
            let (child_usage_bytes, child_snapshots) = scan_directory_tree(&path)?;
            usage_bytes = usage_bytes
                .checked_add(child_usage_bytes)
                .context("directory usage exceeded u64")?;
            snapshots.extend(child_snapshots);
        } else if file_type.is_file() {
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to inspect {}", path.display()));
                }
            };
            usage_bytes = usage_bytes
                .checked_add(metadata.len())
                .context("directory usage exceeded u64")?;
        }
    }

    snapshots.push(DirectorySnapshot::new(directory.to_path_buf(), usage_bytes));
    Ok((usage_bytes, snapshots))
}

fn is_under(root: &Path, path: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_parent_for_file_events_and_directory_for_directory_events() {
        let root = std::env::temp_dir().join(format!("wdu-usage-{}", std::process::id()));
        let directory = root.join("data");
        let file = directory.join("file.bin");
        std::fs::create_dir_all(&directory).unwrap();

        assert_eq!(
            affected_directory(&root, &file, FileChangeKind::Modified),
            Some(directory.clone())
        );
        assert_eq!(
            affected_directory(&root, &directory, FileChangeKind::Created),
            Some(directory.clone())
        );
        assert_eq!(
            affected_directory(&root, &file, FileChangeKind::Removed),
            Some(directory)
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scans_inclusive_logical_file_size_without_following_symlinks() {
        let root = std::env::temp_dir().join(format!("wdu-scan-{}", std::process::id()));
        let nested = root.join("data").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("root.bin"), b"1234").unwrap();
        std::fs::write(nested.join("nested.bin"), b"123456").unwrap();

        let snapshots = scan_tree(&root).unwrap();
        assert_eq!(
            snapshots
                .iter()
                .find(|snapshot| snapshot.directory == nested)
                .map(|snapshot| snapshot.usage_bytes),
            Some(6)
        );
        assert_eq!(
            snapshots
                .iter()
                .find(|snapshot| snapshot.directory == root)
                .map(|snapshot| snapshot.usage_bytes),
            Some(10)
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("root.bin"), root.join("link.bin")).unwrap();
            let rescanned = scan_tree(&root).unwrap();
            assert_eq!(
                rescanned
                    .iter()
                    .find(|snapshot| snapshot.directory == root)
                    .map(|snapshot| snapshot.usage_bytes),
                Some(10)
            );
        }

        std::fs::remove_dir_all(root).unwrap();
    }
}
