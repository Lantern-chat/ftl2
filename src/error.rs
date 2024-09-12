use std::io::{self, ErrorKind};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Hyper error: {0}")]
    HyperError(#[from] hyper::Error),

    #[error("Stream Aborted")]
    StreamAborted,
}

pub type BoxError = Box<dyn core::error::Error + Send + Sync>;

pub(crate) fn io_other<E: Into<BoxError>>(error: E) -> io::Error {
    io::Error::new(ErrorKind::Other, error)
}
