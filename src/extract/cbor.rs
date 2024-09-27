use std::future::Future;

use http_body_util::BodyExt as _;

use crate::{FromRequest, Request};

pub use crate::body::Cbor;

impl<S, T> FromRequest<S> for Cbor<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // collect body in non-contiguous memory and then parse it
            let body = req.body_mut().take().collect().await?;

            let value = {
                use bytes::Buf;

                // ciborium can read from a non-contiguous buffer using the Reader object
                ciborium::de::from_reader(body.aggregate().reader())?
            };

            Ok(Cbor(value))
        }
    }
}
