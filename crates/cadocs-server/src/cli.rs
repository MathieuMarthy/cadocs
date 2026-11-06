//! The command line. A hand-written match: one command today, four at most (`docs/OPERATIONS.md`).

use std::ffi::OsString;

use anyhow::ensure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Serve,
}

/// Reads the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<Command> {
    let mut args = args.into_iter();
    let serve = match args.next() {
        None => true,
        Some(command) => command == "serve" && args.next().is_none(),
    };
    ensure!(serve, "usage: cadocs [serve]");
    Ok(Command::Serve)
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
    fn anything_else_is_refused_with_the_usage() {
        for args in [&["nonsense"][..], &["serve", "extra"], &["--port"]] {
            let error = parsed(args).unwrap_err().to_string();
            assert!(error.contains("usage: cadocs [serve]"), "{error}");
        }
    }
}
