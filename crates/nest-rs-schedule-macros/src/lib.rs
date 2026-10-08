//! The `#[scheduled]` decorator, re-exported by `nest-rs-schedule`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod scheduled;

/// Orchestrator on a provider's `impl` block: each method tagged with a trigger
/// attribute is submitted as a `ScheduledMethod` to the link-time inventory the
/// [`Scheduler`](../nest_rs_schedule/struct.Scheduler.html) drains at boot. The
/// struct itself must be a regular `#[injectable]`.
///
/// Per-method trigger attributes (exactly one per method):
///
/// - `#[every("30s")]` — fixed interval (`ms`/`s`/`m`/`h`); first run one
///   interval after boot.
/// - `#[after("10s")]` — one-shot, fires once after boot.
/// - `#[cron("0 */5 * * * *")]` (5/6/7 fields) or
///   `#[cron(CronExpression::EVERY_MINUTE)]`. Add `tz = "Europe/Paris"` for
///   an IANA timezone (default UTC):
///   `#[cron("0 9 * * MON", tz = "Europe/Paris")]`.
///
/// A trigger's method borrows its host (`&self`, or `self: &Arc<Self>`) and
/// takes nothing else, declares no type or const parameter, and returns
/// `anyhow::Result<()>`, whose `Err` fails the occurrence. It may be `async` or
/// not; only an `async` one is awaited.
///
/// `#[every]` and `#[cron]` take `replicas = "one"` to fire each occurrence on
/// one replica of the app rather than on every one: the replica that claims it
/// through the occurrence lock the app imports. An `#[every]` declared so ticks
/// on multiples of its period since the Unix epoch, so replicas booted at
/// different times share their instants. `replicas = "each"` is the default,
/// and `#[after]` refuses the key — each replica's boot is its own event.
///
/// Every trigger takes `timeout = "30m"`: how long a run lasts before it is cut
/// and reported failed, so a call that never answers skips no later occurrence;
/// default ten minutes (`nest_rs_worker::JOB_TIMEOUT`), at most `"24h"`.
///
/// A job firing once is identified by its crate, its host struct and its
/// method, so moving its module inside the crate keeps it and renaming any of
/// the three starts a new job. Beside `replicas = "one"`, `key = "…"` pins the
/// identity across a rename — the path the boot line names, e.g.
/// `#[every("1h", replicas = "one", key = "billing::InvoiceTasks::close_day")]`.
///
/// A `cron` string literal and a `tz` name are validated at compile time; a
/// preset path is validated when `Scheduler` configures, naming the offending
/// job.
///
/// Several decorated methods on one impl block share the provider's
/// `#[inject]` dependencies.
///
/// The impl is re-emitted unchanged, its methods still callable, with no
/// `Discoverable` — the host's own `#[injectable]` owns it.
#[proc_macro_attribute]
pub fn scheduled(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(scheduled::scheduled(args, input).into()).into()
}
