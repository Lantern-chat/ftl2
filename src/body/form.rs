use http::StatusCode;

use crate::{form_impl, IntoResponse};

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Form<T = ()>(pub T);

impl Form {
    pub fn try_new<T: serde::Serialize>(value: T) -> Result<crate::Response, form_impl::ser::Error> {
        match form_impl::to_string(&value) {
            Ok(v) => Ok(crate::body::Body::from(v)
                .with_header(headers::ContentType::form_url_encoded())
                .into_response()),
            Err(e) => Err(e),
        }
    }
}

impl<T> IntoResponse for Form<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> crate::Response {
        match Form::try_new(self.0) {
            Ok(response) => response,
            Err(e) => {
                log::error!("Form Response error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }
}
