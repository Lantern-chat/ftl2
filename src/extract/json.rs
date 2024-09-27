use http_body_util::BodyExt as _;
use std::future::Future;

use crate::{FromRequest, Request};

pub use crate::body::Json;

impl<S, T> FromRequest<S> for Json<T>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    type Rejection = crate::Error;

    fn from_request(mut req: Request, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            // collect body in non-contiguous memory and then parse it
            let body = req.body_mut().take().collect().await?;

            #[cfg(not(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64"))))]
            let value = {
                use bytes::Buf;
                // serde-json can read from a non-contiguous buffer using the Reader object
                json_impl::from_reader(body.aggregate().reader())?
            };

            #[cfg(all(feature = "json-simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
            let value = {
                // unknown if sonic-rs can read from a non-contiguous buffer?
                json_impl::from_slice(&body.to_bytes())?
            };

            Ok(Json(value))
        }
    }
}
