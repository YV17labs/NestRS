//! The `#[cron_job]` decorator, re-exported by `nestrs-schedule`. The generated
//! code uses absolute paths (`::nestrs_schedule::*`, `::nestrs_core::*`,
//! `::std::*`), so this crate does not depend on them — they resolve at the call
//! site. Token-building helpers are shared with the other decorators via
//! `nestrs-codegen`. The implementation lives in `cron_job`; this is the
//! language-required proc-macro entry.

use proc_macro::TokenStream;

mod cron_job;

/// Mark a struct as a scheduled job. Implement
/// [`Scheduled`](../nestrs_schedule/trait.Scheduled.html) on it; construction
/// mirrors `#[injectable]` (fields tagged `#[inject]` resolve from the container,
/// others default), and the macro additionally emits `impl Discoverable`
/// attaching a `CronJobMeta`: the job name, its trigger, and a thunk that builds
/// the job from the container and calls `Scheduled::run`. The `Scheduler`
/// transport discovers those metas at boot and runs each one.
///
/// Exactly one trigger argument, mirroring `@nestjs/schedule`:
///
/// - `#[cron_job(every = "30s")]` — fixed interval (`@Interval`). Suffixes `ms` /
///   `s` / `m` / `h`. First run one interval after boot.
/// - `#[cron_job(cron = "0 */5 * * * *")]` — cron expression (`@Cron`), 5/6/7
///   fields. `#[cron_job(cron = CronExpression::EVERY_MINUTE)]` for a preset. Add
///   `tz = "Europe/Paris"` to evaluate it in that IANA timezone (default UTC).
/// - `#[cron_job(after = "10s")]` — run once, that long after boot (`@Timeout`).
///
/// A `cron`/`tz` string literal is validated at compile time; a preset path
/// (`CronExpression::X`) is validated when the `Scheduler` configures.
#[proc_macro_attribute]
pub fn cron_job(args: TokenStream, input: TokenStream) -> TokenStream {
    cron_job::cron_job(args, input)
}
