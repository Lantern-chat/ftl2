use std::{
    borrow::Cow,
    fmt::Write,
    time::{Duration, Instant},
};

use smallvec::SmallVec;

/**
 * ```
 * // Single metric without value
 * Server-Timing: missedCache
 *
 * // Single metric with value
 * Server-Timing: cpu;dur=2.4
 *
 * // Single metric with description and value
 * Server-Timing: cache;desc="Cache Read";dur=23.2
 *
 * // Two metrics with value
 * Server-Timing: db;dur=53, app;dur=47.2
 *
 * // Server-Timing as trailer
 * Trailer: Server-Timing
 * --- response body ---
 * Server-Timing: total;dur=123.4
 * ```
 */

/// A single metric in the [ServerTimings] header.
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
#[must_use]
pub struct ServerTiming {
    pub name: Cow<'static, str>,
    pub description: Option<Cow<'static, str>>,
    pub duration: Option<Duration>,
}

impl ServerTiming {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            description: None,
            duration: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<Cow<'static, str>>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    pub fn elapsed_from(mut self, start: Instant) -> Self {
        self.duration = Some(start.elapsed());
        self
    }
}

/// [Server-Timing] header, containing one or more metrics and descriptions.
///
/// The `Server-Timing` header communicates one or more metrics and descriptions for
/// the given request-response cycle. It is used to surface any backend server timing
/// metrics (e.g. database read/write, CPU time, file system access, etc.)
///
/// This implementation assumes that the time units are in decimal milliseconds.
///
/// NOTE: If the `desc` field contains a comma or semicolon, it will likely be parsed
/// incorrectly. This is a limitation of the current implementation.
///
/// [Server-Timing]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Server-Timing
#[must_use]
#[derive(Default)]
pub struct ServerTimings {
    timings: SmallVec<[ServerTiming; 1]>,
}

impl ServerTimings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            timings: SmallVec::with_capacity(capacity),
        }
    }

    pub fn iter(&self) -> core::slice::Iter<ServerTiming> {
        self.timings.iter()
    }

    pub fn push(&mut self, timing: ServerTiming) -> &mut Self {
        self.timings.push(timing);

        self
    }

    pub fn with(mut self, timing: ServerTiming) -> Self {
        self.timings.push(timing);

        self
    }
}

use headers::{Header, HeaderName, HeaderValue};

impl Header for ServerTimings {
    fn name() -> &'static HeaderName {
        static NAME: HeaderName = const { HeaderName::from_static("server-timing") };

        &NAME
    }

    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        if self.timings.is_empty() {
            return;
        }

        let mut value = String::with_capacity(self.timings.len() * 12); // end;dur=12.3 estimated

        for (i, timing) in self.timings.iter().enumerate() {
            if i > 0 {
                #[allow(clippy::single_char_add_str)] // it's faster...
                value.push_str(",");
            }

            value.push_str(&timing.name);

            if let Some(description) = &timing.description {
                write!(value, ";desc=\"{}\"", description).unwrap();
            }

            if let Some(duration) = timing.duration {
                let duration = duration.as_micros();
                let (milliseconds, microseconds) = (duration / 1_000, duration % 1_000);

                write!(value, ";dur={milliseconds}.{microseconds:03}").unwrap();
            }
        }

        if let Ok(value) = HeaderValue::try_from(value) {
            values.extend(Some(value));
        }
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        Self: Sized,
        I: Iterator<Item = &'i HeaderValue>,
    {
        let mut timings = SmallVec::new();

        for value in values.filter_map(|hval| hval.to_str().ok()).flat_map(|s| s.split(',')) {
            let mut timing = ServerTiming::default();

            for entry in value.split(';') {
                let entry = entry.trim();

                if entry.is_empty() {
                    continue; // invalid but not harmful?
                }

                let Some((key, value)) = entry.split_once('=') else {
                    timing.name = Cow::Owned(entry.to_owned());
                    continue;
                };

                match key {
                    "desc" => timing.description = Some(Cow::Owned(value.trim_matches('"').to_owned())),
                    "dur" => {
                        timing.duration = match parse_duration(value) {
                            Some(duration) => Some(duration),
                            None => return Err(headers::Error::invalid()),
                        }
                    }

                    _ => continue, // invalid key
                }
            }

            timings.push(timing);
        }

        Ok(Self { timings })
    }
}

fn parse_duration(value: &str) -> Option<Duration> {
    Some(match value.split_once('.') {
        None => Duration::from_millis(value.parse().ok()?),
        Some((millis, micros)) => {
            let max_len = micros.len().min(3);

            let milliseconds = millis.parse::<u64>().ok()?;
            let microseconds = micros[..max_len].parse::<u64>().ok()?;

            Duration::from_micros(milliseconds * 1_000 + microseconds)
        }
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_server_timing() {
        let mut timings = ServerTimings::new();

        timings
            .push(ServerTiming::new("cpu").with_duration(Duration::from_millis(2400)))
            .push(
                ServerTiming::new("cache")
                    .with_description("Cache Read")
                    .with_duration(Duration::from_millis(23200)),
            )
            .push(ServerTiming::new("db").with_duration(Duration::from_millis(53000)))
            .push(ServerTiming::new("app").with_duration(Duration::from_millis(47005)));

        let mut values = Vec::new();
        timings.encode(&mut values);

        let timings = ServerTimings::decode(&mut values.iter()).unwrap();

        let mut iter = timings.iter();

        assert_eq!(
            iter.next(),
            Some(&ServerTiming {
                name: Cow::Borrowed("cpu"),
                description: None,
                duration: Some(Duration::from_millis(2400))
            })
        );

        assert_eq!(
            iter.next(),
            Some(&ServerTiming {
                name: Cow::Borrowed("cache"),
                description: Some(Cow::Borrowed("Cache Read")),
                duration: Some(Duration::from_millis(23200))
            })
        );

        assert_eq!(
            iter.next(),
            Some(&ServerTiming {
                name: Cow::Borrowed("db"),
                description: None,
                duration: Some(Duration::from_millis(53000))
            })
        );

        assert_eq!(
            iter.next(),
            Some(&ServerTiming {
                name: Cow::Borrowed("app"),
                description: None,
                duration: Some(Duration::from_millis(47005))
            })
        );

        assert_eq!(iter.next(), None);
    }
}
