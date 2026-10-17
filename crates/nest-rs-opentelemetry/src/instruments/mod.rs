//! The synchronous instrument kinds of the OpenTelemetry Metrics API, each a
//! cheap `Clone` handle [`Metrics`](crate::Metrics) makes once per name.

mod counter;
mod gauge;
mod histogram;
mod up_down_counter;

pub use counter::Counter;
pub use gauge::Gauge;
pub use histogram::Histogram;
pub use up_down_counter::UpDownCounter;
