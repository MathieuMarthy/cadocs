# ADR 0010: AGPL-3.0 licence

Status: accepted

## Context

cadocs is a network service that people host for others. Features Docmost sells in paid editions, such as page restrictions, templates and MFA, are planned here as ordinary features. The main risk to the project is a closed hosted fork that benefits from the work without giving changes back.

## Decision

The project is licensed under **AGPL-3.0-only**. The full text is in `LICENSE` at the root of the repository.

## Consequences

- Anyone who runs a modified cadocs for users over a network must offer those users the source.
- The interface links to the source of the running version, which satisfies section 13 for unmodified builds.
- Dependencies must be compatible. MIT, Apache-2.0, BSD and OFL (IBM Plex) are. The OFL and MIT notices of embedded assets ship with the binary.
- Some companies refuse AGPL software, so adoption may be lower than under MIT.
- External contributions are under the same licence. Relicensing later would need every contributor's agreement.

## Rejected alternatives

- **MIT, as uptime-monitoring.** Maximum adoption, but it allows a closed hosted fork.
- **GPL-3.0.** Its copyleft does not apply to software that is only offered over a network.
- **Source-available licences (BSL, Elastic).** Not open source, which goes against a self-hosted community project.
