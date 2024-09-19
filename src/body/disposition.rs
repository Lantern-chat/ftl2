use http::HeaderValue;
use std::borrow::Cow;

use crate::{IntoResponse, Response};

/// Wraps a response and sets the `Content-Disposition` header.
///
/// Filenames are url-encoded automatically.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
pub struct Disposition<R> {
    resp: R,
    attachment: bool,
    filename: Option<Cow<'static, str>>,
}

impl<R> Disposition<R> {
    pub fn inline(resp: R) -> Self {
        Self {
            resp,
            attachment: false,
            filename: None,
        }
    }

    pub fn attachment(resp: R) -> Self {
        Self {
            resp,
            attachment: true,
            filename: None,
        }
    }

    pub fn filename(mut self, filename: impl Into<Cow<'static, str>>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    pub fn as_inline(mut self) -> Self {
        self.attachment = false;
        self
    }

    pub fn as_attachment(mut self) -> Self {
        self.attachment = true;
        self
    }
}

impl<R> IntoResponse for Disposition<R>
where
    R: IntoResponse,
{
    fn into_response(self) -> Response {
        let mut resp = self.resp.into_response();

        if let Some(filename) = self.filename {
            let filename = urlencoding::encode(&filename);

            resp.headers_mut().insert(
                http::header::CONTENT_DISPOSITION,
                HeaderValue::try_from(format!(
                    "{}; filename={}\"{filename}\"",
                    if self.attachment { "attachment" } else { "inline" },
                    if matches!(filename, std::borrow::Cow::Owned(_)) { "*" } else { "" },
                ))
                .expect("valid header value for Content-Disposition"),
            );
        } else {
            resp.headers_mut().insert(
                http::header::CONTENT_DISPOSITION,
                match self.attachment {
                    true => const { HeaderValue::from_static("attachment") },
                    false => const { HeaderValue::from_static("inline") },
                },
            );
        }

        resp
    }
}
