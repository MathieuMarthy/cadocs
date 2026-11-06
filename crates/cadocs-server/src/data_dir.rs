//! The data folder (`docs/OPERATIONS.md`).

use std::fs::{self, File};
use std::path::Path;

use anyhow::Context;

/// Creates the folder if needed and takes its exclusive lock, held until the file is dropped. Two
/// servers on one folder would hand out the same `seq` (ADR 0006). The OS drops the lock when the
/// process ends, however it ends.
pub fn lock(dir: &Path) -> anyhow::Result<File> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join("cadocs.lock");
    let file = File::create(&path).with_context(|| format!("cannot open {}", path.display()))?;
    file.try_lock()
        .with_context(|| format!("{} is in use by another server", dir.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_folder_and_its_lock_file() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("data");
        let _lock = lock(&dir).unwrap();
        assert!(dir.join("cadocs.lock").is_file());
    }

    #[test]
    fn a_held_folder_is_refused_until_it_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let held = lock(dir.path()).unwrap();

        let error = lock(dir.path()).unwrap_err().to_string();
        assert!(error.contains("in use by another server"), "{error}");

        drop(held);
        lock(dir.path()).expect("the lock is free again");
    }
}
