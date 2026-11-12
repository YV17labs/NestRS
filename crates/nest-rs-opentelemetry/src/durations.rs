//! The duration histograms the framework records itself — a unit's, a client
//! operation's — each made once, under the framework's instrumentation scope,
//! into the bucket boundaries its conventions advise.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use opentelemetry::InstrumentationScope;
use opentelemetry::metrics::{Histogram, Meter, MeterProvider as _};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_semantic_conventions::metric::DB_CLIENT_OPERATION_DURATION;

/// The instrumentation scope the framework's own instruments are recorded
/// under: the crate an app installs.
pub(crate) const SCOPE: &str = "nest-rs";

/// The unit a duration is in, as the conventions write it.
pub(crate) const SECONDS: &str = "s";

/// The bucket boundaries, in seconds, the HTTP and messaging conventions
/// advise for a duration; the MCP conventions advise none.
pub(crate) const REQUEST_BOUNDARIES: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// The bucket boundaries, in seconds, the database conventions advise.
pub(crate) const DATABASE_BOUNDARIES: [f64; 9] =
    [0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0];

/// The framework's duration histograms, by name.
pub(crate) struct Durations {
    meter: Meter,
    histograms: RwLock<HashMap<&'static str, Histogram<f64>>>,
}

impl Durations {
    /// Record on `provider`, under the framework's scope.
    pub(crate) fn new(provider: &SdkMeterProvider) -> Self {
        let scope = InstrumentationScope::builder(SCOPE)
            .with_version(env!("CARGO_PKG_VERSION"))
            .build();
        Self {
            meter: provider.meter_with_scope(scope),
            histograms: RwLock::new(HashMap::new()),
        }
    }

    /// The histogram `name`, described by `description`, made the first time
    /// a duration records in it.
    pub(crate) fn histogram(
        &self,
        name: &'static str,
        description: &'static str,
    ) -> Histogram<f64> {
        if let Some(made) = self
            .histograms
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
        {
            return made.clone();
        }
        self.histograms
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(name)
            .or_insert_with(|| {
                self.meter
                    .f64_histogram(name)
                    .with_unit(SECONDS)
                    .with_description(description)
                    .with_boundaries(boundaries(name).to_vec())
                    .build()
            })
            .clone()
    }
}

/// The boundaries the conventions naming `name` advise.
fn boundaries(name: &str) -> &'static [f64] {
    if name == DB_CLIENT_OPERATION_DURATION {
        &DATABASE_BOUNDARIES
    } else {
        &REQUEST_BOUNDARIES
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry_semantic_conventions::metric::HTTP_SERVER_REQUEST_DURATION;

    use super::*;

    #[test]
    fn a_statement_reads_the_database_conventions_buckets_and_a_request_the_http_ones() {
        assert_eq!(
            boundaries(DB_CLIENT_OPERATION_DURATION),
            DATABASE_BOUNDARIES
        );
        assert_eq!(boundaries(HTTP_SERVER_REQUEST_DURATION), REQUEST_BOUNDARIES);
    }
}
