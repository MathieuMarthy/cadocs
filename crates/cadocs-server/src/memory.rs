//! How the server gives memory back to the system. glibc is the only C library cadocs targets
//! (`docs/OPERATIONS.md`), so these are its calls.

use std::ffi::c_int;

/// `M_MMAP_THRESHOLD` in glibc's `malloc.h`.
const M_MMAP_THRESHOLD: c_int = -3;
/// Above a room's usual allocations, below the 19 MB block of a password hash (ADR 0023).
const MMAP_THRESHOLD_BYTES: c_int = 4 * 1024 * 1024;

unsafe extern "C" {
    fn mallopt(param: c_int, value: c_int) -> c_int;
    fn malloc_trim(pad: usize) -> c_int;
}

/// Maps every block of 4 MiB or more on its own, so that freeing it unmaps it at once. glibc
/// otherwise raises its threshold after the first such block is freed, and serves the next ones
/// from the heap of the thread's arena, where `malloc_trim` misses them (ADR 0023). A threshold
/// set by hand stays fixed. Call it first, before any thread starts.
pub fn fix_mmap_threshold() {
    // SAFETY: takes no pointer, and only changes a setting of the allocator.
    let set = unsafe { mallopt(M_MMAP_THRESHOLD, MMAP_THRESHOLD_BYTES) };
    debug_assert_eq!(set, 1, "glibc refused the mmap threshold");
}

/// Gives back the free pages of every arena, which glibc keeps when live blocks sit above them
/// (ADR 0015). Blocking: it walks each heap with that arena locked.
pub fn trim() {
    // SAFETY: takes no pointer, and glibc locks each arena itself.
    unsafe { malloc_trim(0) };
}
