use std::marker::PhantomData;

use futures::stream::StreamExt;

use crate::{IntoResponse, Response};

/// A type with an associated static value, which can be used to create a deferred response
/// without needing to allocate a new value each time.
///
/// Example:
/// ```rust,ignore
/// #[derive(serde::Serialize)]
/// struct MyValue {
///    field: u32,
/// }
///
/// impl StaticValue for MyValue {
///     fn value() -> &'static Self {
///         &MyValue { field: 42 }
///     }
/// }
///
/// // doesn't allocate
/// Deferred::new_static::<MyValue>()
/// ```
pub trait StaticValue: serde::Serialize + 'static {
    fn value() -> &'static Self;
}

pub(crate) enum DeferredInner {
    Single(Box<dyn IndirectSerialize>),
    Array(Box<dyn IndirectStream>),
}

use crate::layers::deferred::Encoding;

impl DeferredInner {
    pub fn into_response(self, encoding: Encoding) -> Response {
        match self {
            DeferredInner::Array(mut stream) => match encoding {
                #[cfg(feature = "json")]
                Encoding::Json => stream.as_json(),

                #[cfg(feature = "cbor")]
                Encoding::Cbor => stream.as_cbor(),
            },
            DeferredInner::Single(value) => match encoding {
                #[cfg(feature = "json")]
                Encoding::Json => value.as_json(),

                #[cfg(feature = "cbor")]
                Encoding::Cbor => value.as_cbor(),
            },
        }
    }
}

/// Defers the encoding of a value, using an encoding parameter given in the request.
///
/// Must be used in conjunction with the [`DeferredEncoding`] layer.
///
/// [`DeferredEncoding`]: crate::layers::deferred::DeferredEncoding
#[repr(transparent)]
pub struct Deferred(pub(crate) DeferredInner);

impl Deferred {
    /// Create a new deferred value.
    #[inline]
    pub fn new<T>(value: T) -> Self
    where
        T: serde::Serialize + Send + 'static,
    {
        Self(DeferredInner::Single(Box::new(value)))
    }

    /// Crate a new deferred value from a static value, notably without allocating.
    ///
    /// See [`StaticValue`] for more information.
    #[must_use]
    pub fn new_static<T: StaticValue>() -> Self {
        // ZST to avoid allocating when boxing the deferred response
        struct ValueSerializer<T: 'static>(PhantomData<&'static T>);

        // a value is not actually sent across threads, so this is safe
        unsafe impl<T: 'static> Send for ValueSerializer<T> {}

        impl<T: StaticValue> serde::Serialize for ValueSerializer<T> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                T::value().serialize(serializer)
            }
        }

        Deferred::new(ValueSerializer::<T>(PhantomData))
    }

    /// Create a new deferred value from a stream of values, to be serialized as an array or sequence.
    ///
    /// See [`Json::stream_array`] and [`Cbor::stream_array`] for more information.
    ///
    /// [`Json::stream_array`]: super::Json::stream_array
    /// [`Cbor::stream_array`]: super::Cbor::stream_array
    #[inline]
    pub fn stream<T, E>(stream: impl futures::Stream<Item = Result<T, E>> + Send + 'static) -> Self
    where
        T: serde::Serialize + Send + Sync + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        Self(DeferredInner::Array(Box::new(Some(stream))))
    }

    /// Simplified version of [`Deferred::stream`] for when the stream does not return errors.
    #[inline]
    pub fn simple_stream<T>(stream: impl futures::Stream<Item = T> + Send + 'static) -> Self
    where
        T: serde::Serialize + Send + Sync + 'static,
    {
        Self::stream(stream.map(Result::<_, std::convert::Infallible>::Ok))
    }
}

impl IntoResponse for Deferred {
    fn into_response(self) -> Response {
        Response::new(super::Body(super::BodyInner::Deferred(self)))
    }
}

pub(crate) trait IndirectSerialize: Send + 'static {
    #[cfg(feature = "json")]
    fn as_json(&self) -> Response;

    #[cfg(feature = "cbor")]
    fn as_cbor(&self) -> Response;
}

pub(crate) trait IndirectStream: Send + 'static {
    #[cfg(feature = "json")]
    fn as_json(&mut self) -> Response;

    #[cfg(feature = "cbor")]
    fn as_cbor(&mut self) -> Response;
}

const _: Option<&dyn IndirectSerialize> = None;
const _: Option<&dyn IndirectStream> = None;

impl<T> IndirectSerialize for T
where
    T: serde::Serialize + Send + 'static,
{
    #[cfg(feature = "json")]
    fn as_json(&self) -> Response {
        super::Json(self).into_response()
    }

    #[cfg(feature = "cbor")]
    fn as_cbor(&self) -> Response {
        super::Cbor(self).into_response()
    }
}

impl<S, T, E> IndirectStream for Option<S>
where
    S: futures::Stream<Item = Result<T, E>> + Send + 'static,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    #[cfg(feature = "json")]
    fn as_json(&mut self) -> Response {
        super::Json::stream_array(unsafe { self.take().unwrap_unchecked() }).into_response()
    }

    #[cfg(feature = "cbor")]
    fn as_cbor(&mut self) -> Response {
        super::Cbor::stream_array(unsafe { self.take().unwrap_unchecked() }).into_response()
    }
}
