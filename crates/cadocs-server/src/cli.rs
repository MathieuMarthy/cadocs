//! The command line. A hand-written match over the four commands of `docs/OPERATIONS.md`.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::bail;

const USAGE: &str = "usage: cadocs [serve] | cadocs reset-password <email> \
                     | cadocs backup <dir> | cadocs restore <dir>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    ResetPassword { email: String },
    Backup { to: PathBuf },
    Restore { from: PathBuf },
}

/// Reads the arguments after the program name. A folder may be any path the system allows.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<Command> {
    let args: Vec<_> = args.into_iter().collect();
    let names: Vec<_> = args.iter().map(|arg| arg.to_str()).collect();
    let folder = || PathBuf::from(&args[1]);
    match names[..] {
        [] | [Some("serve")] => Ok(Command::Serve),
        [Some("reset-password"), Some(email)] => Ok(Command::ResetPassword {
            email: email.to_owned(),
        }),
        [Some("backup"), _] if !args[1].is_empty() => Ok(Command::Backup { to: folder() }),
        [Some("restore"), _] if !args[1].is_empty() => Ok(Command::Restore { from: folder() }),
        _ => bail!(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> anyhow::Result<Command> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn serving_is_the_default() {
        assert_eq!(parsed(&[]).unwrap(), Command::Serve);
        assert_eq!(parsed(&["serve"]).unwrap(), Command::Serve);
    }

    #[test]
    fn reset_password_takes_an_email() {
        assert_eq!(
            parsed(&["reset-password", "ada@example.org"]).unwrap(),
            Command::ResetPassword {
                email: "ada@example.org".to_owned()
            }
        );
    }

    #[test]
    fn backup_and_restore_take_a_folder() {
        assert_eq!(
            parsed(&["backup", "/srv/backups/today"]).unwrap(),
            Command::Backup {
                to: PathBuf::from("/srv/backups/today")
            }
        );
        assert_eq!(
            parsed(&["restore", "today"]).unwrap(),
            Command::Restore {
                from: PathBuf::from("today")
            }
        );
    }

    #[test]
    fn anything_else_is_refused_with_the_usage() {
        for args in [
            &["nonsense"][..],
            &["serve", "extra"],
            &["--port"],
            &["reset-password"],
            &["reset-password", "a@b.c", "extra"],
            &["backup"],
            &["backup", ""],
            &["backup", "a", "b"],
            &["restore"],
            &["restore", ""],
        ] {
            let error = parsed(args).unwrap_err().to_string();
            assert!(error.contains("usage: cadocs [serve]"), "{error}");
        }
    }
}
