# ADR 0031: main runs the tests again only on a tree no pull request tested

Status: accepted.

## Context

`ci.yml` and `security.yml` ran on each pull request, then in full again on the push to `main` that merged it. The second run of a job can only catch what the first did not see: a `main` that moved after the pull request was tested, or a push to `main` that no pull request tested. Over the 28 pull requests merged from 2026-11-17 to 2026-11-27, it caught neither: main had moved since the last pull request run for 7 of them, but only 2 merges (#42 and #44, Dependabot updates merged within a minute) had a tree other than their pull request's head, and the failures it showed were flaky tests or an advisory the next pull request also hit (step 69, [CI notes](../notes/ci.md)).

## Decision

On a push to `main`, a `tested` job compares the merge's tree with its second parent, the pull request's head. When they match and that head has a successful `ci.yml` run as a pull request, `rust` and `web` are skipped; otherwise they run again, so a moved main, a squash or rebase merge and a direct push are tested. `e2e` and `image` run on `main` either way, since no pull request runs them.

`security.yml` on `main` always runs `secrets`, the only scan of a direct push before Monday, and `codeql`, whose analysis of `main` is the alert list and the base every pull request is compared with. Its own `tested` job, the same check against `security.yml`'s runs, gates `cargo-deny` and `npm-audit`: a merge its pull request checked skips them, since its lockfiles are the ones checked and an advisory published since waits for the next pull request or Monday. A direct push, which may bring a crate under a denied license, a git source or a vulnerable npm package, gets them.

## Consequences

- Per merged pull request, 38.7 job minutes before and 28.6 after (51.1 and 39.6 rounded up per job), measured over those 28 merges.
- No repository setting is needed. A pull request merged red, or before its run finished, has no successful run on its head, so main tests it.
- A green run proves the head merged with the base of its time, not the head's own tree. A change merged and then reverted on `main` while a pull request was open, or a pull request retargeted to `main` after its run (`edited` is not a default `pull_request` type), merges a tree no run tested. Accepted as rare; required up-to-date branches would close it.
- `rust` and `web` show as skipped on most `main` commits; the run and the README badge stay green.

## Rejected alternatives

- **A merge queue.** GitHub offers it only to repositories owned by an organization; this one belongs to a user.
- **Branches required to be up to date.** GitHub counts commits behind `main`, not trees: 9 of the 28 pull requests were behind (stacked branches lack their base's merge commit) and would have merged `main` in and run everything again, against 2 test runs on `main` now. It also blocks direct pushes to `main`, or lets an admin's through untested.
- **Drop the tests on `main`.** A moved main and direct pushes would go untested.
