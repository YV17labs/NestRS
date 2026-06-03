//! The `#[cron_job]` decorator, re-exported by `nestrs-schedule`.

use proc_macro::TokenStream;

mod cron_job;

/// Mark a struct as a scheduled job. Implement
/// [`Scheduled`](../nestrs_schedule/trait.Scheduled.html); `#[inject]` fields
/// resolve from the container, others default.
///
/// Exactly one trigger argument:
/// - `every = "30s"` — fixed interval (`ms`/`s`/`m`/`h`); first run one
///   interval after boot.
/// - `cron = "0 */5 * * * *"` (5/6/7 fields) or
///   `cron = CronExpression::EVERY_MINUTE` (preset). Add `tz = "Europe/Paris"`
///   for an IANA timezone (default UTC).
/// - `after = "10s"` — one-shot.
///
/// A `cron`/`tz` string literal is validated at compile time; a preset path
/// is validated when `Scheduler` configures.
#[proc_macro_attribute]
pub fn cron_job(args: TokenStream, input: TokenStream) -> TokenStream {
    cron_job::cron_job(args, input)
}
