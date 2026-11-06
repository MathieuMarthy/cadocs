# Benchmark

What a change to `cadocs-bench` or a RAM measure must know.

- **Benchmark:** the scenario and go or no-go rule in `docs/ARCHITECTURE.md` are fixed. Do not change them to make a number pass. Stage 0 ended on a go, after ADR 0015 added `malloc_trim` on unload. A loaded document costs about 530 KiB, 10 to 15 times its encoded size, so 256 rooms would pass 100 MB: the MVP must keep that in mind when it sets `CADOCS_MAX_ROOMS`. `cadocs-bench seed` also makes the account behind `SESSION_TOKEN`, and needs an empty database. `cadocs-bench` is outside `default-members`: `--workspace` covers it, and building it with the server changes the server's features. glibc's in-use bytes need a preload probe, because `ptrace_scope` blocks `gdb -p`.
