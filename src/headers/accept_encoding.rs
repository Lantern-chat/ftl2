use headers::Header;
use http::HeaderValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct FilterEncoding {
    pub gzip: bool,
    pub br: bool,
    pub deflate: bool,
    pub zstd: bool,
}

#[allow(clippy::derivable_impls)] // lint doesn't understand cfg!
impl Default for FilterEncoding {
    fn default() -> Self {
        Self {
            gzip: cfg!(any(test, feature = "compression-gzip")),
            br: cfg!(any(test, feature = "compression-br")),
            deflate: cfg!(any(test, feature = "compression-deflate")),
            zstd: cfg!(any(test, feature = "compression-zstd")),
        }
    }
}

impl FilterEncoding {
    pub const fn all() -> Self {
        Self {
            gzip: true,
            br: true,
            deflate: true,
            zstd: true,
        }
    }

    pub const fn none() -> Self {
        Self {
            gzip: false,
            br: false,
            deflate: false,
            zstd: false,
        }
    }

    pub const fn gzip() -> Self {
        Self::none().with_gzip(true)
    }

    pub const fn br() -> Self {
        Self::none().with_br(true)
    }

    pub const fn deflate() -> Self {
        Self::none().with_deflate(true)
    }

    pub const fn zstd() -> Self {
        Self::none().with_zstd(true)
    }

    pub const fn with_gzip(mut self, enable: bool) -> Self {
        self.gzip = enable;
        self
    }

    pub const fn with_br(mut self, enable: bool) -> Self {
        self.br = enable;
        self
    }

    pub const fn with_deflate(mut self, enable: bool) -> Self {
        self.deflate = enable;
        self
    }

    pub const fn with_zstd(mut self, enable: bool) -> Self {
        self.zstd = enable;
        self
    }

    pub fn set_gzip(&mut self, enable: bool) -> &mut Self {
        self.gzip = enable;
        self
    }

    pub fn set_br(&mut self, enable: bool) -> &mut Self {
        self.br = enable;
        self
    }

    pub fn set_deflate(&mut self, enable: bool) -> &mut Self {
        self.deflate = enable;
        self
    }

    pub fn set_zstd(&mut self, enable: bool) -> &mut Self {
        self.zstd = enable;
        self
    }
}

impl core::ops::BitOr for FilterEncoding {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self {
            gzip: self.gzip || rhs.gzip,
            br: self.br || rhs.br,
            deflate: self.deflate || rhs.deflate,
            zstd: self.zstd || rhs.zstd,
        }
    }
}

impl core::ops::BitAnd for FilterEncoding {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self {
            gzip: self.gzip && rhs.gzip,
            br: self.br && rhs.br,
            deflate: self.deflate && rhs.deflate,
            zstd: self.zstd && rhs.zstd,
        }
    }
}

impl core::ops::Not for FilterEncoding {
    type Output = Self;

    fn not(self) -> Self {
        Self {
            gzip: !self.gzip,
            br: !self.br,
            deflate: !self.deflate,
            zstd: !self.zstd,
        }
    }
}

// This enum's variants are ordered from least to most preferred.
#[derive(Default, Clone, Copy, Debug, Ord, PartialOrd, PartialEq, Eq)]
#[must_use]
pub enum ContentEncoding {
    #[default]
    Identity,
    Deflate,
    Gzip,
    Brotli,
    Zstd,
}

impl Header for ContentEncoding {
    fn name() -> &'static http::HeaderName {
        &http::header::CONTENT_ENCODING
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        let mut encoding = ContentEncoding::Identity;

        for value in values.filter_map(|hval| hval.to_str().ok()).flat_map(|s| s.split(',')) {
            encoding = encoding.max(match value.trim() {
                enc if (enc.eq_ignore_ascii_case("gzip") || enc.eq_ignore_ascii_case("x-gzip")) => {
                    ContentEncoding::Gzip
                }
                enc if enc.eq_ignore_ascii_case("br") => ContentEncoding::Brotli,
                enc if enc.eq_ignore_ascii_case("deflate") => ContentEncoding::Deflate,
                enc if enc.eq_ignore_ascii_case("zstd") => ContentEncoding::Zstd,
                _ => continue, // ignore unknown encodings
            });
        }

        Ok(encoding)
    }

    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        values.extend(Some(match self {
            ContentEncoding::Gzip => HeaderValue::from_static("gzip"),
            ContentEncoding::Brotli => HeaderValue::from_static("br"),
            ContentEncoding::Deflate => HeaderValue::from_static("deflate"),
            ContentEncoding::Zstd => HeaderValue::from_static("zstd"),
            ContentEncoding::Identity => return,
        }));
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[must_use]
pub struct AcceptEncoding {
    pub gzip: QValue,
    pub br: QValue,
    pub deflate: QValue,
    pub zstd: QValue,
}

impl AcceptEncoding {
    pub fn preferred_encoding(&self, filter: FilterEncoding) -> ContentEncoding {
        // order encodings by preference
        let list = [
            (ContentEncoding::Deflate, self.deflate, filter.deflate),
            (ContentEncoding::Gzip, self.gzip, filter.gzip),
            (ContentEncoding::Brotli, self.br, filter.br),
            (ContentEncoding::Zstd, self.zstd, filter.zstd),
        ];

        let mut preferred = (ContentEncoding::Identity, QValue(0));

        for &(encoding, qval, enable) in list.iter() {
            // if not filtered out, and is requested, and is more preferred
            // we use >= to prefer the later/higher encoding format if equal
            if enable && qval.0 > 0 && qval >= preferred.1 {
                preferred = (encoding, qval);
            }
        }

        preferred.0
    }

    pub fn into_filter(self) -> FilterEncoding {
        FilterEncoding {
            gzip: self.gzip.0 > 0,
            br: self.br.0 > 0,
            deflate: self.deflate.0 > 0,
            zstd: self.zstd.0 > 0,
        }
    }
}

impl Header for AcceptEncoding {
    fn name() -> &'static http::HeaderName {
        &http::header::ACCEPT_ENCODING
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        #[allow(unused)]
        let mut encodings = AcceptEncoding::default();

        for value in values.filter_map(|hval| hval.to_str().ok()).flat_map(|s| s.split(',')) {
            let mut v = value.splitn(2, ';');

            let Some(encoding) = v.next() else {
                continue; // ignore bad encodings?
            };

            let mut wildcard = QValue(0);

            let encoding = match encoding.trim() {
                enc if enc.eq_ignore_ascii_case("br") => &mut encodings.br,
                enc if enc.eq_ignore_ascii_case("deflate") => &mut encodings.deflate,
                enc if enc.eq_ignore_ascii_case("zstd") => &mut encodings.zstd,
                enc if (enc.eq_ignore_ascii_case("gzip") || enc.eq_ignore_ascii_case("x-gzip")) => {
                    &mut encodings.gzip
                }

                "*" => &mut wildcard,

                _ => continue, // ignore unknown encodings
            };

            *encoding = match v.next() {
                Some(qval) => QValue::parse(qval.trim()).ok_or(headers::Error::invalid())?,
                None => QValue::one(),
            };

            if wildcard.0 > 0 {
                encodings.gzip.wildcard(wildcard);
                encodings.br.wildcard(wildcard);
                encodings.deflate.wildcard(wildcard);
                encodings.zstd.wildcard(wildcard);
            }
        }

        Ok(encodings)
    }

    #[allow(unused)]
    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        use std::fmt::Write;

        let mut s = String::new();

        if self.gzip.0 > 0 {
            write!(s, "gzip;q={}", self.gzip).unwrap();
        }

        if self.br.0 > 0 {
            if !s.is_empty() {
                s.push(',');
            }
            write!(s, "br;q={}", self.br).unwrap();
        }

        if self.deflate.0 > 0 {
            if !s.is_empty() {
                s.push(',');
            }
            write!(s, "deflate;q={}", self.deflate).unwrap();
        }

        if self.zstd.0 > 0 {
            if !s.is_empty() {
                s.push(',');
            }
            write!(s, "zstd;q={}", self.zstd).unwrap();
        }

        if !s.is_empty() {
            values.extend(Some(HeaderValue::from_str(&s).expect("invalid header value")));
        }
    }
}

#[derive(Default, Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[must_use]
#[repr(transparent)]
pub struct QValue(u16);

use std::fmt;

impl fmt::Display for QValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 >= 1000 {
            f.write_str("1")
        } else {
            write!(f, "0.{:03}", self.0)
        }
    }
}

impl QValue {
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value <= 1000 {
            Some(Self(value))
        } else {
            None
        }
    }

    #[inline]
    pub const fn one() -> Self {
        Self(1000)
    }

    pub fn wildcard(&mut self, new: Self) {
        if self.0 == 0 {
            self.0 = new.0;
        }
    }

    // Parse a q-value as specified in RFC 7231 section 5.3.1.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut c = s.chars();
        // Parse "q=" (case-insensitively).
        match c.next() {
            Some('q' | 'Q') => (),
            _ => return None,
        };
        match c.next() {
            Some('=') => (),
            _ => return None,
        };

        // Parse leading digit. Since valid q-values are between 0.000 and 1.000, only "0" and "1"
        // are allowed.
        let mut value = match c.next() {
            Some('0') => 0,
            Some('1') => 1000,
            _ => return None,
        };

        // Parse optional decimal point.
        match c.next() {
            Some('.') => (),
            None => return Some(Self(value)),
            _ => return None,
        };

        // Parse optional fractional digits. The value of each digit is multiplied by `factor`.
        // Since the q-value is represented as an integer between 0 and 1000, `factor` is `100` for
        // the first digit, `10` for the next, and `1` for the digit after that.
        let mut factor = 100;
        loop {
            match c.next() {
                Some(n @ '0'..='9') => {
                    // If `factor` is less than `1`, three digits have already been parsed. A
                    // q-value having more than 3 fractional digits is invalid.
                    if factor < 1 {
                        return None;
                    }
                    // Add the digit's value multiplied by `factor` to `value`.
                    value += factor * (n as u16 - '0' as u16);
                }
                None => {
                    // No more characters to parse. Check that the value representing the q-value is
                    // in the valid range.
                    return if value <= 1000 { Some(Self(value)) } else { None };
                }
                _ => return None,
            };
            factor /= 10;
        }
    }
}

#[cfg(test)]
mod test {
    use http::HeaderValue;

    use super::{AcceptEncoding, ContentEncoding, FilterEncoding, Header, QValue};

    #[test]
    fn test_accept_encoding() {
        fn v(v: &str) -> AcceptEncoding {
            AcceptEncoding::decode(&mut [HeaderValue::from_str(v).unwrap()].iter()).unwrap()
        }

        let filter = FilterEncoding::default();

        let mut values = vec![];
        let encodings = AcceptEncoding {
            gzip: QValue(1000),
            br: QValue(500),
            deflate: QValue(0),
            zstd: QValue(250),
        };

        encodings.encode(&mut values);

        let s = format!("{:?}", values);
        assert_eq!(s, r#"["gzip;q=1,br;q=0.500,zstd;q=0.250"]"#);
        assert_eq!(encodings.preferred_encoding(filter), ContentEncoding::Gzip);

        let encodings2 = AcceptEncoding::decode(&mut values.iter()).unwrap();
        assert_eq!(encodings, encodings2);

        assert_eq!(v("zstd;q=0.25,br;q=0.5,gzip;q=1"), encodings);

        assert_eq!(v("gzip;q=1.0").preferred_encoding(filter), ContentEncoding::Gzip);
        assert_eq!(
            v("gzip;q=0.5,br;q=1").preferred_encoding(filter),
            ContentEncoding::Brotli
        );
        assert_eq!(
            v("gzip;q=0.1,*;q=0.5").preferred_encoding(filter),
            ContentEncoding::Zstd
        );
        assert_eq!(v("*").preferred_encoding(filter), ContentEncoding::Zstd);
    }
}
