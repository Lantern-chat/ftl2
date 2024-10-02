#![allow(clippy::multiple_bound_locations)]

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
use crate::headers::entity_tag::{EntityTag, IfNoneMatch};
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

pub trait FileCacheExtra<S: Send + Sync>: FileCache<S> {
    #[inline]
    fn file(&self, parts: &RequestParts, state: &S, path: impl AsRef<Path>) -> impl Future<Output = Response> {
        file(parts, state, path, self)
    }

    #[inline]
    fn dir(
        &self,
        parts: &RequestParts,
        state: &S,
        path: impl AsRef<str>,
        base: impl Into<PathBuf>,
    ) -> impl Future<Output = Response> {
        dir(parts, state, path, base, self)
    }
}

impl<S: Send + Sync, F> FileCacheExtra<S> for F where F: FileCache<S> {}

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
    // NOTE: Only use if-none-match due to its weak comparison semantics,
    // whereas if-match always requires a strong match and would thus
    // always fail for files.
    if_none_match: Option<IfNoneMatch>,
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
            if_none_match: parts.headers.typed_get(),
        }
    }

    pub fn check(self, last_modified: Option<LastModified>, etag: &EntityTag) -> Cond {
        if let Some(if_none_match) = self.if_none_match {
            log::trace!("if-none-match? {if_none_match:?} vs {etag:?}",);

            // "When the condition fails for GET and HEAD methods,
            //  then the server must return HTTP status code 304 (Not Modified)"
            if if_none_match.iter().any(|e| e.weak_eq(etag)) {
                return Cond::NoBody(StatusCode::NOT_MODIFIED);
            }
        }

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

pub async fn file<S: Send + Sync, F: FileCache<S> + ?Sized>(
    parts: &RequestParts,
    state: &S,
    request_path: impl AsRef<Path>,
    cache: &F,
) -> Response {
    if !cache.is_method_allowed(&parts.method) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    file_reply(parts, state, request_path, cache, None).await
}

pub async fn dir<S: Send + Sync, F: FileCache<S> + ?Sized>(
    parts: &RequestParts,
    state: &S,
    request_path: impl AsRef<str>,
    base: impl Into<PathBuf>,
    cache: &F,
) -> Response {
    if !cache.is_method_allowed(&parts.method) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let mut buf = match sanitize_path(base, request_path.as_ref()) {
        Ok(buf) => buf,
        Err(e) => return e.to_string().with_status(StatusCode::BAD_REQUEST).into_response(),
    };

    let metadata = match cache.metadata(&buf, state).await {
        Ok(meta) => {
            if meta.is_dir() {
                log::debug!("dir: appending index.html to directory path");
                buf.push("index.html");
                None // not applicable
            } else {
                Some(meta)
            }
        }
        _ => None, // TODO: Should this be an error?
    };

    file_reply(parts, state, buf, cache, metadata).await
}

async fn file_reply<S: Send + Sync, F: FileCache<S> + ?Sized>(
    req: &RequestParts,
    state: &S,
    path: impl AsRef<Path>,
    cache: &F,
    metadata: Option<F::Meta>,
) -> Response {
    let req_start = Instant::now();

    let path = path.as_ref();

    let range = req.headers.typed_get::<headers::Range>();

    // if a range is given, do not use pre-compression
    let accepts = match range {
        None => req.headers.typed_get::<AcceptEncoding>(),
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

    let metadata = match metadata {
        Some(metadata) => metadata,
        None => match cache.file_metadata(&file, state).await {
            Ok(m) => m,
            Err(e) => {
                log::error!("Error retreiving file metadata: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
    };

    // parse after opening the file handle to save time on open error
    let conditionals = Conditionals::new(req, range);

    let modified = metadata.modified().ok();
    let last_modified = modified.map(LastModified::from);

    let mut len = metadata.len();

    let etag = EntityTag::from_file(
        modified.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()),
        len,
    );

    match conditionals.check(last_modified, &etag) {
        Cond::NoBody(resp) => resp.with_header(etag).into_response(),
        Cond::WithBody(range) => match bytes_range(range, len) {
            Err(_) => {
                StatusCode::RANGE_NOT_SATISFIABLE.with_header(ContentRange::unsatisfied_bytes(len)).into_response()
            }

            Ok((start, end)) => {
                let sub_len = end - start;
                let buf_size = metadata.blksize().max(DEFAULT_READ_BUF_SIZE).min(len) as usize;
                let encoding = file.encoding();

                let mut body = Body::empty();
                let mut parts = http::response::Response::new(()).into_parts().0; // this is stupid, only way to create Parts

                parts.headers.reserve(7); // might overallocate a bit, but that's better than multiple reallocations

                parts.headers.typed_insert(etag);

                let is_partial = sub_len != len;

                if is_partial {
                    assert_eq!(encoding, ContentEncoding::Identity);

                    parts.status = StatusCode::PARTIAL_CONTENT;
                    parts.headers.typed_insert(ContentRange::bytes(start..end, len).expect("valid ContentRange"));

                    len = sub_len;
                }

                if req.method == Method::GET {
                    if !is_partial {
                        if let Some(full) = file.full() {
                            body = full.into();
                        }
                    } else if start != 0 {
                        if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                            return crate::Error::IoError(e).into_response();
                        }
                    }

                    // only create a body if there isn't one already, like from Full files
                    if body.is_empty() {
                        body = Body::wrap(crate::body::async_read::AsyncReadBody::new(
                            file, buf_size, req_start, len,
                        ));

                        parts.headers.insert(TRAILER, const { HeaderValue::from_static("server-timing") });
                    }
                };

                if let Some(last_modified) = last_modified {
                    parts.headers.typed_insert(last_modified);
                }

                if encoding != ContentEncoding::Identity {
                    parts.headers.typed_insert(encoding);
                }

                parts.headers.typed_insert(ContentLength(len));
                parts.headers.typed_insert(AcceptRanges::bytes());

                let mime = path
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .and_then(|ext| mime_db::lookup_ext(ext)?.types.first().copied())
                    .unwrap_or("application/octet-stream");

                parts.headers.append(
                    const { HeaderName::from_static("content-type") },
                    HeaderValue::from_static(mime),
                );

                http::Response::from_parts(parts, body)
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
