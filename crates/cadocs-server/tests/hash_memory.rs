//! Alone in its file, so that its process runs nothing else while it reads its own memory.

use std::fs;

use cadocs_server::password::Passwords;

/// One hash maps about 19 MB. Several staying would pass this by far.
const MAX_GROWTH_KIB: u64 = 5 * 1024;

fn anonymous_kib() -> u64 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let line = status
        .lines()
        .find(|line| line.starts_with("RssAnon:"))
        .unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// Without a trim: what a hash maps must go back as soon as it ends (ADR 0023).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_memory_of_a_hash_goes_back_without_a_trim() {
    cadocs_server::memory::fix_mmap_threshold();
    let passwords = Passwords::default();
    // The first hash raised glibc's threshold past its block, so the next ones stayed.
    let hash = passwords.hash("first".to_owned()).await.unwrap();
    let before = anonymous_kib();
    for _ in 0..5 {
        let matches = passwords.verify("first".to_owned(), Some(hash.clone()));
        assert!(matches.await.unwrap());
    }
    let after = anonymous_kib();
    assert!(
        after <= before + MAX_GROWTH_KIB,
        "{before} KiB before the hashes, {after} KiB after"
    );
}
