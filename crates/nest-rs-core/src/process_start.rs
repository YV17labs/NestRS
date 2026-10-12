//! The steps `#[nest_rs::main]` runs before the runtime exists, while the
//! process has one thread: what is unsound once another thread may read
//! alongside it — publishing the `.env` cascade into the process environment.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::stderr_lines::StderrLines;

/// A step `#[nest_rs::main]` runs before the runtime exists, sorted by `name`.
pub struct ProcessStart {
    /// Orders the steps; named like a span target (`nest_rs::config::cascade`).
    pub name: &'static str,
    /// The step.
    pub run: fn(),
}

inventory::collect!(ProcessStart);

/// Whether the steps ran under [`StderrLines`], the first default this
/// process set.
static STEPS_SAID_ON_STDERR: AtomicBool = AtomicBool::new(false);

/// Run every step this binary links, by name. No subscriber exists yet, so
/// what a step files at `warn` or `error` is written to stderr.
pub(crate) fn run_all() {
    let mut steps: Vec<&ProcessStart> = inventory::iter::<ProcessStart>().collect();
    steps.sort_by_key(|step| step.name);
    let run = || {
        for step in steps {
            (step.run)();
        }
    };
    if tracing::dispatcher::has_been_set() {
        run();
    } else {
        STEPS_SAID_ON_STDERR.store(true, Ordering::Relaxed);
        tracing::subscriber::with_default(StderrLines, run);
    }
}

/// Whether a subscriber is set, so the kernel's fallback steps aside.
///
/// tracing's `has_been_set` stays true once any default was set, a scoped one
/// long gone included: when the steps' own was the first, the default this
/// thread now reaches is read instead.
#[cfg(feature = "logging")]
pub(crate) fn subscriber_set() -> bool {
    if STEPS_SAID_ON_STDERR.load(Ordering::Relaxed) {
        crate::panic::subscribed()
    } else {
        tracing::dispatcher::has_been_set()
    }
}
