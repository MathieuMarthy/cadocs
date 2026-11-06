//! The command line. A hand-written match: two commands today, four at most (`docs/OPERATIONS.md`).

use std::ffi::OsString;

use anyhow::bail;

const USAGE: &str = "usage: cadocs [serve] | cadocs reset-password <email>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    ResetPassword { email: String },
}

/// Reads the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<Command> {
    let args: Vec<_> = args.into_iter().collect();
    let args: Vec<_> = args.iter().map(|arg| arg.to_str()).collect();
    match args[..] {
        [] | [Some("serve")] => Ok(Command::Serve),
        [Some("reset-password"), Some(email)] => Ok(Command::ResetPassword {
            email: email.to_owned(),
        }),
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
    fn anything_else_is_refused_with_the_usage() {
        for args in [
            &["nonsense"][..],
            &["serve", "extra"],
            &["--port"],
            &["reset-password"],
            &["reset-password", "a@b.c", "extra"],
        ] {
            let error = parsed(args).unwrap_err().to_string();
            assert!(error.contains("usage: cadocs [serve]"), "{error}");
        }
    }
}
