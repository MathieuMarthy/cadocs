//! The S3 store (ADR 0020), against an in-memory S3 that ignores signatures, and against a real
//! server when `CADOCS_TEST_S3_ENDPOINT` names one (see `real_s3` below).
#![cfg(feature = "s3")]

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use cadocs_files::{
    ByteStream, Download, Downloads, FileError, FileStore, LIST_BATCH, S3Config, S3FileStore,
};
use futures_util::{StreamExt, stream};
use tempfile::TempDir;
use tokio::net::TcpListener;

const ID: &str = "0190a1b2-c3d4-7e5f-8a9b-0123456789ab";
const BUCKET: &str = "wiki";

/// Objects by key, with their date.
type Objects = Arc<Mutex<BTreeMap<String, (Bytes, SystemTime)>>>;

struct FakeS3 {
    endpoint: String,
    objects: Objects,
}

/// Path-style only, as cadocs sends. A request with no signature is refused, so a store that
/// forgot to sign fails here too.
async fn fake_s3() -> FakeS3 {
    let objects = Objects::default();
    let app = Router::new()
        // `rusty-s3` addresses the bucket with a final slash.
        .route("/{bucket}/", get(list).head(head_bucket))
        .route(
            "/{bucket}/{*key}",
            get(get_object).put(put_object).delete(delete_object),
        )
        .with_state(objects.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    FakeS3 { endpoint, objects }
}

fn signed(query: &BTreeMap<String, String>) -> bool {
    query.contains_key("X-Amz-Signature")
}

async fn head_bucket(
    UrlPath(bucket): UrlPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> StatusCode {
    match (signed(&query), bucket == BUCKET) {
        (false, _) => StatusCode::FORBIDDEN,
        (true, false) => StatusCode::NOT_FOUND,
        (true, true) => StatusCode::OK,
    }
}

async fn put_object(
    State(objects): State<Objects>,
    UrlPath((_, key)): UrlPath<(String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if !signed(&query) {
        return StatusCode::FORBIDDEN;
    }
    let length = headers.get("content-length").and_then(|v| v.to_str().ok());
    if length != Some(&body.len().to_string()) {
        return StatusCode::LENGTH_REQUIRED;
    }
    let mut objects = objects.lock().unwrap();
    objects.insert(key, (body, SystemTime::now()));
    StatusCode::OK
}

async fn get_object(
    State(objects): State<Objects>,
    UrlPath((_, key)): UrlPath<(String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    if !signed(&query) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let found = objects.lock().unwrap().get(&key).map(|(data, _)| data.clone());
    match found {
        Some(data) => data.into_response(),
        None => (StatusCode::NOT_FOUND, "<Error><Code>NoSuchKey</Code></Error>").into_response(),
    }
}

async fn delete_object(
    State(objects): State<Objects>,
    UrlPath((_, key)): UrlPath<(String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
) -> StatusCode {
    if !signed(&query) {
        return StatusCode::FORBIDDEN;
    }
    objects.lock().unwrap().remove(&key);
    StatusCode::NO_CONTENT
}

/// `ListObjectsV2`, with `prefix`, `max-keys` and a continuation token that is the last key sent.
async fn list(
    State(objects): State<Objects>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    if !signed(&query) || query.get("list-type").map(String::as_str) != Some("2") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let prefix = query.get("prefix").cloned().unwrap_or_default();
    let max: usize = query.get("max-keys").map_or(1000, |v| v.parse().unwrap());
    let after = query.get("continuation-token").cloned();
    let objects = objects.lock().unwrap();
    let keys: Vec<(&String, &SystemTime)> = objects
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .filter(|(key, _)| after.as_ref().is_none_or(|after| *key > after))
        .map(|(key, (_, date))| (key, date))
        .take(max + 1)
        .collect();
    let truncated = keys.len() > max;
    let page = &keys[..keys.len().min(max)];
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
    );
    for (key, date) in page {
        let date = jiff::Timestamp::try_from(**date).unwrap();
        xml.push_str(&format!(
            "<Contents><Key>{key}</Key><LastModified>{date}</LastModified>\
             <ETag>\"0\"</ETag><Size>1</Size><StorageClass>STANDARD</StorageClass></Contents>"
        ));
    }
    if truncated {
        let last = page.last().unwrap().0;
        xml.push_str(&format!("<NextContinuationToken>{last}</NextContinuationToken>"));
    }
    xml.push_str("</ListBucketResult>");
    xml.into_response()
}

fn config(endpoint: &str, prefix: &str, downloads: Downloads) -> S3Config {
    S3Config {
        endpoint: endpoint.to_owned(),
        bucket: BUCKET.to_owned(),
        region: "us-east-1".to_owned(),
        access_key_id: "cadocs".to_owned(),
        secret_access_key: "secret".to_owned(),
        prefix: prefix.to_owned(),
        downloads,
        link_lifetime: Duration::from_secs(3600),
        ca_file: None,
    }
}

async fn open(fake: &FakeS3, prefix: &str) -> (S3FileStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = S3FileStore::open(dir.path(), config(&fake.endpoint, prefix, Downloads::Proxy))
        .await
        .unwrap();
    (store, dir)
}

fn body(chunks: &[&'static [u8]]) -> ByteStream<'static> {
    let chunks: Vec<io::Result<Bytes>> = chunks.iter().map(|c| Ok(Bytes::from(*c))).collect();
    Box::pin(stream::iter(chunks))
}

async fn read_all(store: &dyn FileStore, id: &str) -> Vec<u8> {
    let mut file = store.get(id).await.unwrap();
    let mut out = Vec::new();
    while let Some(chunk) = file.bytes.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(out.len() as u64, file.size);
    out
}

async fn listed(store: &dyn FileStore) -> Vec<String> {
    let mut ids = Vec::new();
    let mut listing = store.list();
    while let Some(batch) = listing.next().await {
        let batch = batch.unwrap();
        assert!(!batch.is_empty() && batch.len() <= LIST_BATCH);
        ids.extend(batch.into_iter().map(|file| file.id));
    }
    ids.sort();
    ids
}

/// The behaviour every store shares, so a real server can run it too.
async fn contract(store: &dyn FileStore, tmp: &std::path::Path) {
    let stored = store
        .put(ID, body(&[b"hello ", b"world"]), 100)
        .await
        .unwrap();
    assert_eq!(stored.size, 11);
    assert_eq!(
        stored.sha256,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
    assert_eq!(read_all(store, ID).await, b"hello world");
    assert_eq!(listed(store).await, [ID]);

    let other = "0190a1b2-c3d4-7e5f-8a9b-000000000001";
    let refused = store.put(other, body(&[b"0123456789", b"x"]), 10).await;
    assert!(matches!(refused, Err(FileError::TooLarge(10))), "{refused:?}");
    assert!(matches!(store.get(other).await, Err(FileError::NotFound)));

    assert!(matches!(
        store.put("../etc/passwd", body(&[b"x"]), 10).await,
        Err(FileError::InvalidId)
    ));
    assert!(matches!(store.get("ABCD").await, Err(FileError::InvalidId)));

    store.delete(ID).await.unwrap();
    store.delete(ID).await.unwrap();
    assert!(matches!(store.get(ID).await, Err(FileError::NotFound)));
    assert!(listed(store).await.is_empty());
    // Nothing stays behind in the temporary folder, kept or refused.
    assert_eq!(std::fs::read_dir(tmp).unwrap().count(), 0);
}

#[tokio::test]
async fn the_s3_store_keeps_the_contract() {
    let fake = fake_s3().await;
    let (store, dir) = open(&fake, "").await;
    contract(&store, &dir.path().join("tmp")).await;
}

#[tokio::test]
async fn keys_follow_the_disk_layout_under_the_prefix() {
    let fake = fake_s3().await;
    for prefix in ["cadocs", "cadocs/"] {
        let (store, _dir) = open(&fake, prefix).await;
        store.put(ID, body(&[b"x"]), 10).await.unwrap();
        let keys: Vec<String> = fake.objects.lock().unwrap().keys().cloned().collect();
        assert_eq!(keys, [format!("cadocs/ab/89/{ID}")]);
        store.delete(ID).await.unwrap();
    }
}

#[tokio::test]
async fn listing_pages_through_the_bucket_and_skips_foreign_keys() {
    let fake = fake_s3().await;
    let (store, _dir) = open(&fake, "cadocs").await;
    let mut ids: Vec<String> = (0..LIST_BATCH * 2 + 5)
        .map(|n| format!("0190a1b2-c3d4-7e5f-8a9b-{n:012x}"))
        .collect();
    for id in &ids {
        store.put(id, body(&[b"x"]), 10).await.unwrap();
    }
    {
        let mut objects = fake.objects.lock().unwrap();
        let now = SystemTime::now();
        // Another application's object, a key in the wrong folder, and one beside the prefix.
        for key in [
            "cadocs/notes.txt",
            &format!("cadocs/00/00/{ID}"),
            &format!("cadocs-old/ab/89/{ID}"),
        ] {
            objects.insert(key.to_owned(), (Bytes::from_static(b"x"), now));
        }
    }
    ids.sort();
    assert_eq!(listed(&store).await, ids);

    let mut listing = store.list();
    let first = listing.next().await.unwrap().unwrap();
    let age = SystemTime::now().duration_since(first[0].modified).unwrap();
    assert!(age < Duration::from_secs(60), "{age:?}");
}

#[tokio::test]
async fn opening_checks_the_bucket_and_empties_the_temporary_folder() {
    let fake = fake_s3().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tmp")).unwrap();
    std::fs::write(dir.path().join("tmp/left.part"), b"x").unwrap();

    let mut wrong = config(&fake.endpoint, "", Downloads::Proxy);
    wrong.bucket = "missing".to_owned();
    assert!(S3FileStore::open(dir.path(), wrong).await.is_err());

    S3FileStore::open(dir.path(), config(&fake.endpoint, "", Downloads::Proxy))
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
}

const IMAGE: Download<'static> = Download {
    content_type: "image/png",
    disposition: "inline; filename=\"a.png\"",
};

#[tokio::test]
async fn proxy_mode_gives_no_link_and_redirect_mode_a_presigned_one() {
    let fake = fake_s3().await;
    let (proxy, _dir) = open(&fake, "").await;
    assert_eq!(proxy.download_url(ID, &IMAGE), None);
    assert_eq!(proxy.download_origin(), None);

    let dir = tempfile::tempdir().unwrap();
    let redirect = S3FileStore::new(
        dir.path(),
        config(&fake.endpoint, "cadocs", Downloads::Redirect),
    )
    .unwrap();
    assert_eq!(redirect.download_origin(), Some(fake.endpoint.clone()));
    let url = redirect.download_url(ID, &IMAGE).unwrap();
    assert!(
        url.starts_with(&format!("{}/{BUCKET}/cadocs/ab/89/{ID}?", fake.endpoint)),
        "{url}"
    );
    let query: BTreeMap<String, String> = url::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    for (name, value) in [
        ("X-Amz-Expires", "3600"),
        ("response-content-type", "image/png"),
        ("response-content-disposition", "inline; filename=\"a.png\""),
        ("response-cache-control", "private, max-age=1800"),
    ] {
        assert_eq!(query.get(name).map(String::as_str), Some(value), "{url}");
    }
    assert!(query.contains_key("X-Amz-Signature"), "{url}");
    // The same link within a window, so the browser cache works.
    assert_eq!(redirect.download_url(ID, &IMAGE), Some(url));
    assert_eq!(redirect.download_url("../x", &IMAGE), None);

    // The link really serves the object.
    proxy.put(ID, body(&[b"png"]), 10).await.unwrap();
    let mut store = config(&fake.endpoint, "", Downloads::Redirect);
    store.prefix.clear();
    let redirect = S3FileStore::new(dir.path(), store).unwrap();
    let url = redirect.download_url(ID, &IMAGE).unwrap();
    let served = reqwest::get(url).await.unwrap();
    assert_eq!(served.status(), 200);
    assert_eq!(served.bytes().await.unwrap(), "png");
}

/// Runs the contract on a real server, such as MinIO, when these are set:
/// `CADOCS_TEST_S3_ENDPOINT`, `_BUCKET`, `_ACCESS_KEY_ID` and `_SECRET_ACCESS_KEY`. The bucket
/// must exist, and the test works under a prefix of its own, which it leaves empty.
#[tokio::test]
async fn real_s3() {
    let Ok(endpoint) = std::env::var("CADOCS_TEST_S3_ENDPOINT") else {
        eprintln!("CADOCS_TEST_S3_ENDPOINT is not set: skipped");
        return;
    };
    let var = |name: &str| std::env::var(format!("CADOCS_TEST_S3_{name}")).unwrap();
    let prefix = format!("cadocs-test-{}", std::process::id());
    let config = S3Config {
        endpoint,
        bucket: var("BUCKET"),
        region: std::env::var("CADOCS_TEST_S3_REGION").unwrap_or("us-east-1".to_owned()),
        access_key_id: var("ACCESS_KEY_ID"),
        secret_access_key: var("SECRET_ACCESS_KEY"),
        prefix,
        downloads: Downloads::Redirect,
        link_lifetime: Duration::from_secs(600),
        ca_file: None,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = S3FileStore::open(dir.path(), config).await.unwrap();
    contract(&store, &dir.path().join("tmp")).await;

    store.put(ID, body(&[b"png"]), 10).await.unwrap();
    let url = store.download_url(ID, &IMAGE).unwrap();
    let served = reqwest::get(url).await.unwrap();
    assert_eq!(served.status(), 200);
    assert_eq!(
        served.headers()["content-disposition"],
        "inline; filename=\"a.png\""
    );
    assert_eq!(served.headers()["content-type"], "image/png");
    assert_eq!(served.bytes().await.unwrap(), "png");
    store.delete(ID).await.unwrap();
}

#[tokio::test]
async fn a_prefix_or_endpoint_that_would_leave_the_bucket_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for prefix in ["/cadocs", "../x", "a/../b", "./a", "a//b", "/"] {
        let refused = S3FileStore::new(dir.path(), config("http://127.0.0.1:9", prefix, Downloads::Proxy));
        assert!(refused.is_err(), "prefix {prefix:?}");
    }
    for prefix in ["", "cadocs", "cadocs/", "apps/cadocs"] {
        let kept = S3FileStore::new(dir.path(), config("http://127.0.0.1:9", prefix, Downloads::Proxy));
        assert!(kept.is_ok(), "prefix {prefix:?}");
    }
    // A path on the endpoint would be dropped by the bucket's URL.
    // So would credentials in it, which would also go out as a Basic header.
    for endpoint in ["http://127.0.0.1:9/s3", "http://127.0.0.1:9/s3/", "http://u:p@127.0.0.1:9"] {
        let refused = S3FileStore::new(dir.path(), config(endpoint, "", Downloads::Proxy));
        assert!(refused.is_err(), "endpoint {endpoint:?}");
    }
    assert!(S3FileStore::new(dir.path(), config("http://127.0.0.1:9/", "", Downloads::Proxy)).is_ok());
}

#[tokio::test]
async fn a_transport_error_never_shows_a_presigned_url() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing listens on port 9 of the loopback.
    let store = S3FileStore::new(dir.path(), config("http://127.0.0.1:9", "", Downloads::Proxy)).unwrap();
    let errors = [
        store.get(ID).await.err().unwrap().to_string(),
        store.delete(ID).await.err().unwrap().to_string(),
        store.put(ID, body(&[b"x"]), 10).await.err().unwrap().to_string(),
        store.list().next().await.unwrap().err().unwrap().to_string(),
    ];
    for error in errors {
        assert!(!error.contains("X-Amz-"), "{error}");
    }
}
