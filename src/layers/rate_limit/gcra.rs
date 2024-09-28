//! Lower-level rate limiter implementation using the Generic Cell Rate Algorithm (GCRA) and
//! an asynchronous hash map for concurrent access.

use std::{
    borrow::Borrow,
    error::Error,
    fmt,
    hash::{BuildHasher, Hash},
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use scc::hash_map::{Entry, HashMap};

/// A rate limiter that uses the Generic Cell Rate Algorithm (GCRA) to limit the rate of requests.
///
/// This rate limiter is designed to be used in a concurrent environment, and is thread-safe.
pub struct RateLimiter<K: Eq + Hash, H: BuildHasher = std::collections::hash_map::RandomState> {
    start: Instant,
    gc_interval: u64,
    last_gc: AtomicU64,
    limits: HashMap<K, Gcra, H>,
}

impl<K: Eq + Hash, H: BuildHasher> RateLimiter<K, H> {
    /// Constructs a new rate limiter with the given GCRA hasher and garbage collection interval, which is in number of requests
    /// (i.e. how many requests to process before cleaning up old entries), not time.
    pub fn new(gc_interval: u64, hasher: H) -> Self {
        RateLimiter {
            start: Instant::now(),
            gc_interval,
            last_gc: AtomicU64::new(1),
            limits: HashMap::with_hasher(hasher),
        }
    }

    fn should_gc(&self) -> bool {
        self.gc_interval != u64::MAX && 0 == self.last_gc.fetch_add(1, Ordering::Relaxed) % self.gc_interval
    }

    #[inline]
    fn relative(&self, ts: Instant) -> u64 {
        ts.saturating_duration_since(self.start).as_nanos() as u64
    }

    /// Cleans up any entries that have not been accessed since the given time.
    pub async fn clean(&self, before: Instant) {
        let before = self.relative(before);
        self.limits.retain_async(move |_, v| *AtomicU64::get_mut(&mut v.0) >= before).await;
        self.last_gc.store(1, Ordering::Relaxed); // manual reset
    }

    /// Synchronous version of [`RateLimiter::clean`].
    pub fn clean_sync(&self, before: Instant) {
        let before = self.relative(before);
        self.limits.retain(move |_, v| *AtomicU64::get_mut(&mut v.0) >= before);
        self.last_gc.store(1, Ordering::Relaxed); // manual reset
    }

    /// Perform a request, returning an error if the request is too soon.
    pub async fn req(&self, key: K, quota: Quota, now: Instant) -> Result<(), RateLimitError> {
        let now = self.relative(now);

        let Some(res) = self.limits.read_async(&key, |_, gcra| gcra.req(quota, now)).await else {
            if self.should_gc() {
                self.limits.retain_async(move |_, v| *AtomicU64::get_mut(&mut v.0) >= now).await;
            }

            return match self.limits.entry_async(key).await {
                Entry::Occupied(gcra) => gcra.get().req(quota, now),
                Entry::Vacant(gcra) => {
                    gcra.insert_entry(Gcra::first(quota, now));
                    Ok(())
                }
            };
        };

        res
    }

    /// Synchonous version of [`RateLimiter::req`].
    pub fn req_sync(&self, key: K, quota: Quota, now: Instant) -> Result<(), RateLimitError> {
        let now = self.relative(now);

        let Some(res) = self.limits.read(&key, |_, gcra| gcra.req(quota, now)) else {
            if self.should_gc() {
                self.limits.retain(move |_, v| *AtomicU64::get_mut(&mut v.0) >= now);
            }

            return match self.limits.entry(key) {
                Entry::Occupied(gcra) => gcra.get().req(quota, now),
                Entry::Vacant(gcra) => {
                    gcra.insert_entry(Gcra::first(quota, now));
                    Ok(())
                }
            };
        };

        res
    }

    /// Variant of [`RateLimiter::req`] that allows for a peek at the key after it's been inserted.
    pub(crate) async fn req_peek_key<F>(
        &self,
        key: K,
        quota: Quota,
        now: Instant,
        peek: F,
    ) -> Result<(), RateLimitError>
    where
        F: FnOnce(&K),
    {
        let now = self.relative(now);
        let mut peek = Some(peek);

        let read = self
            .limits
            .read_async(&key, |_, gcra| {
                gcra.req(quota, now)?;
                let peek = unsafe { peek.take().unwrap_unchecked() }; // SAFETY: peek is Some
                peek(&key);
                Ok(())
            })
            .await;

        // if read returns Some, then peek was consumed
        let Some(res) = read else {
            // otherwise we're free to unwrap it and use it here normally
            let peek = unsafe { peek.unwrap_unchecked() };

            // since we hit the slow path, perform garbage collection
            if self.should_gc() {
                self.limits.retain_async(move |_, v| *AtomicU64::get_mut(&mut v.0) >= now).await;
            }

            let entry = match self.limits.entry_async(key).await {
                Entry::Occupied(gcra) => {
                    gcra.get().req(quota, now)?;
                    gcra
                }
                Entry::Vacant(gcra) => gcra.insert_entry(Gcra::first(quota, now)),
            };

            // NOTE: By using the returned entry from either branch, we potentially avoid duplicate codegen for peek
            peek(entry.key());

            return Ok(());
        };

        res
    }

    /// Penalizes the given key by the given amount of time,
    /// returning `true` if the key was found.
    ///
    /// NOTE: This is a relative penalty, so if used on an old entry it may be inneffective.
    /// Best to use after [`RateLimiter::req`] has been called receently.
    ///
    /// NOTE: Furthermore, it uses a 64-bit integer to store the GCRA value,
    /// so it may overflow if used with too large a value. Keep it modest.
    pub async fn penalize<Q>(&self, key: &Q, penalty: Duration) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.limits
            .read_async(key, |_, grca| {
                grca.0.fetch_add(penalty.as_nanos() as u64, Ordering::Relaxed)
            })
            .await
            .is_some()
    }

    /// Synchronous version of [`RateLimiter::penalize`].
    pub fn penalize_sync<Q>(&self, key: &Q, penalty: Duration) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.limits
            .read(key, |_, grca| {
                grca.0.fetch_add(penalty.as_nanos() as u64, Ordering::Relaxed)
            })
            .is_some()
    }

    /// Resets the rate limit for the given key, returning `true` if the key was found.
    pub async fn reset<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.limits.remove_async(key).await.is_some()
    }

    /// Synchronous version of [`RateLimiter::reset`].
    pub fn reset_sync<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.limits.remove(key).is_some()
    }
}

impl<K: Eq + Hash, H: BuildHasher> Default for RateLimiter<K, H>
where
    H: Default,
{
    fn default() -> Self {
        // default to 8192 unique requests before garbage collection
        RateLimiter::new(8192, H::default())
    }
}

/// An error that occurs when a rate limit is exceeded,
/// with the amount of time until the next request can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RateLimitError(pub NonZeroU64);

impl fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rate limit exceeded, retry in {:.3} seconds",
            self.as_duration().as_secs_f32()
        )
    }
}

impl Error for RateLimitError {}

use crate::response::{IntoResponse, Response};
use http::StatusCode;

impl IntoResponse for RateLimitError {
    fn into_response(self) -> Response {
        use http::{HeaderName, HeaderValue};

        // reuse as_duration value
        let reset = self.as_duration();

        let mut res = Response::new(From::from(format!(
            "rate limit exceeded, retry in {:.3} seconds",
            reset.as_secs_f32()
        )));

        *res.status_mut() = StatusCode::TOO_MANY_REQUESTS;

        let reset = reset.as_secs().max(1);

        // optimize for common values
        let value = match reset {
            1 => const { HeaderValue::from_static("1") },
            2 => const { HeaderValue::from_static("2") },
            3 => const { HeaderValue::from_static("3") },
            _ => {
                let mut buffer = itoa::Buffer::new();
                HeaderValue::from_str(buffer.format(reset)).unwrap()
            }
        };

        let headers = res.headers_mut();

        headers.insert(const { HeaderName::from_static("ratelimit-reset") }, value.clone());
        headers.insert(const { HeaderName::from_static("x-ratelimit-reset") }, value.clone());
        headers.insert(const { HeaderName::from_static("retry-after") }, value.clone());
        headers.insert(
            const { HeaderName::from_static("ratelimit-remaining") },
            const { HeaderValue::from_static("0") },
        );

        res
    }
}

impl RateLimitError {
    /// Returns the amount of time until the next request can be made as a `Duration`.
    #[inline]
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        Duration::from_nanos(self.0.get())
    }
}

/// A rate limit quota, which defines the number of requests that can be made
/// within a given time frame and with a given burst size.
#[derive(Debug, Clone, Copy)]
pub struct Quota {
    /// Burst size/cells, in nanoseconds `(t * burst)`
    tau: u64,

    /// Cell emission interval, in nanoseconds
    t: u64,
}

impl Default for Quota {
    /// Returns a default quota of 1000 requests per second.
    fn default() -> Quota {
        Quota::simple(Duration::from_millis(1))
    }
}

impl From<Duration> for Quota {
    /// Constructs a new quota with the given emission interval, but with a burst size of 1.
    fn from(emission_interval: Duration) -> Quota {
        Quota::simple(emission_interval)
    }
}

impl Quota {
    /// Constructs a new quota with the given number of burst requests and
    /// an `emission_interval` parameter, which is the amount of time it takes
    /// for the limit to release.
    ///
    /// For example, 100reqs/second would have an emission_interval of 10ms
    ///
    /// Burst requests ignore the individual emission interval in favor of
    /// delivering all at once or in quick succession, up until the provided limit.
    #[rustfmt::skip] #[must_use]
    pub const fn new(emission_interval: Duration, burst: NonZeroU64) -> Quota {
        let t = emission_interval.as_nanos() as u64;
        Quota { t, tau: t * burst.get() }
    }

    /// Constructs a new quota with the given emission interval, but with a burst size of 1.
    ///
    /// See [`Quota::new`] for more information.
    #[must_use]
    pub const fn simple(emission_interval: Duration) -> Quota {
        Self::new(emission_interval, NonZeroU64::MIN)
    }
}

/// Generic Cell Rate Algorithm (GCRA) implementation.
///
/// Uses a single atomic value to store the next time a request can be made.
#[derive(Debug)]
#[repr(transparent)]
pub struct Gcra(AtomicU64);

impl Gcra {
    /// Constructs a new GCRA for the first request at the given time.
    ///
    /// This is equivalent to `Gcra(now + t).req()`, but more efficient.
    #[inline]
    #[must_use]
    pub const fn first(Quota { t, .. }: Quota, now: u64) -> Gcra {
        // Equivalent to `Gcra(now + t).req()` to calculate the first request
        Gcra(AtomicU64::new(now + t + t))
    }

    /// Core GCRA logic. Returns the next time a request can be made, either as an error or a success.
    fn decide(prev: u64, now: u64, Quota { tau, t }: Quota) -> Result<u64, RateLimitError> {
        // burst's act as an offset to allow more through at the start
        let next = prev.saturating_sub(tau);

        if now < next {
            // SAFETY: next > now, so next - now is non-zero by definition
            Err(RateLimitError(unsafe { NonZeroU64::new_unchecked(next - now) }))
        } else {
            Ok(now.max(prev) + t)
        }
    }

    /// Perform a request, returning an error if the request is too soon.
    pub fn req(&self, quota: Quota, now: u64) -> Result<(), RateLimitError> {
        let mut prev = self.0.load(Ordering::Acquire);

        loop {
            let next = Self::decide(prev, now, quota)?;

            match self.0.compare_exchange_weak(prev, next, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => return Ok(()),
                Err(next_prev) => prev = next_prev,
            }
        }
    }
}
