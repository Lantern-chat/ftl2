use core::fmt::{self, Write};
use std::time::Duration;

use headers::Header;
use http::HeaderValue;

type Buffer = str_buf::StrBuf<62>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[must_use]
pub struct EntityTag {
    pub weak: bool,

    tag: Buffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityTagError {
    InvalidFormat,
    NotAscii,
    Overflow,
}

impl EntityTag {
    pub const fn checked_new(weak: bool, tag: &str) -> Result<Self, EntityTagError> {
        if tag.len() > 62 {
            return Err(EntityTagError::Overflow);
        }

        if !tag.is_ascii() {
            return Err(EntityTagError::NotAscii);
        }

        Ok(Self {
            weak,
            tag: Buffer::from_str(tag),
        })
    }

    pub const fn new(weak: bool, tag: &str) -> Self {
        match Self::checked_new(weak, tag) {
            Ok(etag) => etag,
            Err(EntityTagError::NotAscii) => panic!("EntityTag must be ASCII"),
            Err(EntityTagError::Overflow) => panic!("EntityTag must be at most 62 bytes"),
            Err(EntityTagError::InvalidFormat) => panic!("EntityTag must be a valid tag"),
        }
    }

    pub const fn weak(tag: &str) -> Self {
        Self::new(true, tag)
    }

    pub const fn checked_weak(tag: &str) -> Result<Self, EntityTagError> {
        Self::checked_new(true, tag)
    }

    pub const fn strong(tag: &str) -> Self {
        Self::new(false, tag)
    }

    pub const fn checked_strong(tag: &str) -> Result<Self, EntityTagError> {
        Self::checked_new(false, tag)
    }

    #[must_use]
    pub const fn tag(&self) -> &str {
        self.tag.as_str()
    }

    /// Create a new Weak EntityTag from a file's age (optional) and length, where the age
    /// is difference between the file's last modified time and the UNIX epoch.
    pub fn from_file(age: Option<Duration>, len: u64) -> Self {
        let mut tag = Buffer::new();

        _ = match age {
            Some(age) => write!(tag, "{}.{}-{}", age.as_secs(), age.subsec_nanos(), len),
            None => write!(tag, "{}", len),
        };

        Self { weak: true, tag }
    }

    #[must_use]
    pub fn strong_eq(&self, other: &EntityTag) -> bool {
        !self.weak && !other.weak && self.tag.as_str() == other.tag.as_str()
    }

    #[must_use]
    pub fn weak_eq(&self, other: &EntityTag) -> bool {
        self.tag.as_str() == other.tag.as_str()
    }
}

impl fmt::Display for EntityTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.weak {
            f.write_str("W/")?;
        }

        f.write_str("\"")?;
        f.write_str(&self.tag)?;
        f.write_str("\"")
    }
}

impl core::str::FromStr for EntityTag {
    type Err = EntityTagError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim();
        let len = text.len();

        if !text.ends_with('"') || len < 2 {
            return Err(EntityTagError::InvalidFormat);
        }

        if text.starts_with('"') {
            EntityTag::checked_strong(&text[1..len - 1])
        } else if len >= 4 && text.starts_with("W/\"") {
            EntityTag::checked_weak(&text[3..len - 1])
        } else {
            Err(EntityTagError::InvalidFormat)
        }
    }
}

impl Header for EntityTag {
    fn name() -> &'static http::HeaderName {
        &http::header::ETAG
    }

    fn encode<E: Extend<http::HeaderValue>>(&self, values: &mut E) {
        if let Ok(value) = HeaderValue::try_from(self.to_string()) {
            values.extend(Some(value))
        }
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        values
            .next()
            .and_then(|hdr| hdr.to_str().ok())
            .and_then(|hdr| hdr.parse().ok())
            .ok_or_else(headers::Error::invalid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct IfMatch(pub Vec<EntityTag>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct IfNoneMatch(pub Vec<EntityTag>);

use core::ops::Deref;

impl Deref for IfMatch {
    type Target = [EntityTag];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for IfNoneMatch {
    type Target = [EntityTag];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Header for IfMatch {
    fn name() -> &'static http::HeaderName {
        &http::header::IF_MATCH
    }

    fn encode<E: Extend<http::HeaderValue>>(&self, values: &mut E) {
        encode_etag_list(values, &self.0)
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        parse_etag_list(values).map(Self)
    }
}

impl Header for IfNoneMatch {
    fn name() -> &'static http::HeaderName {
        &http::header::IF_NONE_MATCH
    }

    fn encode<E: Extend<http::HeaderValue>>(&self, values: &mut E) {
        encode_etag_list(values, &self.0)
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        parse_etag_list(values).map(Self)
    }
}

fn parse_etag_list<'i, I>(values: &mut I) -> Result<Vec<EntityTag>, headers::Error>
where
    I: Iterator<Item = &'i HeaderValue>,
{
    let mut etags = Vec::new();

    for value in values.filter_map(|hval| hval.to_str().ok()).flat_map(|s| s.split(',')) {
        etags.push(value.parse().map_err(|_| headers::Error::invalid())?);
    }

    Ok(etags)
}

fn encode_etag_list<E: Extend<HeaderValue>>(values: &mut E, etags: &[EntityTag]) {
    let mut value = String::with_capacity(etags.len() * 16);

    for etag in etags.iter() {
        if !value.is_empty() {
            value.push_str(", ");
        }

        _ = write!(value, "{etag}");
    }

    if let Ok(value) = HeaderValue::try_from(value) {
        values.extend(Some(value))
    }
}
