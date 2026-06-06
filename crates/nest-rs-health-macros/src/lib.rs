//! The `#[indicators]` decorator, re-exported by `nestrs-health`.

use proc_macro::TokenStream;

mod indicators;

/// Orchestrator on a provider's `impl` block. Walks the methods; for each one
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
/// Each tagged method takes `&self` and returns `anyhow::Result<()>` (or any
/// `Result<(), E: Into<anyhow::Error>>`). `Ok(())` reports the indicator as
/// `up`; an error reports it as `down` and stringifies the error into the
/// probe's JSON body.
///
/// Multiple decorated methods on the same `#[indicators]` impl block all
/// share the provider's `#[inject]` dependencies — pool a DB ping, a Redis
/// ping, and a migration check on a single `AppHealth` service rather than
/// writing a struct per check.
#[proc_macro_attribute]
pub fn indicators(args: TokenStream, input: TokenStream) -> TokenStream {
    indicators::indicators(args, input)
}
