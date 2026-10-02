//! The `#[scheduled]` decorator, re-exported by `nest-rs-schedule`.
#![warn(missing_docs)]

use proc_macro::TokenStream;

mod scheduled;

/// Orchestrator on a provider's `impl` block. Walks the methods; for each one
/// tagged with a trigger attribute, submits a `ScheduledMethod` to the
/// link-time inventory the [`Scheduler`](../nest_rs_schedule/struct.Scheduler.html)
/// drains at boot. The struct itself must be a regular `#[injectable]`.
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
/// Multiple decorated methods on the same `#[scheduled]` impl block all
/// share the provider's `#[inject]` dependencies — pooling related cron
/// methods on a single service keeps shared state (clients, caches) in
/// one place.
///
/// # Expands to
///
/// The impl unchanged (methods stay callable), plus one `ScheduledMethod`
/// submitted to the link-time inventory per trigger-tagged method, whose `run`
/// resolves the provider from the container and invokes the method. No
/// `Discoverable` — the host's own `#[injectable]` owns it.
///
/// ```ignore
/// impl ReportTasks { /* unchanged */ }
/// ::nest_rs_core::inventory::submit! {
///     ::nest_rs_schedule::ScheduledMethod {
///         provider: "ReportTasks", method: "nightly",
///         provider_type_id: || TypeId::of::<ReportTasks>(),
///         trigger: ::nest_rs_schedule::Trigger::Cron { expr, tz }, // or Interval / Timeout
///         transaction: ::nest_rs_schedule::nest_rs_worker::JobTransaction::PerAttempt,
///         replicas: ::nest_rs_schedule::Replicas::Each,
///         key: ::std::option::Option::None, // or Some("billing::InvoiceTasks::close_day")
///         origin: ::core::module_path!(),
///         run: |c| Box::pin(async move { /* resolve + call */ }),
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn scheduled(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(scheduled::scheduled(args, input).into()).into()
}
