use crate::ResponseParts;

// pub fn should_compress<P: Predicate>(parts: &Parts, predicate: P) -> bool {
//     // Never compress ranges?
//     // See https://stackoverflow.com/a/53135659
//     if parts.headers.contains_key(http::header::RANGE) {
//         return false;
//     }

//     predicate.should_compress(parts)
// }

pub trait Predicate: Clone + Send + Sync + 'static {
    fn should_compress(&self, parts: &ResponseParts) -> bool;

    #[inline(always)]
    fn and<P>(self, other: P) -> And<Self, P>
    where
        Self: Sized,
        P: Predicate,
    {
        And(self, other)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct And<Lhs, Rhs>(Lhs, Rhs);

impl<Lhs, Rhs> Predicate for And<Lhs, Rhs>
where
    Lhs: Predicate,
    Rhs: Predicate,
{
    #[inline]
    fn should_compress(&self, parts: &ResponseParts) -> bool {
        self.0.should_compress(parts) && self.1.should_compress(parts)
    }
}

impl<F> Predicate for F
where
    F: Fn(&ResponseParts) -> bool + Clone + Send + Sync + 'static,
{
    #[inline]
    fn should_compress(&self, parts: &ResponseParts) -> bool {
        self(parts)
    }
}

impl Predicate for bool {
    #[inline]
    fn should_compress(&self, _: &ResponseParts) -> bool {
        *self
    }
}

/// Default predicate for compression, attempting intelligent compression
/// based on content type and size.
///
/// It compresses responses with a content size greater than 1024 bytes,
/// except for images/video/audio, gRPC, and event-streams. SVG images are compressed,
/// however, as they are text-based. The predicate also checks for common compressed
/// content types and skips re-compression for those.
#[derive(Default, Clone, Copy, Debug)]
pub struct DefaultPredicate;

const MIN_CONTENT_SIZE: usize = 1024;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, Anchored, Input, MatchKind, StartKind};
use std::sync::LazyLock;

static INCOMPRESSIBLE_MIMES: LazyLock<AhoCorasick> = LazyLock::new(|| {
    #[rustfmt::skip]
    let built_in_patterns = [
        "image/", "video/", "audio/", // media types
        "application/ogg",   // OGG/OGX media format
        "application/grpc",  // gRPC
        "text/event-stream", // Server-Sent Events
        // pre-compressed formats
        "application/x-bzip", "application/x-bzip2",
        "application/gzip",
        "application/zip", "application/x-zip", "x-zip-compressed", "application/x-zip-compressed",
        "application/x-7z-compressed",
        "application/vnd.rar", "application/x-rar-compressed",
    ];

    #[cfg(feature = "mime_db")]
    let patterns = built_in_patterns.into_iter().chain(mime_db::list_mimes().filter_map(|(mime, info)| {
        // skip generic media types already defined
        if mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/") {
            return None;
        }

        (info.compressible == mime_db::Compressible::No).then_some(mime)
    }));

    #[cfg(not(feature = "mime_db"))]
    let patterns = built_in_patterns.into_iter();

    AhoCorasickBuilder::new()
        .ascii_case_insensitive(true) // may as well
        .match_kind(MatchKind::LeftmostFirst)
        .start_kind(StartKind::Anchored)
        .build(patterns)
        .expect("Failed to build AhoCorasick matcher for incompressible MIME types")
});

impl Predicate for DefaultPredicate {
    fn should_compress(&self, parts: &ResponseParts) -> bool {
        let mut should_compress = match content_size(parts) {
            Some(content_size) => content_size >= MIN_CONTENT_SIZE,
            None => true, // assume dynamic stream size is compressible
        };

        should_compress = should_compress && {
            let ty = content_type(parts);

            !INCOMPRESSIBLE_MIMES.is_match(Input::new(ty).anchored(Anchored::Yes))
                || ty.starts_with("image/svg+xml")
        };

        should_compress
    }
}

fn content_type(response: &ResponseParts) -> &str {
    response.headers.get(http::header::CONTENT_TYPE).and_then(|h| h.to_str().ok()).unwrap_or_default()
}

fn content_size(response: &ResponseParts) -> Option<usize> {
    response
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse().ok())
}
