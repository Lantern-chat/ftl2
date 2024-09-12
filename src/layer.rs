pub trait Layer<S> {
    type Service;

    fn layer(&self, inner: S) -> Self::Service;
}
