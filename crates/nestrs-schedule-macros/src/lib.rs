//! The `#[scheduled]` decorator, re-exported by `nestrs-schedule`.

use proc_macro::TokenStream;

mod scheduled;

/// Orchestrator on a provider's `impl` block. Walks the methods; for each one
/// tagged with a trigger attribute, submits a `ScheduledMethod` to the
/// link-time inventory the [`Scheduler`](../nestrs_schedule/struct.Scheduler.html)
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
/// A `cron` string literal is validated at compile time; a preset path and
/// any timezone are validated when `Scheduler` configures, naming the
/// offending job.
///
/// Multiple decorated methods on the same `#[scheduled]` impl block all
/// share the provider's `#[inject]` dependencies — the NestJS-equivalent
/// pattern of pooling related cron methods on one service.
#[proc_macro_attribute]
pub fn scheduled(args: TokenStream, input: TokenStream) -> TokenStream {
    scheduled::scheduled(args, input)
}
