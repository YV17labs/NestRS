//! The connection pools drivers opened — each one's connections by state, its
//! bounds and the callers it timed out — read when the metric reader exports,
//! as the database conventions' `db.client.connection.*` instruments name them
//! in v1.42, the schema this crate exports.

use nest_rs_core::connection_pool::{ConnectionPool, PoolConnections};
use opentelemetry::KeyValue;
use opentelemetry::metrics::Meter;
use opentelemetry_semantic_conventions::attribute::{
    DB_CLIENT_CONNECTION_POOL_NAME, DB_CLIENT_CONNECTION_STATE,
};
use opentelemetry_semantic_conventions::metric::{
    DB_CLIENT_CONNECTION_COUNT, DB_CLIENT_CONNECTION_IDLE_MIN, DB_CLIENT_CONNECTION_MAX,
    DB_CLIENT_CONNECTION_TIMEOUTS,
};

/// The unit the conventions give a count of connections.
const CONNECTIONS: &str = "{connection}";

/// Registers the pools' instruments.
pub(crate) struct ConnectionPoolMetrics;

/// One of a pool's bounds: its name, description and read.
type Bound = (&'static str, &'static str, fn(&PoolConnections) -> u64);

/// The bounds each pool reports, as the conventions describe them.
const BOUNDS: [Bound; 2] = [
    (
        DB_CLIENT_CONNECTION_MAX,
        "The maximum number of open connections allowed.",
        PoolConnections::max,
    ),
    (
        DB_CLIENT_CONNECTION_IDLE_MIN,
        "The minimum number of idle open connections allowed.",
        PoolConnections::idle_min,
    ),
];

impl ConnectionPoolMetrics {
    /// Register on `meter` the instruments of every pool observed when the
    /// reader exports.
    pub(crate) fn observe(meter: &Meter) {
        meter
            .i64_observable_up_down_counter(DB_CLIENT_CONNECTION_COUNT)
            .with_unit(CONNECTIONS)
            .with_description(
                "The number of connections that are currently in state described by the \
                 `state` attribute.",
            )
            .with_callback(|observer| {
                for (pool, connections) in open() {
                    for (state, count) in
                        [("idle", connections.idle()), ("used", connections.used())]
                    {
                        observer.observe(
                            signed(count),
                            &[
                                named(pool.as_ref()),
                                KeyValue::new(DB_CLIENT_CONNECTION_STATE, state),
                            ],
                        );
                    }
                }
            })
            .build();
        for (name, description, read) in BOUNDS {
            meter
                .i64_observable_up_down_counter(name)
                .with_unit(CONNECTIONS)
                .with_description(description)
                .with_callback(move |observer| {
                    for (pool, connections) in open() {
                        observer.observe(signed(read(&connections)), &[named(pool.as_ref())]);
                    }
                })
                .build();
        }
        meter
            .u64_observable_counter(DB_CLIENT_CONNECTION_TIMEOUTS)
            .with_unit("{timeout}")
            .with_description(
                "The number of connection timeouts that have occurred trying to obtain a \
                 connection from the pool.",
            )
            .with_callback(|observer| {
                for (pool, _) in open() {
                    observer.observe(pool.timeouts(), &[named(pool.as_ref())]);
                }
            })
            .build();
    }
}

/// Every pool observed and still open, with its connections now.
fn open() -> impl Iterator<Item = (std::sync::Arc<dyn ConnectionPool>, PoolConnections)> {
    nest_rs_core::__private::observed_pools()
        .into_iter()
        .filter_map(|pool| {
            let connections = pool.connections()?;
            Some((pool, connections))
        })
}

fn named(pool: &dyn ConnectionPool) -> KeyValue {
    KeyValue::new(DB_CLIENT_CONNECTION_POOL_NAME, pool.name().to_owned())
}

fn signed(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};

    use super::*;

    struct Probe;

    impl ConnectionPool for Probe {
        fn name(&self) -> &str {
            "db.internal:5432/publish"
        }

        fn connections(&self) -> Option<PoolConnections> {
            Some(PoolConnections::new(3, 2, 10, 1))
        }

        fn timeouts(&self) -> u64 {
            4
        }
    }

    /// Each point of `name` in `exported`: its attributes, sorted, and its
    /// value.
    fn points(exported: &[ResourceMetrics], name: &str) -> Vec<(Vec<(String, String)>, String)> {
        exported
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == name)
            .flat_map(|metric| {
                let attributes = |attributes: &mut dyn Iterator<Item = &KeyValue>| {
                    let mut attributes: Vec<_> = attributes
                        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                        .collect();
                    attributes.sort();
                    attributes
                };
                match metric.data() {
                    AggregatedMetrics::I64(MetricData::Sum(sum)) => sum
                        .data_points()
                        .map(|point| {
                            (
                                attributes(&mut point.attributes()),
                                point.value().to_string(),
                            )
                        })
                        .collect::<Vec<_>>(),
                    AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
                        .data_points()
                        .map(|point| {
                            (
                                attributes(&mut point.attributes()),
                                point.value().to_string(),
                            )
                        })
                        .collect(),
                    other => panic!("a pool's instrument is a sum: {other:?}"),
                }
            })
            .collect()
    }

    #[test]
    fn every_pool_still_held_is_exported_under_its_name_by_state_and_bound() {
        let pool = Arc::new(Probe);
        nest_rs_core::connection_pool::observe(&pool);
        let (provider, exporter) = crate::testing::metered();
        ConnectionPoolMetrics::observe(&provider.meter("pools"));

        provider.force_flush().expect("the reader collects");
        let exported = exporter.get_finished_metrics().expect("the export");

        let name = |rest: &[(&str, &str)]| {
            let mut attributes: Vec<(String, String)> =
                [(DB_CLIENT_CONNECTION_POOL_NAME, "db.internal:5432/publish")]
                    .iter()
                    .chain(rest)
                    .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                    .collect();
            attributes.sort();
            attributes
        };
        let mut count = points(&exported, DB_CLIENT_CONNECTION_COUNT);
        count.sort();
        assert_eq!(
            count,
            vec![
                (
                    name(&[(DB_CLIENT_CONNECTION_STATE, "idle")]),
                    "3".to_owned()
                ),
                (
                    name(&[(DB_CLIENT_CONNECTION_STATE, "used")]),
                    "2".to_owned()
                ),
            ],
        );
        assert_eq!(
            points(&exported, DB_CLIENT_CONNECTION_MAX),
            vec![(name(&[]), "10".to_owned())],
        );
        assert_eq!(
            points(&exported, DB_CLIENT_CONNECTION_IDLE_MIN),
            vec![(name(&[]), "1".to_owned())],
        );
        assert_eq!(
            points(&exported, DB_CLIENT_CONNECTION_TIMEOUTS),
            vec![(name(&[]), "4".to_owned())],
        );
        drop(pool);
    }
}
