//! [`Replicas`] — how many replicas of an app fire one occurrence of a scheduled
//! job.

/// How many replicas of an app fire one occurrence of a scheduled job, declared
/// as `replicas = "each"` or `replicas = "one"` on `#[every]` and `#[cron]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Replicas {
    /// Every replica fires every occurrence — the default. An `#[every]` job
    /// first fires one period after this process boots.
    #[default]
    Each,
    /// Each occurrence fires on the one replica whose claim on it succeeds,
    /// through the bound [`OccurrenceLock`](crate::OccurrenceLock) — at most once
    /// across the replicas. An `#[every]` job ticks on multiples of its period
    /// since the Unix epoch, so replicas started at different times share their
    /// instants.
    ///
    /// Once per **occurrence**, not one run at a time: a run outlasting its period
    /// holds nothing, so the next occurrence fires on another replica while it goes
    /// on, and the two runs overlap. A replica never overlaps its own runs.
    One,
}

impl Replicas {
    /// The value as the decorator spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Each => "each",
            Self::One => "one",
        }
    }
}
