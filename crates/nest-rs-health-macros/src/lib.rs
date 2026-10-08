//! The `#[indicators]` decorator, re-exported by `nest-rs-health`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod indicators;

/// Walks the methods; for each one
/// tagged with `#[liveness]`, `#[readiness]`, or `#[startup]`, submits a
/// `HealthIndicator` to the link-time inventory the
/// [`HealthService`](../nest_rs_health/struct.HealthService.html) drains at
/// probe time. The struct itself must be a regular `#[injectable]`.
///
/// Per-method probe attributes (exactly one per method):
///
/// - `#[liveness]` — answer "is the process still alive?"; runs on `GET
///   /health/live`.
/// - `#[readiness]` — answer "should I send traffic to it?"; runs on `GET
///   /health/ready`.
/// - `#[startup]` — answer "has it finished booting?"; runs on `GET
///   /health/startup`.
///
/// Those paths sit under the app's `HttpConfig::global_prefix` like any other
/// controller's; a prefixed app logs one `warn` at boot naming the paths its
/// probes actually answer on.
///
/// Each tagged method takes `&self` and returns `()` — reported `up` once it
/// completes — or a `Result<(), E: Into<anyhow::Error>>`. An error reports
/// `down` with a **fixed, opaque** reason, never your error's text, which goes
/// to a `warn` on `nest_rs::health`: `/health/*` is routinely unauthenticated.
///
/// Every indicator on a probe runs **concurrently**, under a per-indicator
/// ceiling and a probe-wide deadline set through
/// [`HealthConfig`](../nest_rs_health/struct.HealthConfig.html).
#[proc_macro_attribute]
pub fn indicators(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(indicators::indicators(args, input).into()).into()
}
