use std::borrow::Cow;

use crate::headers::Header;
use crate::{FromRequest, IntoResponse, Request, Response};

use futures::{future, ready, Future, Sink, Stream};
use std::pin::Pin;
use std::task::{Context, Poll};

use headers::{
    Connection, HeaderMapExt, SecWebsocketAccept, SecWebsocketKey, SecWebsocketVersion, Upgrade,
};
use http::{HeaderName, HeaderValue, Method, StatusCode, Version};
use hyper::upgrade::{OnUpgrade, Upgraded};
use hyper_util::rt::TokioIo;
use tokio_tungstenite::{
    tungstenite::{self, protocol},
    WebSocketStream,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsRejection {
    MethodNotGet,
    MethodNotConnect,
    MissingUpgrade,
    IncorrectUpgrade,
    IncorrectWebSocketVersion,
    InvalidProtocolPsuedoHeader,
    MissingWebSocketKey,
}

impl IntoResponse for WsRejection {
    fn into_response(self) -> Response {
        IntoResponse::into_response(match self {
            WsRejection::MethodNotGet => ("Method Not GET", StatusCode::METHOD_NOT_ALLOWED),
            WsRejection::MethodNotConnect => ("Method Not CONNECT", StatusCode::METHOD_NOT_ALLOWED),
            WsRejection::MissingUpgrade => ("Missing Upgrade header", StatusCode::BAD_REQUEST),
            WsRejection::IncorrectUpgrade => ("Incorrect Upgrade header", StatusCode::BAD_REQUEST),
            WsRejection::IncorrectWebSocketVersion => {
                ("Incorrect WebSocket version", StatusCode::BAD_REQUEST)
            }
            WsRejection::InvalidProtocolPsuedoHeader => {
                ("Invalid protocol psuedo-header", StatusCode::BAD_REQUEST)
            }
            WsRejection::MissingWebSocketKey => {
                ("Missing Sec-WebSocket-Key header", StatusCode::BAD_REQUEST)
            }
        })
    }
}

pub struct Ws {
    /// `None` if HTTP/2
    key: Option<SecWebsocketKey>,
    sec_websocket_protocol: Option<HeaderValue>,
    config: protocol::WebSocketConfig,
    on_upgrade: Option<OnUpgrade>,
}

impl<S> FromRequest<S> for Ws {
    type Rejection = WsRejection;

    fn from_request(
        mut req: Request,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let headers = req.headers();

            let key = if req.version() <= Version::HTTP_11 {
                if req.method() != Method::GET {
                    return Err(WsRejection::MethodNotGet);
                }

                match headers.typed_get::<Connection>() {
                    Some(header) if header.contains("upgrade") => {}
                    _ => return Err(WsRejection::MissingUpgrade),
                }

                match headers.typed_get::<Upgrade>() {
                    Some(upgrade) if upgrade == Upgrade::websocket() => {}
                    _ => return Err(WsRejection::IncorrectUpgrade),
                }

                match headers.typed_get() {
                    Some(key) => Some(key),
                    None => return Err(WsRejection::MissingWebSocketKey),
                }
            } else {
                if req.method() != Method::CONNECT {
                    return Err(WsRejection::MethodNotConnect);
                }

                if req
                    .extensions()
                    .get::<hyper::ext::Protocol>()
                    .map_or(true, |p| p.as_str() != "websocket")
                {
                    return Err(WsRejection::InvalidProtocolPsuedoHeader);
                }

                None
            };

            match headers.typed_get::<SecWebsocketVersion>() {
                Some(SecWebsocketVersion::V13) => {}
                _ => return Err(WsRejection::IncorrectWebSocketVersion),
            }

            let sec_websocket_protocol = req
                .headers()
                .get(hyper::header::SEC_WEBSOCKET_PROTOCOL)
                .cloned();

            let on_upgrade = req.extensions_mut().remove::<OnUpgrade>();

            Ok(Ws {
                key,
                sec_websocket_protocol,
                config: Default::default(),
                on_upgrade,
            })
        }
    }
}

impl Ws {
    /// See [WebSocketConfig]
    #[must_use]
    pub fn write_buffer_size(mut self, size: usize) -> Self {
        self.config.write_buffer_size = size;
        self
    }

    /// See [WebSocketConfig]
    #[must_use]
    pub fn max_write_buffer_size(mut self, max: usize) -> Self {
        self.config.max_write_buffer_size = max;
        self
    }

    /// Set the maximum message size (defaults to 64 megabytes)
    #[must_use]
    pub fn max_message_size(mut self, max: usize) -> Self {
        self.config.max_message_size = Some(max);
        self
    }

    /// Set the maximum frame size (defaults to 16 megabytes)
    #[must_use]
    pub fn max_frame_size(mut self, max: usize) -> Self {
        self.config.max_frame_size = Some(max);
        self
    }

    #[must_use]
    pub fn on_upgrade<F, Fut>(self, func: F) -> impl IntoResponse
    where
        F: FnOnce(Result<WebSocket, hyper::Error>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        WsResponse {
            ws: self,
            on_upgrade: func,
        }
    }
}

struct WsResponse<F> {
    ws: Ws,
    on_upgrade: F,
}

impl<F, Fut> IntoResponse for WsResponse<F>
where
    F: FnOnce(Result<WebSocket, hyper::Error>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    fn into_response(self) -> Response {
        let Some(on_upgrade) = self.ws.on_upgrade else {
            log::warn!("ws couldn't be upgraded since no upgrade state was present");

            return IntoResponse::into_response(StatusCode::BAD_REQUEST);
        };

        let on_upgrade_cb = self.on_upgrade;
        let config = self.ws.config;

        tokio::spawn(async move {
            let ws = match on_upgrade.await {
                Err(e) => {
                    log::error!("ws upgrade error: {e}");

                    Err(e)
                }
                Ok(upgraded) => {
                    log::trace!("websocket upgrade complete");

                    Ok(WebSocket {
                        inner: WebSocketStream::from_raw_socket(
                            TokioIo::new(upgraded),
                            protocol::Role::Server,
                            Some(config),
                        )
                        .await,
                    })
                }
            };

            on_upgrade_cb(ws).await;
        });

        match self.ws.key {
            // HTTP/1
            Some(key) => IntoResponse::into_response((
                StatusCode::SWITCHING_PROTOCOLS,
                Header(Connection::upgrade()),
                Header(Upgrade::websocket()),
                Header(SecWebsocketAccept::from(key)),
                self.ws
                    .sec_websocket_protocol
                    .map(|p| [(hyper::header::SEC_WEBSOCKET_PROTOCOL, p)]),
            )),
            // HTTP/2
            // As established in RFC 9113 section 8.5, we just respond
            // with a 2XX with an empty body:
            // <https://datatracker.ietf.org/doc/html/rfc9113#name-the-connect-method>.
            None => IntoResponse::into_response(StatusCode::OK),
        }
    }
}

pub struct WebSocket {
    inner: WebSocketStream<TokioIo<Upgraded>>,
}

/// A websocket `Stream` and `Sink`, provided to `ws` filters.
///
/// Ping messages sent from the client will be handled internally by replying with a Pong message.
/// Close messages need to be handled explicitly: usually by closing the `Sink` end of the
/// `WebSocket`.
impl WebSocket {
    /// Gracefully close this websocket.
    pub async fn close(mut self) -> Result<(), tungstenite::Error> {
        future::poll_fn(|cx| Pin::new(&mut self).poll_close(cx)).await
    }
}

impl Stream for WebSocket {
    type Item = Result<Message, tungstenite::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        match ready!(Pin::new(&mut self.inner).poll_next(cx)) {
            Some(Ok(item)) => Poll::Ready(Some(Ok(Message { inner: item }))),
            Some(Err(e)) => {
                tracing::debug!("websocket poll error: {}", e);
                Poll::Ready(Some(Err(e)))
            }
            None => {
                tracing::trace!("websocket closed");
                Poll::Ready(None)
            }
        }
    }
}

pub type SinkError = tungstenite::Error;
impl Sink<Message> for WebSocket {
    type Error = SinkError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match ready!(Pin::new(&mut self.inner).poll_ready(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        match Pin::new(&mut self.inner).start_send(item.inner) {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::debug!("websocket start_send error: {}", e);
                Err(e)
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        match ready!(Pin::new(&mut self.inner).poll_flush(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        match ready!(Pin::new(&mut self.inner).poll_close(cx)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(err) => {
                tracing::debug!("websocket close error: {}", err);
                Poll::Ready(Err(err))
            }
        }
    }
}

/// A WebSocket message.
///
/// This will likely become a `non-exhaustive` enum in the future, once that
/// language feature has stabilized.
#[derive(Eq, PartialEq, Clone)]
#[repr(transparent)]
pub struct Message {
    inner: protocol::Message,
}

impl Message {
    /// Construct a new Text `Message`.
    pub fn text<S: Into<String>>(s: S) -> Message {
        Message {
            inner: protocol::Message::text(s),
        }
    }

    /// Construct a new Binary `Message`.
    pub fn binary<V: Into<Vec<u8>>>(v: V) -> Message {
        Message {
            inner: protocol::Message::binary(v),
        }
    }

    /// Construct a new Ping `Message`.
    pub fn ping<V: Into<Vec<u8>>>(v: V) -> Message {
        Message {
            inner: protocol::Message::Ping(v.into()),
        }
    }

    /// Construct a new Pong `Message`.
    ///
    /// Note that one rarely needs to manually construct a Pong message because the underlying tungstenite socket
    /// automatically responds to the Ping messages it receives. Manual construction might still be useful in some cases
    /// like in tests or to send unidirectional heartbeats.
    pub fn pong<V: Into<Vec<u8>>>(v: V) -> Message {
        Message {
            inner: protocol::Message::Pong(v.into()),
        }
    }

    /// Construct the default Close `Message`.
    pub fn close() -> Message {
        Message {
            inner: protocol::Message::Close(None),
        }
    }

    /// Construct a Close `Message` with a code and reason.
    pub fn close_with(code: impl Into<u16>, reason: impl Into<Cow<'static, str>>) -> Message {
        Message {
            inner: protocol::Message::Close(Some(protocol::frame::CloseFrame {
                code: protocol::frame::coding::CloseCode::from(code.into()),
                reason: reason.into(),
            })),
        }
    }

    /// Returns true if this message is a Text message.
    pub fn is_text(&self) -> bool {
        self.inner.is_text()
    }

    /// Returns true if this message is a Binary message.
    pub fn is_binary(&self) -> bool {
        self.inner.is_binary()
    }

    /// Returns true if this message a is a Close message.
    pub fn is_close(&self) -> bool {
        self.inner.is_close()
    }

    /// Returns true if this message is a Ping message.
    pub fn is_ping(&self) -> bool {
        self.inner.is_ping()
    }

    /// Returns true if this message is a Pong message.
    pub fn is_pong(&self) -> bool {
        self.inner.is_pong()
    }

    /// Try to get the close frame (close code and reason)
    pub fn close_frame(&self) -> Option<(u16, &str)> {
        if let protocol::Message::Close(Some(ref close_frame)) = self.inner {
            Some((close_frame.code.into(), close_frame.reason.as_ref()))
        } else {
            None
        }
    }

    /// Try to get a reference to the string text, if this is a Text message.
    pub fn to_str(&self) -> Option<&str> {
        match self.inner {
            protocol::Message::Text(ref s) => Some(s),
            _ => None,
        }
    }

    /// Return the bytes of this message, if the message can contain data.
    pub fn as_bytes(&self) -> &[u8] {
        match self.inner {
            protocol::Message::Text(ref s) => s.as_bytes(),
            protocol::Message::Binary(ref v) => v,
            protocol::Message::Ping(ref v) => v,
            protocol::Message::Pong(ref v) => v,
            protocol::Message::Close(_) => &[],
            protocol::Message::Frame(ref f) => f.payload(),
        }
    }

    /// Destructure this message into binary data.
    pub fn into_bytes(self) -> Vec<u8> {
        self.inner.into_data()
    }
}

use std::fmt;
impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

impl From<Message> for Vec<u8> {
    fn from(m: Message) -> Self {
        m.into_bytes()
    }
}
