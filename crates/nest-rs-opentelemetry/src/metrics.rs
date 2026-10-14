//! [`Metrics`] — where an app records its measurements, and [`Describe`], what
//! an instrument declares when it is made.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use opentelemetry::metrics::Meter;

use crate::{Counter, Gauge, Histogram, UpDownCounter};

/// Records measurements on the app's meter; provided by
/// [`OpenTelemetryModule`](crate::OpenTelemetryModule) under `otlp`.
///
/// Each method hands back the instrument of that name, made on the first call
/// and the same one on every call after it: a lookup, never a rebuild, so
/// asking at the call site costs one read lock. What an instrument declares —
/// its unit, its description, a histogram's bucket boundaries — is declared by
/// the `_with` method that first makes it.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_core::injectable;
/// use nest_rs_opentelemetry::{Attribute, Metrics};
///
/// #[injectable]
/// pub struct SignupMetrics {
///     #[inject]
///     metrics: Arc<Metrics>,
/// }
///
/// impl SignupMetrics {
///     pub fn record_signup(&self, source: &'static str) {
///         self.metrics
///             .counter("users.created")
///             .add(1, &[Attribute::new("source", source)]);
///     }
/// }
/// ```
pub struct Metrics {
    meter: Meter,
    counters: Made<opentelemetry::metrics::Counter<u64>>,
    up_down_counters: Made<opentelemetry::metrics::UpDownCounter<i64>>,
    histograms: Made<opentelemetry::metrics::Histogram<f64>>,
    gauges: Made<opentelemetry::metrics::Gauge<f64>>,
}

/// What an instrument declares when it is made: the unit its values are in and
/// what it measures.
///
/// ```
/// use nest_rs_opentelemetry::Describe;
///
/// let latency = Describe::default()
///     .with_unit("ms")
///     .with_description("time to answer a checkout");
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Describe {
    /// The unit the values are in, as UCUM spells it (`ms`, `By`, `{request}`).
    pub unit: Option<&'static str>,
    /// What the instrument measures.
    pub description: Option<&'static str>,
}

impl Describe {
    /// The same, in `unit`.
    pub const fn with_unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
    }

    /// The same, measuring what `description` says.
    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }
}

/// The instruments of one kind made so far, by name.
struct Made<I>(RwLock<HashMap<&'static str, I>>);

impl<I: Clone> Made<I> {
    fn new() -> Self {
        Self(RwLock::new(HashMap::new()))
    }

    /// The instrument named `name`, made by `make` if it is the first asked for.
    fn get_or_make(&self, name: &'static str, make: impl FnOnce() -> I) -> I {
        if let Some(made) = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
        {
            return made.clone();
        }
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(name)
            .or_insert_with(make)
            .clone()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.read().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl Metrics {
    pub(crate) fn new(meter: Meter) -> Self {
        Self {
            meter,
            counters: Made::new(),
            up_down_counters: Made::new(),
            histograms: Made::new(),
            gauges: Made::new(),
        }
    }

    /// The counter named `name`: a monotonic sum.
    pub fn counter(&self, name: &'static str) -> Counter {
        self.counter_with(name, Describe::default())
    }

    /// The counter named `name`, declaring `describe` if this call makes it.
    pub fn counter_with(&self, name: &'static str, describe: Describe) -> Counter {
        Counter::new(self.counters.get_or_make(name, || {
            let mut builder = self.meter.u64_counter(name);
            builder.unit = describe.unit.map(Cow::Borrowed);
            builder.description = describe.description.map(Cow::Borrowed);
            builder.build()
        }))
    }

    /// The up-down counter named `name`: a sum that goes both ways.
    pub fn up_down_counter(&self, name: &'static str) -> UpDownCounter {
        self.up_down_counter_with(name, Describe::default())
    }

    /// The up-down counter named `name`, declaring `describe` if this call
    /// makes it.
    pub fn up_down_counter_with(&self, name: &'static str, describe: Describe) -> UpDownCounter {
        UpDownCounter::new(self.up_down_counters.get_or_make(name, || {
            let mut builder = self.meter.i64_up_down_counter(name);
            builder.unit = describe.unit.map(Cow::Borrowed);
            builder.description = describe.description.map(Cow::Borrowed);
            builder.build()
        }))
    }

    /// The histogram named `name`: a distribution, in the SDK's default
    /// buckets.
    pub fn histogram(&self, name: &'static str) -> Histogram {
        self.make_histogram(name, Describe::default(), None)
    }

    /// The histogram named `name`, declaring `describe` and the bucket
    /// `boundaries` — the one aggregation option the specification lets an
    /// instrument advise — if this call makes it.
    pub fn histogram_with(
        &self,
        name: &'static str,
        describe: Describe,
        boundaries: &'static [f64],
    ) -> Histogram {
        self.make_histogram(name, describe, Some(boundaries))
    }

    fn make_histogram(
        &self,
        name: &'static str,
        describe: Describe,
        boundaries: Option<&'static [f64]>,
    ) -> Histogram {
        Histogram::new(self.histograms.get_or_make(name, || {
            let mut builder = self.meter.f64_histogram(name);
            builder.unit = describe.unit.map(Cow::Borrowed);
            builder.description = describe.description.map(Cow::Borrowed);
            builder.boundaries = boundaries.map(<[f64]>::to_vec);
            builder.build()
        }))
    }

    /// The gauge named `name`: the last value measured.
    pub fn gauge(&self, name: &'static str) -> Gauge {
        self.gauge_with(name, Describe::default())
    }

    /// The gauge named `name`, declaring `describe` if this call makes it.
    pub fn gauge_with(&self, name: &'static str, describe: Describe) -> Gauge {
        Gauge::new(self.gauges.get_or_make(name, || {
            let mut builder = self.meter.f64_gauge(name);
            builder.unit = describe.unit.map(Cow::Borrowed);
            builder.description = describe.description.map(Cow::Borrowed);
            builder.build()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> Metrics {
        Metrics::new(opentelemetry::global::meter("nestrs"))
    }

    #[test]
    fn a_name_asked_for_twice_is_one_instrument() {
        let metrics = metrics();
        metrics.counter("x");
        metrics.counter_with("x", Describe::default().with_unit("ms"));
        metrics.histogram("x");
        metrics.histogram("x");
        assert_eq!(metrics.counters.len(), 1);
        assert_eq!(metrics.histograms.len(), 1);
        metrics.counter("y");
        assert_eq!(metrics.counters.len(), 2);
    }
}
