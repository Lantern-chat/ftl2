use std::future::Future;

use http::StatusCode;
use http_body_util::BodyExt as _;

use crate::{body::BodyError, IntoResponse, Request, Response};
use crate::{body::Form, form_impl};

use super::{body::body_error_to_response, FromRequest};

#[derive(Debug, thiserror::Error)]
pub enum FormRejectionError {
    #[error(transparent)]
    Form(#[from] form_impl::de::Error),

    #[error("An error occurred while reading the body: {0}")]
    BodyError(#[from] BodyError),
}

impl IntoResponse for FormRejectionError {
    fn into_response(self) -> Response {
        match self {
            // assume badly formatted form is a bad request
            FormRejectionError::Form(_) => StatusCode::BAD_REQUEST.into_response(),
            FormRejectionError::BodyError(err) => body_error_to_response(err),
        }
    }
}

impl<S, T> FromRequest<S> for Form<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = FormRejectionError;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // collect body in non-contiguous memory and then parse it
            let body = req.body_mut().take().collect().await.map_err(FormRejectionError::BodyError)?;

            Ok(Form({
                use bytes::Buf;

                // both serde-urlencoded and serde_html_form can read from a
                // non-contiguous buffer using the Reader object
                form_impl::from_reader(body.aggregate().reader())?
            }))
        }
    }
}
