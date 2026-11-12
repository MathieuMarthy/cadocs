# ADR 0021: First-run setup needs a code from the server's log

Status: accepted

## Context

Until the first account exists, `POST /auth/setup` made whoever called it the instance admin. The security review of step 24 found that anyone who reaches a new instance before its operator can therefore claim it. The window is short but real: a reverse proxy that gets a certificate publishes the host name in Certificate Transparency logs, and scanners visit such names within minutes.

## Decision

Setup needs a code that only the server's log shows.

- Each start generates a new code, 32 random bytes in hexadecimal, kept in memory only. Nothing is stored, so a restart makes a new one and an old link stops working.
- While no account exists, `cadocs serve` logs `first-run setup is pending: open <public url>/setup/<code> to create the first account`, at `warn`, so a log filter of `warn` still shows it.
- `POST /auth/setup` takes the code in `code`. A missing one is `400` with `code: required`, a wrong one `400` with `code: wrong`, compared in constant time. The route keeps its limit on password guesses, which bounds attempts.
- The front end reads the code from `/setup/<code>`. Opened as `/setup`, the page asks for it, and it also asks once the server refuses the code of the link.

## Consequences

- An operator reads the log once, at first run: `docker logs`, `journalctl -u cadocs`, or the terminal.
- Scripts that set up an instance read the code from the log, as `scripts/check-image.sh` and the end-to-end tests do.
- With `CADOCS_LOG=error` the link does not show, and the operator must lower the filter for the first run.

## Rejected alternatives

- **Documenting only** that setup must be done before the instance is exposed. The Compose example publishes the instance at once, so the advice would be missed exactly when it matters.
- **A code chosen by the operator in an environment variable.** It would be optional, so instances would be open by default, and it stays in the environment after setup.
- **A time window after startup**, as some products do. It narrows the race without closing it, and a restart reopens it.
- **Setup from the command line only.** It needs a shell in the container, which the distroless image does not have.
