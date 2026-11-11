//! Attachments in an S3 bucket (ADR 0020). `rusty-s3` signs each request as a presigned URL and
//! parses listings, and `reqwest` sends them. Keys repeat the layout of `files/` under a prefix,
//! so a local folder can be mirrored to the bucket as it is.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures_util::{TryStreamExt, stream};
use reqwest::header::CONTENT_LENGTH;
use reqwest::{Body, Client, Method, Response, StatusCode};
use rusty_s3::actions::{DeleteObject, GetObject, HeadBucket, ListObjectsV2, PutObject};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use tokio::fs::File;

use crate::{
    BoxFuture, ByteStream, Download, FileBody, FileError, FileList, FileStore, LIST_BATCH, Stored,
    StoredFile, empty_tmp, read_stream, receive_temp, relative_path,
};

/// How long a signed request stays valid. It only has to reach the server.
const REQUEST_LIFETIME: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Silence on a connection before a request fails. Not a bound on a whole transfer, which can
/// take long for a large file on a slow link.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle connections kept, each with its TLS state.
const IDLE_CONNECTIONS: usize = 2;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Downloads {
    /// The server streams each file.
    Proxy,
    /// The server answers with a presigned link to the bucket.
    Redirect,
}

#[derive(Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    /// Put before every key, so applications can share a bucket. A `/` is added if missing.
    pub prefix: String,
    pub downloads: Downloads,
    /// How long a download link is valid. Links are signed at the start of a window of half
    /// that, so a link stays the same within a window and lives at least that long.
    pub link_lifetime: Duration,
    /// A PEM bundle trusted besides the system roots, for an internal certificate authority.
    pub ca_file: Option<PathBuf>,
}

/// Leaves the secret out, so a logged configuration never shows it.
impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field("prefix", &self.prefix)
            .field("downloads", &self.downloads)
            .field("link_lifetime", &self.link_lifetime)
            .field("ca_file", &self.ca_file)
            .finish_non_exhaustive()
    }
}

/// Cheap to clone: a listing holds a copy while it runs.
#[derive(Clone)]
pub struct S3FileStore {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    bucket: Bucket,
    credentials: Credentials,
    prefix: String,
    downloads: Downloads,
    link_lifetime: Duration,
    /// Scheme, host and port of the endpoint.
    origin: String,
    tmp: PathBuf,
}

impl S3FileStore {
    /// Empties `<data>/tmp`, as [`crate::LocalFileStore::open`] does, and checks nothing remote.
    /// Call it once the data folder is locked.
    pub fn new(data_dir: &Path, config: S3Config) -> Result<Self, FileError> {
        if !valid_prefix(&config.prefix) {
            return Err(invalid_input(format!("invalid S3 prefix {:?}", config.prefix)));
        }
        let endpoint: url::Url = config.endpoint.parse().map_err(invalid)?;
        // The bucket's URL is the endpoint joined with `<bucket>/`, which drops any path, and
        // credentials in it would go out as a Basic header beside the signature.
        let credentials = !endpoint.username().is_empty() || endpoint.password().is_some();
        if endpoint.path() != "/" || endpoint.query().is_some() || credentials {
            return Err(invalid_input(
                "the S3 endpoint must have no path, query or credentials".to_owned(),
            ));
        }
        let bucket = Bucket::new(endpoint, UrlStyle::Path, config.bucket, config.region)
            .map_err(invalid)?;
        let origin = bucket.base_url().origin().ascii_serialization();
        let mut client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .pool_max_idle_per_host(IDLE_CONNECTIONS)
            .pool_idle_timeout(IDLE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(path) = &config.ca_file {
            let pem = std::fs::read(path)?;
            for certificate in reqwest::Certificate::from_pem_bundle(&pem).map_err(invalid)? {
                client = client.add_root_certificate(certificate);
            }
        }
        let mut prefix = config.prefix;
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        Ok(Self {
            inner: Arc::new(Inner {
                client: client.build().map_err(invalid)?,
                bucket,
                credentials: Credentials::new(config.access_key_id, config.secret_access_key),
                prefix,
                downloads: config.downloads,
                link_lifetime: config.link_lifetime.max(Duration::from_secs(2)),
                origin,
                tmp: empty_tmp(data_dir)?,
            }),
        })
    }

    /// [`S3FileStore::new`], then a `HEAD` on the bucket, so a wrong setting fails at startup.
    pub async fn open(data_dir: &Path, config: S3Config) -> Result<Self, FileError> {
        let store = Self::new(data_dir, config)?;
        let inner = &store.inner;
        let url = HeadBucket::new(&inner.bucket, Some(&inner.credentials)).sign(REQUEST_LIFETIME);
        let response = inner.send(Method::HEAD, url, None).await?;
        expect(&response, &[StatusCode::OK], "HEAD bucket")?;
        Ok(store)
    }
}

impl Inner {
    fn key(&self, id: &str) -> Result<String, FileError> {
        Ok(format!("{}{}", self.prefix, relative_path(id)?))
    }

    async fn send(
        &self,
        method: Method,
        url: url::Url,
        body: Option<(Body, u64)>,
    ) -> Result<Response, FileError> {
        let mut request = self.client.request(method, url.as_str());
        if let Some((body, length)) = body {
            request = request.header(CONTENT_LENGTH, length).body(body);
        }
        request.send().await.map_err(transport)
    }

    async fn put_file(
        &self,
        id: &str,
        body: ByteStream<'_>,
        max_bytes: u64,
    ) -> Result<Stored, FileError> {
        let key = self.key(id)?;
        let (temp, stored) = receive_temp(&self.tmp, id, body, max_bytes).await?;
        let file = File::open(&temp.0).await?;
        let url = PutObject::new(&self.bucket, Some(&self.credentials), &key).sign(REQUEST_LIFETIME);
        let body = Body::wrap_stream(read_stream(file));
        let response = self.send(Method::PUT, url, Some((body, stored.size))).await?;
        expect(&response, &[StatusCode::OK], "PUT")?;
        Ok(stored)
    }

    async fn get_file(&self, id: &str) -> Result<FileBody, FileError> {
        let key = self.key(id)?;
        let url = GetObject::new(&self.bucket, Some(&self.credentials), &key).sign(REQUEST_LIFETIME);
        let response = self.send(Method::GET, url, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(FileError::NotFound);
        }
        expect(&response, &[StatusCode::OK], "GET")?;
        let size = response
            .content_length()
            .ok_or_else(|| io::Error::other("S3 sent no Content-Length"))?;
        let bytes = response.bytes_stream().map_err(io::Error::other);
        Ok(FileBody {
            size,
            bytes: Box::pin(bytes),
        })
    }

    async fn delete_file(&self, id: &str) -> Result<(), FileError> {
        let key = self.key(id)?;
        let action = DeleteObject::new(&self.bucket, Some(&self.credentials), &key);
        let response = self.send(Method::DELETE, action.sign(REQUEST_LIFETIME), None).await?;
        let done = [StatusCode::NO_CONTENT, StatusCode::OK, StatusCode::NOT_FOUND];
        expect(&response, &done, "DELETE")
    }

    /// One page of the listing, keeping the keys that name a file as this store writes them.
    async fn list_page(
        &self,
        token: Option<&str>,
    ) -> Result<(Vec<StoredFile>, Option<String>), FileError> {
        let mut action = ListObjectsV2::new(&self.bucket, Some(&self.credentials));
        action.with_max_keys(LIST_BATCH);
        if !self.prefix.is_empty() {
            action.with_prefix(self.prefix.as_str());
        }
        if let Some(token) = token {
            action.with_continuation_token(token);
        }
        let response = self.send(Method::GET, action.sign(REQUEST_LIFETIME), None).await?;
        expect(&response, &[StatusCode::OK], "list")?;
        let xml = response.bytes().await.map_err(transport)?;
        let xml = std::str::from_utf8(&xml).map_err(invalid)?;
        let page = ListObjectsV2::parse_response(xml).map_err(invalid)?;
        let mut files = Vec::with_capacity(page.contents.len());
        for object in page.contents {
            let Some(relative) = object.key.strip_prefix(&self.prefix) else {
                continue;
            };
            let Some(id) = relative.rsplit('/').next() else {
                continue;
            };
            if relative_path(id).ok().as_deref() != Some(relative) {
                continue;
            }
            // With no date, the file counts as new, so the cleanup never takes it for an old stray.
            let modified = match object.last_modified {
                Some(date) => SystemTime::from(date.parse::<jiff::Timestamp>().map_err(invalid)?),
                None => SystemTime::now(),
            };
            files.push(StoredFile {
                id: id.to_owned(),
                modified,
            });
        }
        Ok((files, page.next_continuation_token))
    }

    fn download_url(&self, id: &str, download: &Download<'_>) -> Option<String> {
        if self.downloads != Downloads::Redirect {
            return None;
        }
        let key = self.key(id).ok()?;
        let window = (self.link_lifetime / 2).as_secs();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs();
        let start = jiff::Timestamp::from_second((now - now % window) as i64).ok()?;
        let cache = format!("private, max-age={window}");
        let mut action = GetObject::new(&self.bucket, Some(&self.credentials), &key);
        let query = action.query_mut();
        query.insert("response-content-type", download.content_type);
        query.insert("response-content-disposition", download.disposition);
        query.insert("response-cache-control", cache);
        Some(action.sign_with_time(self.link_lifetime, &start).into())
    }
}

impl FileStore for S3FileStore {
    fn put<'a>(
        &'a self,
        id: &'a str,
        body: ByteStream<'a>,
        max_bytes: u64,
    ) -> BoxFuture<'a, Result<Stored, FileError>> {
        Box::pin(self.inner.put_file(id, body, max_bytes))
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<FileBody, FileError>> {
        Box::pin(self.inner.get_file(id))
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(self.inner.delete_file(id))
    }

    /// Under `<data>/tmp`, as on disk: an import's zip never goes to the bucket.
    fn scratch(&self) -> io::Result<std::fs::File> {
        tempfile::tempfile_in(&self.inner.tmp)
    }

    fn list(&self) -> FileList {
        enum Next {
            Start,
            After(String),
            Done,
        }
        let inner = self.inner.clone();
        Box::pin(stream::unfold(Next::Start, move |mut next| {
            let inner = inner.clone();
            async move {
                // Pages whose keys all belong to something else are skipped, so no batch is empty.
                loop {
                    let token = match &next {
                        Next::Start => None,
                        Next::After(token) => Some(token.as_str()),
                        Next::Done => return None,
                    };
                    match inner.list_page(token).await {
                        Err(error) => return Some((Err(error), Next::Done)),
                        Ok((files, token)) => {
                            next = token.map_or(Next::Done, Next::After);
                            if !files.is_empty() {
                                return Some((Ok(files), next));
                            }
                        }
                    }
                }
            }
        }))
    }

    fn download_url(&self, id: &str, download: &Download<'_>) -> Option<String> {
        self.inner.download_url(id, download)
    }

    fn download_origin(&self) -> Option<String> {
        (self.inner.downloads == Downloads::Redirect).then(|| self.inner.origin.clone())
    }
}

fn expect(response: &Response, accepted: &[StatusCode], what: &str) -> Result<(), FileError> {
    let status = response.status();
    if accepted.contains(&status) {
        Ok(())
    } else {
        Err(io::Error::other(format!("S3 answered {status} to {what}")).into())
    }
}

/// Without its URL: a presigned one works for a while as a bearer token, and errors get logged.
fn transport(error: reqwest::Error) -> FileError {
    io::Error::other(error.without_url()).into()
}

/// A prefix joins the bucket's URL as a relative path, so it must not start at the root or walk
/// out of the bucket: no leading `/`, and no empty, `.` or `..` segment before its last `/`.
pub fn valid_prefix(prefix: &str) -> bool {
    let inner = prefix.strip_suffix('/').unwrap_or(prefix);
    prefix.is_empty()
        || (!inner.is_empty()
            && inner
                .split('/')
                .all(|segment| !matches!(segment, "" | "." | "..")))
}

fn invalid_input(message: String) -> FileError {
    io::Error::new(io::ErrorKind::InvalidInput, message).into()
}

fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> FileError {
    io::Error::new(io::ErrorKind::InvalidInput, error).into()
}
