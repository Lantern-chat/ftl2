use std::future::Future;

use http_body_util::BodyExt as _;

use crate::{body::Form, FromRequest, Request};

impl<S, T> FromRequest<S> for Form<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // collect body in non-contiguous memory and then parse it
            let body = req.body_mut().take().collect().await?;

            Ok(Form({
                use bytes::Buf;

                // both serde-urlencoded and serde_html_form can read from a
                // non-contiguous buffer using the Reader object
                crate::form_impl::from_reader(body.aggregate().reader())?
            }))
        }
    }
}
