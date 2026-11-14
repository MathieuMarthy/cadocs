# ADR 0025: The image starts as root and the binary gives it up

Status: accepted

## Context

The image ran as `nonroot` (uid 65532) from its first instruction. A named volume takes its owner from the image's `/data`, so it worked. A bind mount does not: Docker creates a missing host folder as root, and a folder the operator made belongs to them. Either way the first start failed with `cannot open /data/cadocs.lock: Permission denied`, and the fix was a `chown` by hand to a uid the operator had to look up.

Images that fix this start as root and run an entrypoint script that changes the owner, then `gosu` or `su-exec` to drop root. The cadocs image has no shell and no other binary.

## Decision

The image starts as root (`USER 0:0`) and sets `CADOCS_RUN_AS=65532:65532`. The binary does the entrypoint's work itself, in `cadocs_server::privileges`, as the first thing after reading its settings, before any thread starts or any file is opened, and for every command:

1. It only acts when its effective user is root and `CADOCS_RUN_AS` is set. A binary run by hand, as any user, behaves as before.
2. It creates the data folder if needed and reads its owner.
3. If root owns the folder, it gives the folder and all it holds to the `CADOCS_RUN_AS` user, without following links, and that is the user it becomes. Otherwise it becomes the folder's owner and changes no owner. Root's group is never kept: the `CADOCS_RUN_AS` group takes its place.
4. It clears the supplementary groups, sets the group, sets the user, then checks that root cannot be taken back, and refuses to start if it can.

Leaving uid 0 for good makes the kernel clear every capability of the process, so the server runs as it did under `USER nonroot`.

`backup`, `restore` and `reset-password` drop root the same way: `docker exec` now enters as root, and a backup run as root would leave root's files in the data folder of a server that is not root.

## Consequences

- `./data:/data` works with no step by hand, and a folder the operator made keeps its owner, so they read their own backups and files.
- The walk that changes owners runs once, on a folder root owns. The folder itself changes last, so a walk that fails or is killed halfway starts again at the next start. After it the folder has another owner, so later starts skip it.
- The walk gives everything under the folder, a volume mounted inside it included. It checks each entry, then reads it, so someone who can already write inside a folder root owns could swap a folder for a link between the two and have the files of another folder given away. Nobody but cadocs and its operator writes to a data folder, so this is accepted.
- For a moment the process is root in the container: while it parses its arguments and settings, creates the folder and changes owners. No socket is open and no request is read by then.
- Giving up root needs the `CHOWN`, `SETUID` and `SETGID` capabilities, which Docker grants by default. With `cap_drop: ALL`, or where root is refused (`runAsNonRoot`), set `user: "65532:65532"`: the binary then does nothing, as before this ADR, and the folder must be writable by that user.
- A data folder owned by root on purpose is taken from root. An empty `CADOCS_RUN_AS` turns all of this off and the server stays the user it was started as.
- With rootless Docker or Podman, root in the container is the operator on the host, so a folder they made looks owned by root and is given to uid 65532 of the container, a sub-uid on the host. `user:` or an empty `CADOCS_RUN_AS` avoids it.
- The three calls are declared by hand, as `memory.rs` does for glibc's, so no dependency is added.

## Rejected alternatives

- **Document the `chown`.** It was documented, and the first person to try a bind mount still hit the error.
- **An entrypoint script with `su-exec`.** It needs a shell and a second binary in an image that has neither.
- **Run as root.** A flaw in the server would then own the volume and whatever else the container reaches.
- **Always become `CADOCS_RUN_AS`,** changing owners at each start. It takes a folder away from the operator who made it, and walks the files at every start.
- **Without `CADOCS_RUN_AS`: give up root whenever started as root.** A binary run with `sudo` outside Docker would change the owner of a folder to a uid that means nothing on that host.
