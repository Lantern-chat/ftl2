use std::future::Future;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs::Metadata, io, time::Instant};

use bytes::Bytes;
use tokio::fs::File as TkFile;
use tokio::io::{AsyncRead, AsyncSeek, AsyncSeekExt};

use http::{header::TRAILER, HeaderName, HeaderValue, Method, StatusCode};
use percent_encoding::percent_decode_str;

use crate::headers::accept_encoding::{AcceptEncoding, ContentEncoding};
use headers::{
    AcceptRanges, ContentLength, ContentRange, HeaderMapExt, IfModifiedSince, IfRange, IfUnmodifiedSince,
    LastModified, Range,
};

use crate::{body::Body, IntoResponse, RequestParts, Response};

// TODO: https://github.com/magiclen/entity-tag/blob/master/src/lib.rs
// https://github.com/pillarjs/send/blob/master/index.js
// https://github.com/jshttp/etag/blob/master/index.js

pub trait GenericFile: Unpin + AsyncRead + AsyncSeek + Send + 'static {}
impl<T> GenericFile for T where T: Unpin + AsyncRead + AsyncSeek + Send + 'static {}

pub trait EncodedFile {
    fn encoding(&self) -> ContentEncoding;

    fn full(&self) -> Option<Bytes> {
        None
    }
}

impl EncodedFile for TkFile {
    #[inline]
    fn encoding(&self) -> ContentEncoding {
        ContentEncoding::Identity
    }
}

pub trait FileMetadata {
    fn is_dir(&self) -> bool;
    fn len(&self) -> u64;
    fn modified(&self) -> io::Result<SystemTime>;
    fn blksize(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl FileMetadata for Metadata {
    #[inline]
    fn is_dir(&self) -> bool {
        Metadata::is_dir(self)
    }

    #[inline]
    fn modified(&self) -> io::Result<SystemTime> {
        Metadata::modified(self)
    }

    #[inline]
    fn len(&self) -> u64 {
        Metadata::len(self)
    }

    #[cfg(unix)]
    #[inline]
    fn blksize(&self) -> u64 {
        use std::os::unix::fs::MetadataExt;

        MetadataExt::blksize(self)
    }

    #[cfg(not(unix))]
    #[inline]
    fn blksize(&self) -> u64 {
        0
    }
}

pub trait FileCache<S: Send + Sync> {
    type File: GenericFile + EncodedFile;
    type Meta: FileMetadata;

    /// Clear the file cache, unload all files
    fn clear(&self, state: &S) -> impl Future<Output = ()> + Send;

    /// Open a file and return it
    fn open(
        &self,
        path: &Path,
        accepts: Option<AcceptEncoding>,
        state: &S,
    ) -> impl Future<Output = io::Result<Self::File>> + Send;

    /// Retrieve the file's metadata from path
    fn metadata(&self, path: &Path, state: &S) -> impl Future<Output = io::Result<Self::Meta>> + Send;

    /// Retrieve the file's metadata from an already opened file
    fn file_metadata(&self, file: &Self::File, state: &S) -> impl Future<Output = io::Result<Self::Meta>> + Send;

    /// Check if the method is allowed. By default, only GET and HEAD are allowed,
    /// anything else will return a 405 Method Not Allowed
    fn is_method_allowed(&self, method: &Method) -> bool {
        method == Method::GET || method == Method::HEAD
    }
}

impl<S: Send + Sync, F: FileCache<S>> FileCache<S> for &F {
    type File = F::File;
    type Meta = F::Meta;

    #[inline(always)]
    fn clear(&self, state: &S) -> impl Future<Output = ()> + Send {
        (**self).clear(state)
    }

    #[inline(always)]
    fn open(
        &self,
        path: &Path,
        accepts: Option<AcceptEncoding>,
        state: &S,
    ) -> impl Future<Output = io::Result<Self::File>> + Send {
        (**self).open(path, accepts, state)
    }

    #[inline(always)]
    fn metadata(&self, path: &Path, state: &S) -> impl Future<Output = io::Result<Self::Meta>> + Send {
        (**self).metadata(path, state)
    }

    #[inline(always)]
    fn file_metadata(&self, file: &Self::File, state: &S) -> impl Future<Output = io::Result<Self::Meta>> + Send {
        (**self).file_metadata(file, state)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoCache;

impl<S: Send + Sync> FileCache<S> for NoCache {
    type File = TkFile;
    type Meta = Metadata;

    #[inline]
    async fn clear(&self, _state: &S) {
        // Nothing to do here
    }

    #[inline]
    async fn open(&self, path: &Path, _accepts: Option<AcceptEncoding>, _state: &S) -> io::Result<Self::File> {
        TkFile::open(path).await
    }

    #[inline]
    async fn metadata(&self, path: &Path, _state: &S) -> io::Result<Self::Meta> {
        tokio::fs::metadata(path).await
    }

    #[inline]
    async fn file_metadata(&self, file: &Self::File, _state: &S) -> io::Result<Self::Meta> {
        file.metadata().await
    }
}

#[derive(Debug)]
pub struct Conditionals {
    if_modified_since: Option<IfModifiedSince>,
    if_unmodified_since: Option<IfUnmodifiedSince>,
    if_range: Option<IfRange>,
    range: Option<Range>,
}

pub enum Cond {
    NoBody(StatusCode),
    WithBody(Option<Range>),
}

impl Conditionals {
    pub fn new(parts: &RequestParts, range: Option<Range>) -> Conditionals {
        Conditionals {
            range,
            if_modified_since: parts.headers.typed_get(),
            if_unmodified_since: parts.headers.typed_get(),
            if_range: parts.headers.typed_get(),
        }
    }

    pub fn check(self, last_modified: Option<LastModified>) -> Cond {
        if let Some(since) = self.if_unmodified_since {
            let precondition = last_modified.map(|time| since.precondition_passes(time.into())).unwrap_or(false);

            log::trace!("if-unmodified-since? {since:?} vs {last_modified:?} = {precondition}",);

            if !precondition {
                return Cond::NoBody(StatusCode::PRECONDITION_FAILED);
            }
        }

        if let Some(since) = self.if_modified_since {
            log::trace!("if-modified-since? header = {since:?}, file = {last_modified:?}",);

            let unmodified = last_modified
                .map(|time| !since.is_modified(time.into()))
                // no last_modified means its always modified
                .unwrap_or(false);

            if unmodified {
                return Cond::NoBody(StatusCode::NOT_MODIFIED);
            }
        }

        if let Some(if_range) = self.if_range {
            log::trace!("if-range? {:?} vs {:?}", if_range, last_modified);
            let can_range = !if_range.is_modified(None, last_modified.as_ref());

            if !can_range {
                return Cond::WithBody(None);
            }
        }

        Cond::WithBody(self.range)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SanitizeError {
    #[error("Invalid Path")]
    InvalidPath,

    #[error("UTF-8 Error: {0}")]
    Utf8Error(#[from] std::str::Utf8Error),
}

pub fn sanitize_path(base: impl Into<PathBuf>, tail: &str) -> Result<PathBuf, SanitizeError> {
    let mut buf = base.into();
    let p = percent_decode_str(tail).decode_utf8()?;

    for seg in p.split('/') {
        if seg.starts_with("..") {
            log::warn!("dir: rejecting segment starting with '..'");
            return Err(SanitizeError::InvalidPath);
        }

        if seg.contains('\\') {
            log::warn!("dir: rejecting segment containing with backslash (\\)");
            return Err(SanitizeError::InvalidPath);
        }

        if cfg!(windows) && seg.contains(':') {
            log::warn!("dir: rejecting segment containing colon (:)");
            return Err(SanitizeError::InvalidPath);
        }

        buf.push(seg);
    }

    //if !buf.starts_with(base) {
    //    log::warn!("dir: rejecting path that is not a child of base");
    //    return Err(SanitizeError::InvalidPath);
    //}

    Ok(buf)
}

const DEFAULT_READ_BUF_SIZE: u64 = 1024 * 32;

pub async fn file<S: Send + Sync>(
    parts: &RequestParts,
    state: &S,
    request_path: impl AsRef<Path>,
    cache: impl FileCache<S>,
) -> Response {
    if !cache.is_method_allowed(&parts.method) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    file_reply(parts, state, request_path, cache).await
}

pub async fn dir<S: Send + Sync>(
    parts: &RequestParts,
    state: &S,
    request_path: impl AsRef<str>,
    base: impl Into<PathBuf>,
    cache: impl FileCache<S>,
) -> Response {
    if !cache.is_method_allowed(&parts.method) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let mut buf = match sanitize_path(base, request_path.as_ref()) {
        Ok(buf) => buf,
        Err(e) => return e.to_string().with_status(StatusCode::BAD_REQUEST).into_response(),
    };

    let is_dir = cache.metadata(&buf, state).await.map(|m| m.is_dir()).unwrap_or(false);

    if is_dir {
        log::debug!("dir: appending index.html to directory path");
        buf.push("index.html");
    }

    file_reply(parts, state, buf, cache).await
}

async fn file_reply<S: Send + Sync>(
    parts: &RequestParts,
    state: &S,
    path: impl AsRef<Path>,
    cache: impl FileCache<S>,
) -> Response {
    let req_start = Instant::now();

    let path = path.as_ref();

    let range = parts.headers.typed_get::<headers::Range>();

    // if a range is given, do not use pre-compression
    let accepts = match range {
        None => parts.headers.typed_get::<AcceptEncoding>(),
        Some(_) => None,
    };

    let mut file = match cache.open(path, accepts, state).await {
        Ok(f) => f,
        Err(e) => {
            return match e.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
                std::io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN.into_response(),
                _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    };

    let metadata = match cache.file_metadata(&file, state).await {
        Ok(m) => m,
        Err(e) => {
            log::error!("Error retreiving file metadata: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // parse after opening the file handle to save time on open error
    let conditionals = Conditionals::new(parts, range);

    let modified = match metadata.modified() {
        Err(_) => None,
        Ok(t) => Some(LastModified::from(t)),
    };

    let mut len = metadata.len();

    match conditionals.check(modified) {
        Cond::NoBody(resp) => resp.into_response(),
        Cond::WithBody(range) => match bytes_range(range, len) {
            Err(_) => {
                StatusCode::RANGE_NOT_SATISFIABLE.with_header(ContentRange::unsatisfied_bytes(len)).into_response()
            }

            Ok((start, end)) => {
                let sub_len = end - start;
                let buf_size = metadata.blksize().max(DEFAULT_READ_BUF_SIZE).min(len) as usize;
                let encoding = file.encoding();

                let mut resp = Response::default();

                let is_partial = sub_len != len;

                if is_partial {
                    assert_eq!(encoding, ContentEncoding::Identity);

                    *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
                    resp.headers_mut()
                        .typed_insert(ContentRange::bytes(start..end, len).expect("valid ContentRange"));

                    len = sub_len;
                }

                if parts.method == Method::GET {
                    if !is_partial {
                        if let Some(full) = file.full() {
                            *resp.body_mut() = full.into();
                        }
                    } else if start != 0 {
                        if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                            return crate::Error::IoError(e).into_response();
                        }
                    }

                    // only create a body if there isn't one already, like from full files
                    if resp.body().is_empty() {
                        *resp.body_mut() = Body::wrap(crate::body::async_read::AsyncReadBody::new(
                            file, buf_size, req_start, len,
                        ));

                        resp.headers_mut().insert(TRAILER, const { HeaderValue::from_static("server-timing") });
                    }
                };

                let headers = resp.headers_mut();

                if let Some(last_modified) = modified {
                    headers.typed_insert(last_modified);
                }

                if encoding != ContentEncoding::Identity {
                    headers.typed_insert(encoding);
                }

                headers.typed_insert(ContentLength(len));
                headers.typed_insert(AcceptRanges::bytes());

                let mime = path
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .and_then(|ext| mime_db::lookup_ext(ext)?.types.first().copied())
                    .unwrap_or("application/octet-stream");

                headers.append(
                    const { HeaderName::from_static("content-type") },
                    HeaderValue::from_static(mime),
                );

                resp
            }
        },
    }
}

pub struct BadRange;
pub fn bytes_range(range: Option<Range>, max_len: u64) -> Result<(u64, u64), BadRange> {
    use std::ops::Bound;

    match range.and_then(|r| r.satisfiable_ranges(max_len).next()) {
        Some((start, end)) => {
            let start = match start {
                Bound::Unbounded => 0,
                Bound::Included(s) => s,
                Bound::Excluded(s) => s + 1,
            };

            let end = match end {
                Bound::Unbounded => max_len,
                Bound::Included(s) => s + (s != max_len) as u64,
                Bound::Excluded(s) => s,
            };

            if start < end && end <= max_len {
                Ok((start, end))
            } else {
                log::trace!("unsatisfiable byte range: {start}-{end}/{max_len}");
                Err(BadRange)
            }
        }
        None => Ok((0, max_len)),
    }
}
