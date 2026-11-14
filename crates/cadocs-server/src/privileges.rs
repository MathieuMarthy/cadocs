//! Giving up root at start (ADR 0025). The Docker image starts as root, so that it can take a
//! data folder it does not own, and nothing after `drop_root` runs as root.

use std::ffi::c_int;
use std::os::unix::fs::{MetadataExt, lchown};
use std::path::Path;
use std::{fs, io, ptr};

use anyhow::{Context, ensure};

/// A user and its group, written `uid:gid` in `CADOCS_RUN_AS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAs {
    pub uid: u32,
    pub gid: u32,
}

impl RunAs {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let (uid, gid) = value.split_once(':').context("expected uid:gid")?;
        let uid = uid.parse().context("the uid is not a number")?;
        let gid = gid.parse().context("the gid is not a number")?;
        ensure!(uid != 0 && gid != 0, "root is not a user to fall back to");
        // -1, which `chown` reads as "leave it as it is".
        ensure!(uid != u32::MAX && gid != u32::MAX, "not a user");
        Ok(Self { uid, gid })
    }
}

unsafe extern "C" {
    fn geteuid() -> u32;
    fn setgroups(size: usize, list: *const u32) -> c_int;
    fn setgid(gid: u32) -> c_int;
    fn setuid(uid: u32) -> c_int;
}

/// When root started the process and `CADOCS_RUN_AS` is set, becomes the owner of the data
/// folder, or `run_as` once the folder is given to it if root owns it. Does nothing otherwise.
/// Call it before any thread starts and before anything opens a file.
pub fn drop_root(data_dir: &Path, run_as: Option<RunAs>) -> anyhow::Result<()> {
    // SAFETY: takes no argument and cannot fail.
    let root = unsafe { geteuid() } == 0;
    let Some(run_as) = run_as.filter(|_| root) else {
        return Ok(());
    };
    let shown = data_dir.display();
    fs::create_dir_all(data_dir).with_context(|| format!("cannot create {shown}"))?;
    // The folder itself, in case the setting names a link to it.
    let dir = fs::canonicalize(data_dir).with_context(|| format!("cannot read {shown}"))?;
    let meta = fs::metadata(&dir).with_context(|| format!("cannot read {shown}"))?;
    let user = user_for(meta.uid(), meta.gid(), run_as);
    if meta.uid() == 0 {
        give(&dir, user).with_context(|| {
            format!(
                "cannot give {shown} to user {}: is CAP_CHOWN dropped?",
                user.uid
            )
        })?;
    }
    become_user(user)
}

/// Who serves a folder: its owner, so a folder the operator made stays theirs, or `fallback`
/// when root owns it. Never root's group either.
fn user_for(owner: u32, group: u32, fallback: RunAs) -> RunAs {
    if owner == 0 {
        return fallback;
    }
    let gid = if group == 0 { fallback.gid } else { group };
    RunAs { uid: owner, gid }
}

/// Gives `path` and all it holds to `user`. A link is given, never followed, so nothing outside
/// the folder changes hands.
fn give(path: &Path, user: RunAs) -> io::Result<()> {
    walk(path, &mut |entry| {
        lchown(entry, Some(user.uid), Some(user.gid))
    })
}

/// Calls `visit` on all `path` holds, then on `path`: the folder's owner tells `drop_root` that
/// the walk is done, so it changes last.
fn walk(path: &Path, visit: &mut dyn FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        for entry in fs::read_dir(path)? {
            walk(&entry?.path(), visit)?;
        }
    }
    visit(path)
}

fn become_user(user: RunAs) -> anyhow::Result<()> {
    // SAFETY: the list is empty, so its pointer is never read, and the other calls take numbers.
    // The groups go first, then the group: neither can change once root is gone.
    let dropped =
        unsafe { setgroups(0, ptr::null()) == 0 && setgid(user.gid) == 0 && setuid(user.uid) == 0 };
    if !dropped {
        let error = io::Error::last_os_error();
        return Err(error).with_context(|| {
            format!(
                "cannot give up root for user {}: are CAP_SETUID or CAP_SETGID dropped?",
                user.uid
            )
        });
    }
    // SAFETY: takes a number. It succeeds only if root was not given up for good.
    ensure!(unsafe { setuid(0) } != 0, "root can still be taken back");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    const FALLBACK: RunAs = RunAs {
        uid: 65532,
        gid: 65532,
    };

    #[test]
    fn a_user_is_written_as_uid_and_gid() {
        let user = RunAs::parse("1000:100").unwrap();
        assert_eq!(
            user,
            RunAs {
                uid: 1000,
                gid: 100
            }
        );
        for wrong in [
            "",
            "4294967295:5",
            "5:4294967295",
            "1000",
            "ada:users",
            "1000:",
            "0:0",
            "0:100",
            "1000:0",
            "-1:5",
        ] {
            assert!(RunAs::parse(wrong).is_err(), "{wrong:?} was accepted");
        }
    }

    #[test]
    fn the_owner_of_the_folder_serves_it_unless_it_is_root() {
        assert_eq!(user_for(0, 0, FALLBACK), FALLBACK);
        assert_eq!(user_for(0, 100, FALLBACK), FALLBACK);
        let owner = RunAs {
            uid: 1000,
            gid: 100,
        };
        assert_eq!(user_for(1000, 100, FALLBACK), owner);
    }

    #[test]
    fn the_group_of_root_is_never_kept() {
        let user = user_for(1000, 0, FALLBACK);
        assert_eq!((user.uid, user.gid), (1000, 65532));
    }

    #[test]
    fn nothing_happens_without_root_or_without_the_setting() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("data");
        drop_root(&dir, None).unwrap();
        // SAFETY: takes no argument and cannot fail.
        if unsafe { geteuid() } != 0 {
            drop_root(&dir, Some(FALLBACK)).unwrap();
        }
        assert!(!dir.exists());
    }

    /// As the user who owns the files, giving them to that user changes nothing, but a followed
    /// link would reach a file of root and be refused.
    #[test]
    fn giving_a_folder_walks_it_and_follows_no_link() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("files/ab");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("one"), "1").unwrap();
        symlink("/etc/passwd", nested.join("file-link")).unwrap();
        symlink("/etc", dir.path().join("folder-link")).unwrap();
        let meta = fs::metadata(dir.path()).unwrap();
        let me = RunAs {
            uid: meta.uid(),
            gid: meta.gid(),
        };

        give(dir.path(), me).unwrap();

        // A missing entry stops the walk, so the walk does reach what the folder holds.
        let gone = dir.path().join("gone");
        symlink(&gone, nested.join("dangling")).unwrap();
        give(dir.path(), me).expect("a dangling link is given as a link");
        assert!(give(&gone, me).is_err());
    }

    /// The owner of the folder is what says the walk is still to do, so a walk that stops
    /// halfway must leave it to root, and the next start does it again.
    #[test]
    fn the_folder_itself_changes_hands_last() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("files");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("one"), "1").unwrap();

        let mut seen = Vec::new();
        walk(dir.path(), &mut |entry| {
            seen.push(entry.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [nested.join("one"), nested, dir.path().to_owned()]);

        let mut reached = Vec::new();
        let stopped = walk(dir.path(), &mut |entry| {
            reached.push(entry.to_owned());
            Err(io::Error::other("read-only"))
        });
        assert!(stopped.is_err());
        assert!(!reached.contains(&dir.path().to_owned()));
    }
}
