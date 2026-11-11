//! `cadocs backup` and `cadocs restore` (`docs/OPERATIONS.md`). A backup is a folder holding
//! `cadocs.db`, written by `VACUUM INTO`, and a copy of `files/` when attachments are on disk.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use cadocs_store::SqliteStore;

use crate::data_dir;

const DATABASE: &str = "cadocs.db";
const FILES: &str = "files";
/// What a copy is called until it is complete. A run cut short leaves one, which the next run
/// replaces and no copy ever carries over.
const PARTIAL: &str = ".partial";

/// What a run did, for its last line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Copied {
    /// Files written, those already there with the same size left out.
    pub files: u64,
}

/// Writes a backup of `data_dir` into `to`, which may hold an earlier one, beside a running
/// server or a stopped one. Files are copied before and after the database: a file the database
/// points to was either there for the first pass, or added since and copied by the second. A file
/// newer than the database is at worst an orphan, which the cleanup job removes. Only a file
/// uploaded between the first pass and the database copy, then deleted before the second, is
/// missed. `local_files` is false with S3, whose files are backed up on the S3 side.
pub async fn backup(data_dir: &Path, to: &Path, local_files: bool) -> anyhow::Result<Copied> {
    let data = canonical(data_dir)?;
    let target = canonical_to_be(to)?;
    if target.starts_with(&data) || data.starts_with(&target) {
        bail!(
            "a backup and the data folder {} cannot hold each other",
            data.display()
        );
    }
    make_dirs(to)?;

    let mut copied = Copied::default();
    if local_files {
        copied.files = copy_files(&data_dir.join(FILES), &to.join(FILES))?;
    }
    let partial = to.join(format!("{DATABASE}{PARTIAL}"));
    remove_if_present(&partial)?;
    let source = data_dir.join(DATABASE);
    SqliteStore::write_backup(&source, &partial)
        .await
        .with_context(|| format!("cannot back up {}", source.display()))?;
    sync(&partial)?;
    publish(&partial, &to.join(DATABASE))?;
    if local_files {
        copied.files += copy_files(&data_dir.join(FILES), &to.join(FILES))?;
    }
    Ok(copied)
}

/// Restores the backup in `from` into `data_dir`, which must hold no database, on a stopped
/// server: it takes the lock `serve` takes. The database is checked first, and moved in last, so
/// a run cut short leaves no database and can simply be repeated.
pub async fn restore(from: &Path, data_dir: &Path, local_files: bool) -> anyhow::Result<Copied> {
    let source = from.join(DATABASE);
    if !source.is_file() {
        bail!(
            "{} holds no backup: {} is missing",
            from.display(),
            source.display()
        );
    }
    let _lock = data_dir::lock(data_dir)?;
    for name in [DATABASE, "cadocs.db-wal", "cadocs.db-shm"] {
        let path = data_dir.join(name);
        if fs::symlink_metadata(&path).is_ok() {
            bail!(
                "{} already holds a database ({name}): restore into an empty data folder",
                data_dir.display()
            );
        }
    }

    let partial = data_dir.join(format!("{DATABASE}{PARTIAL}"));
    copy_one(&source, &partial)?;
    if let Err(error) = SqliteStore::check_backup(&partial).await {
        remove_if_present(&partial)?;
        return Err(error).with_context(|| format!("cannot restore {}", source.display()));
    }

    let mut copied = Copied::default();
    if local_files {
        copied.files = copy_files(&from.join(FILES), &data_dir.join(FILES))?;
    }
    publish(&partial, &data_dir.join(DATABASE))?;
    Ok(copied)
}

fn canonical(path: &Path) -> anyhow::Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("cannot open {}", path.display()))
}

/// Where `path` is or would be: its nearest existing ancestor, resolved, then the rest.
fn canonical_to_be(path: &Path) -> anyhow::Result<PathBuf> {
    let path =
        std::path::absolute(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut missing = Vec::new();
    let mut existing = path.as_path();
    while !existing.exists() {
        // `..` or `.` past what exists could lead anywhere once created.
        let Some(name) = existing.file_name() else {
            bail!("{} names a missing folder through . or ..", path.display());
        };
        missing.push(name);
        existing = existing.parent().unwrap_or(Path::new("/"));
    }
    let mut resolved = canonical(existing)?;
    resolved.extend(missing.iter().rev());
    Ok(resolved)
}

/// Creates `path` and its missing parents, syncing each parent so a new folder survives a crash.
fn make_dirs(path: &Path) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() || path.is_dir() {
        return Ok(());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    make_dirs(parent)?;
    match fs::create_dir(path) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            return Err(error).with_context(|| format!("cannot create {}", path.display()));
        }
        _ => {}
    }
    sync(parent)
}

fn remove_if_present(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("cannot remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

fn sync(path: &Path) -> anyhow::Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Renames a complete copy to its name, then syncs its folder so the rename survives a crash.
fn publish(partial: &Path, path: &Path) -> anyhow::Result<()> {
    fs::rename(partial, path).with_context(|| format!("cannot write {}", path.display()))?;
    if let Some(parent) = path.parent() {
        sync(parent)?;
    }
    Ok(())
}

/// Copies `from` to `to` through a partial file, so `to` is either absent or complete.
fn copy_one(from: &Path, to: &Path) -> anyhow::Result<()> {
    let partial = PathBuf::from(format!("{}{PARTIAL}", to.display()));
    fs::copy(from, &partial)
        .with_context(|| format!("cannot copy {} to {}", from.display(), partial.display()))?;
    sync(&partial)?;
    publish(&partial, to)
}

/// Copies every file under `from` that `to` lacks, and returns how many. An attachment never
/// changes once stored, so a file of the same size is the same file. Nothing in `to` is removed,
/// and a file deleted while the copy runs is skipped: the cleanup job deletes orphans at any time.
fn copy_files(from: &Path, to: &Path) -> anyhow::Result<u64> {
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", from.display())),
    };
    let mut copied = 0;
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read {}", from.display()))?;
        let name = entry.file_name();
        let (source, target) = (from.join(&name), to.join(&name));
        let kind = entry
            .file_type()
            .with_context(|| format!("cannot read {}", source.display()))?;
        if kind.is_dir() {
            copied += copy_files(&source, &target)?;
            continue;
        }
        if !kind.is_file() || name.to_string_lossy().ends_with(PARTIAL) {
            continue;
        }
        let size = match entry.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", source.display()));
            }
        };
        if fs::metadata(&target).is_ok_and(|existing| existing.len() == size) {
            continue;
        }
        make_dirs(to)?;
        match copy_one(&source, &target) {
            Ok(()) => copied += 1,
            Err(_) if !source.exists() => {
                remove_if_present(&PathBuf::from(format!("{}{PARTIAL}", target.display())))?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn only_missing_or_different_files_are_copied_and_nothing_is_removed() {
        let (from, to) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(&from.path().join("ab/89/one"), b"one");
        write(&from.path().join("cd/01/two"), b"two");
        assert_eq!(copy_files(from.path(), to.path()).unwrap(), 2);
        assert_eq!(fs::read(to.path().join("cd/01/two")).unwrap(), b"two");

        write(&to.path().join("ab/89/one"), b"cut");
        write(&to.path().join("ff/ff/gone"), b"deleted since");
        write(&to.path().join("cd/01/two"), b"t");
        assert_eq!(copy_files(from.path(), to.path()).unwrap(), 1);
        assert_eq!(fs::read(to.path().join("cd/01/two")).unwrap(), b"two");
        assert_eq!(fs::read(to.path().join("ab/89/one")).unwrap(), b"cut");
        assert!(to.path().join("ff/ff/gone").exists());
    }

    #[test]
    fn partial_copies_are_never_carried_over() {
        let (from, to) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(&from.path().join("ab/89/one.partial"), b"half");
        assert_eq!(copy_files(from.path(), to.path()).unwrap(), 0);
        assert!(!to.path().join("ab/89/one.partial").exists());
    }

    #[test]
    fn a_missing_files_folder_copies_nothing() {
        let to = tempfile::tempdir().unwrap();
        let missing = to.path().join("nothing here");
        assert_eq!(copy_files(&missing, to.path()).unwrap(), 0);
    }

    #[tokio::test]
    async fn without_local_files_only_the_database_moves() {
        let data = tempfile::tempdir().unwrap();
        SqliteStore::open(&data.path().join(DATABASE))
            .await
            .unwrap();
        write(&data.path().join("files/ab/89/one"), b"one");
        let backup_dir = tempfile::tempdir().unwrap();

        let copied = backup(data.path(), backup_dir.path(), false).await.unwrap();
        assert_eq!(copied, Copied { files: 0 });
        assert!(backup_dir.path().join(DATABASE).is_file());
        assert!(!backup_dir.path().join(FILES).exists());

        write(&backup_dir.path().join("files/ab/89/one"), b"one");
        let restored = tempfile::tempdir().unwrap();
        let copied = restore(backup_dir.path(), restored.path(), false)
            .await
            .unwrap();
        assert_eq!(copied, Copied { files: 0 });
        assert!(restored.path().join(DATABASE).is_file());
        assert!(!restored.path().join(FILES).exists());
        assert!(!restored.path().join("cadocs.db.partial").exists());
    }

    #[tokio::test]
    async fn a_backup_and_its_data_folder_never_nest_and_a_refusal_creates_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let data = parent.path().join("files");
        fs::create_dir(&data).unwrap();
        SqliteStore::open(&data.join(DATABASE)).await.unwrap();

        let inside = data.join("backups/today");
        backup(&data, &inside, true).await.unwrap_err();
        assert!(!data.join("backups").exists());
        let around = parent.path().join("nope/../files/bk");
        backup(&data, &around, true).await.unwrap_err();
        assert!(!data.join("bk").exists());
        assert!(!parent.path().join("nope").exists());
        let error = backup(&data, parent.path(), true).await.unwrap_err();
        assert!(error.to_string().contains("data folder"), "{error}");
        assert!(!parent.path().join(DATABASE).exists());
    }

    #[tokio::test]
    async fn a_refused_restore_leaves_no_database() {
        let backup_dir = tempfile::tempdir().unwrap();
        write(&backup_dir.path().join(DATABASE), b"not a database at all");
        let data = tempfile::tempdir().unwrap();
        restore(backup_dir.path(), data.path(), true)
            .await
            .unwrap_err();
        assert!(!data.path().join(DATABASE).exists());
        assert!(!data.path().join("cadocs.db.partial").exists());

        // The next run replaces what the failed one left.
        let source = tempfile::tempdir().unwrap();
        SqliteStore::open(&source.path().join(DATABASE))
            .await
            .unwrap();
        backup(source.path(), backup_dir.path(), true)
            .await
            .unwrap();
        restore(backup_dir.path(), data.path(), true).await.unwrap();
        assert!(data.path().join(DATABASE).is_file());
    }
}
