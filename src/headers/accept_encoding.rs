use http::HeaderValue;

#[derive(Debug, Clone, Copy)]
pub struct AcceptEncoding {
    pub(crate) gzip: bool,
    pub(crate) br: bool,
    pub(crate) deflate: bool,
    pub(crate) zstd: bool,
}

impl Default for AcceptEncoding {
    fn default() -> Self {
        AcceptEncoding {
            gzip: true,
            deflate: true,
            br: true,
            zstd: true,
        }
    }
}

// This enum's variants are ordered from least to most preferred.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, PartialEq, Eq)]
pub(crate) enum Encoding {
    #[allow(dead_code)]
    Identity,
    #[cfg(feature = "compression-deflate")]
    Deflate,
    #[cfg(feature = "compression-gzip")]
    Gzip,
    #[cfg(feature = "compression-br")]
    Brotli,
    #[cfg(feature = "compression-zstd")]
    Zstd,
}

impl AcceptEncoding {
    #[allow(dead_code)]
    pub(crate) const fn to_header_value(self) -> Option<HeaderValue> {
        Some(match (self.gzip(), self.deflate(), self.br(), self.zstd()) {
            (true, true, true, false) => const { HeaderValue::from_static("gzip,deflate,br") },
            (true, true, false, false) => const { HeaderValue::from_static("gzip,deflate") },
            (true, false, true, false) => const { HeaderValue::from_static("gzip,br") },
            (true, false, false, false) => const { HeaderValue::from_static("gzip") },
            (false, true, true, false) => const { HeaderValue::from_static("deflate,br") },
            (false, true, false, false) => const { HeaderValue::from_static("deflate") },
            (false, false, true, false) => const { HeaderValue::from_static("br") },
            (true, true, true, true) => const { HeaderValue::from_static("zstd,gzip,deflate,br") },
            (true, true, false, true) => const { HeaderValue::from_static("zstd,gzip,deflate") },
            (true, false, true, true) => const { HeaderValue::from_static("zstd,gzip,br") },
            (true, false, false, true) => const { HeaderValue::from_static("zstd,gzip") },
            (false, true, true, true) => const { HeaderValue::from_static("zstd,deflate,br") },
            (false, true, false, true) => const { HeaderValue::from_static("zstd,deflate") },
            (false, false, true, true) => const { HeaderValue::from_static("zstd,br") },
            (false, false, false, true) => const { HeaderValue::from_static("zstd") },
            (false, false, false, false) => return None,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn set_gzip(&mut self, enable: bool) {
        self.gzip = enable;
    }

    #[allow(dead_code)]
    pub(crate) fn set_deflate(&mut self, enable: bool) {
        self.deflate = enable;
    }

    #[allow(dead_code)]
    pub(crate) fn set_br(&mut self, enable: bool) {
        self.br = enable;
    }

    #[allow(dead_code)]
    pub(crate) fn set_zstd(&mut self, enable: bool) {
        self.zstd = enable;
    }

    #[allow(dead_code)]
    const fn gzip(&self) -> bool {
        #[cfg(feature = "compression-gzip")]
        return self.gzip;
        #[cfg(not(feature = "compression-gzip"))]
        return false;
    }

    #[allow(dead_code)]
    const fn deflate(&self) -> bool {
        #[cfg(feature = "compression-deflate")]
        return self.deflate;
        #[cfg(not(feature = "compression-deflate"))]
        return false;
    }

    #[allow(dead_code)]
    const fn br(&self) -> bool {
        #[cfg(feature = "compression-br")]
        return self.br;
        #[cfg(not(feature = "compression-br"))]
        return false;
    }

    #[allow(dead_code)]
    const fn zstd(&self) -> bool {
        #[cfg(feature = "compression-zstd")]
        return self.zstd;
        #[cfg(not(feature = "compression-zstd"))]
        return false;
    }
}

impl Encoding {
    const fn to_str(self) -> &'static str {
        match self {
            #[cfg(feature = "compression-gzip")]
            Encoding::Gzip => "gzip",
            #[cfg(feature = "compression-deflate")]
            Encoding::Deflate => "deflate",
            #[cfg(feature = "compression-br")]
            Encoding::Brotli => "br",
            #[cfg(feature = "compression-zstd")]
            Encoding::Zstd => "zstd",
            Encoding::Identity => "identity",
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn into_header_value(self) -> http::HeaderValue {
        http::HeaderValue::from_static(self.to_str())
    }
}

#[cfg(any(
    feature = "compression-gzip",
    feature = "compression-br",
    feature = "compression-deflate",
    feature = "compression-zstd",
))]
impl Encoding {
    fn parse(s: &str, _supported_encoding: AcceptEncoding) -> Option<Encoding> {
        #[cfg(feature = "compression-gzip")]
        if (s.eq_ignore_ascii_case("gzip") || s.eq_ignore_ascii_case("x-gzip")) && _supported_encoding.gzip() {
            return Some(Encoding::Gzip);
        }

        #[cfg(feature = "compression-deflate")]
        if s.eq_ignore_ascii_case("deflate") && _supported_encoding.deflate() {
            return Some(Encoding::Deflate);
        }

        #[cfg(feature = "compression-br")]
        if s.eq_ignore_ascii_case("br") && _supported_encoding.br() {
            return Some(Encoding::Brotli);
        }

        #[cfg(feature = "compression-zstd")]
        if s.eq_ignore_ascii_case("zstd") && _supported_encoding.zstd() {
            return Some(Encoding::Zstd);
        }

        if s.eq_ignore_ascii_case("identity") {
            return Some(Encoding::Identity);
        }

        None
    }

    // based on https://github.com/http-rs/accept-encoding
    pub(crate) fn from_headers(headers: &http::HeaderMap, supported_encoding: AcceptEncoding) -> Self {
        Encoding::preferred_encoding(Self::encodings(headers, supported_encoding)).unwrap_or(Encoding::Identity)
    }

    pub(crate) fn preferred_encoding(
        accepted_encodings: impl Iterator<Item = (Encoding, qvalue::QValue)>,
    ) -> Option<Self> {
        accepted_encodings
            .filter(|(_, qvalue)| qvalue.0 > 0)
            .max_by_key(|&(encoding, qvalue)| (qvalue, encoding))
            .map(|(encoding, _)| encoding)
    }

    pub(crate) fn encodings(
        headers: &http::HeaderMap,
        supported_encoding: AcceptEncoding,
    ) -> impl Iterator<Item = (Encoding, qvalue::QValue)> + '_ {
        headers
            .get_all(http::header::ACCEPT_ENCODING)
            .iter()
            .filter_map(|hval| hval.to_str().ok())
            .flat_map(|s| s.split(','))
            .filter_map(move |v| {
                let mut v = v.splitn(2, ';');

                let encoding = match Encoding::parse(v.next().unwrap().trim(), supported_encoding) {
                    Some(encoding) => encoding,
                    None => return None, // ignore unknown encodings
                };

                let qval = if let Some(qval) = v.next() {
                    qvalue::QValue::parse(qval.trim())?
                } else {
                    qvalue::QValue::one()
                };

                Some((encoding, qval))
            })
    }
}

#[cfg(any(
    feature = "compression-gzip",
    feature = "compression-br",
    feature = "compression-zstd",
    feature = "compression-deflate",
))]
pub(crate) mod qvalue {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct QValue(pub u16);

    impl QValue {
        #[inline]
        pub const fn one() -> Self {
            Self(1000)
        }

        // Parse a q-value as specified in RFC 7231 section 5.3.1.
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
}
