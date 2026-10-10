//! The steps `#[nest_rs::main]` runs before the runtime exists, while the
//! process has one thread: what is unsound once another thread may read
//! alongside it — publishing the `.env` cascade into the process environment.

/// A step `#[nest_rs::main]` runs before the runtime exists, sorted by `name`.
pub struct ProcessStart {
    /// Orders the steps; named like a span target (`nest_rs::config::cascade`).
    pub name: &'static str,
    /// The step.
    pub run: fn(),
}

inventory::collect!(ProcessStart);

/// Run every step this binary links, by name.
pub(crate) fn run_all() {
    let mut steps: Vec<&ProcessStart> = inventory::iter::<ProcessStart>().collect();
    steps.sort_by_key(|step| step.name);
    for step in steps {
        (step.run)();
    }
}
