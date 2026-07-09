//! The `#[indicators]` decorator, re-exported by `nest-rs-health`.
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
///
/// # Expands to
///
/// The impl unchanged, plus one `HealthIndicator` submitted to the link-time
/// inventory per probe-tagged method, whose `run` resolves the provider and
/// invokes the method (adapting its return to `anyhow::Result<()>`). No
/// `Discoverable` — the host's own `#[injectable]` owns it.
///
/// ```ignore
/// impl AppHealth { /* unchanged */ }
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_health::HealthIndicator {
///         name: "db_ping",
///         kind: ::nest_rs_health::ProbeKind::Readiness, // Liveness / Startup
///         provider_type_id: || TypeId::of::<AppHealth>(),
///         run: |c| Box::pin(async move { /* resolve + call → Ok/Err */ }),
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn indicators(args: TokenStream, input: TokenStream) -> TokenStream {
    indicators::indicators(args, input)
}
