use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use cadocs_files::{ByteStream, FileError, FileStore, LocalFileStore};
use futures_util::StreamExt;
use futures_util::stream;

const ID: &str = "0190a1b2-c3d4-7e5f-8a9b-0123456789ab";

fn body(chunks: &[&'static [u8]]) -> ByteStream<'static> {
    let chunks: Vec<io::Result<Bytes>> = chunks.iter().map(|c| Ok(Bytes::from(*c))).collect();
    Box::pin(stream::iter(chunks))
}

async fn read_all(store: &LocalFileStore, id: &str) -> Vec<u8> {
    let mut file = store.get(id).await.unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = file.bytes.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(out.len() as u64, file.size);
    out
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else {
                found.push(entry.file_name().into_string().unwrap());
            }
        }
    }
    found
}

#[tokio::test]
async fn a_file_is_stored_hashed_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();

    let stored = store
        .put(ID, body(&[b"hello ", b"world"]), 100)
        .await
        .unwrap();
    assert_eq!(stored.size, 11);
    // sha256("hello world")
    assert_eq!(
        stored.sha256,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
    assert_eq!(read_all(&store, ID).await, b"hello world");
    // Two levels of fan-out, from the random end of the id.
    assert!(dir.path().join("files/ab/89").join(ID).is_file());
    assert!(files_under(&dir.path().join("tmp")).is_empty());
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_and_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();

    let refused = store.put(ID, body(&[b"12345", b"6"]), 5).await;
    assert!(
        matches!(refused, Err(FileError::TooLarge(5))),
        "{refused:?}"
    );
    assert!(files_under(dir.path()).is_empty());
    assert!(matches!(store.get(ID).await, Err(FileError::NotFound)));

    store
        .put(ID, body(&[b"12345"]), 5)
        .await
        .expect("exactly the limit");
}

#[tokio::test]
async fn an_upload_cut_short_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();

    let chunks: Vec<io::Result<Bytes>> = vec![
        Ok(Bytes::from_static(b"start")),
        Err(io::Error::new(io::ErrorKind::ConnectionReset, "gone")),
    ];
    let refused = store.put(ID, Box::pin(stream::iter(chunks)), 100).await;
    assert!(matches!(refused, Err(FileError::Body(_))), "{refused:?}");
    assert!(files_under(dir.path()).is_empty());
}

#[tokio::test]
async fn a_full_disk_is_a_storage_error() {
    let full = tokio::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .await
        .unwrap();
    let refused = cadocs_files::receive(full, body(&[&[7; 4096]]), 1 << 20).await;
    match refused {
        Err(FileError::Io(error)) => assert_eq!(error.kind(), io::ErrorKind::StorageFull),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn ids_that_could_name_a_path_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();
    for id in [
        "",
        "../../etc/passwd",
        "abc",
        "0190a1b2/../x1234",
        "ABCD1234",
    ] {
        let refused = store.put(id, body(&[b"x"]), 10).await;
        assert!(
            matches!(refused, Err(FileError::InvalidId)),
            "{id}: {refused:?}"
        );
        assert!(
            matches!(store.get(id).await, Err(FileError::InvalidId)),
            "{id}"
        );
    }
}

#[tokio::test]
async fn deleting_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();
    store.put(ID, body(&[b"x"]), 10).await.unwrap();

    store.delete(ID).await.unwrap();
    assert!(matches!(store.get(ID).await, Err(FileError::NotFound)));
    store
        .delete(ID)
        .await
        .expect("a missing file is already deleted");
}

#[tokio::test]
async fn opening_empties_the_temporary_folder() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    std::fs::write(dir.path().join("tmp/left-by-a-crash.part"), b"x").unwrap();

    LocalFileStore::open(dir.path()).unwrap();
    assert!(files_under(&dir.path().join("tmp")).is_empty());
}

#[tokio::test]
async fn listing_finds_every_file_with_its_date() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();
    let ids: Vec<String> = (0..250)
        .map(|n| format!("0190a1b2-c3d4-7e5f-8a9b-{n:012x}"))
        .collect();
    for id in &ids {
        store.put(id, body(&[b"x"]), 10).await.unwrap();
    }

    let before = SystemTime::now() - Duration::from_secs(60);
    let mut listed = Vec::new();
    let mut batches = store.list();
    while let Some(batch) = batches.next().await {
        let batch = batch.unwrap();
        assert!(batch.len() <= cadocs_files::LIST_BATCH);
        for file in batch {
            assert!(file.modified > before);
            listed.push(file.id);
        }
    }
    listed.sort();
    assert_eq!(listed, ids);
}

#[tokio::test]
async fn an_upload_dropped_halfway_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFileStore::open(dir.path()).unwrap();

    // A client that sends a chunk, then nothing: the server drops the request.
    let first = stream::iter([Ok(Bytes::from_static(b"start"))]);
    let body: ByteStream<'static> = Box::pin(first.chain(stream::pending()));
    let upload = store.put(ID, body, 100);
    let cut = tokio::time::timeout(Duration::from_millis(100), upload).await;
    assert!(cut.is_err(), "the body never ends");
    assert!(files_under(dir.path()).is_empty());
}
