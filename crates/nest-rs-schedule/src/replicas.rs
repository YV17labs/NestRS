//! [`Replicas`] — how many replicas of an app fire one occurrence of a scheduled
//! job.

/// How many replicas of an app fire one occurrence of a scheduled job, declared
/// as `replicas = "each"` or `replicas = "one"` on `#[every]` and `#[cron]`.
///
/// A schedule runs on the process's own clock, so without a declaration every
/// replica fires every occurrence. That is right for a warm-up or a heartbeat
/// saying *this* process is alive, and wrong for work the deployment should do
/// once — enqueueing a job, sending a report. Only the developer knows which, so
/// it is declared per job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Replicas {
    /// Every replica fires every occurrence — the default. An `#[every]` job
    /// first fires one period after this process boots.
    #[default]
    Each,
    /// One replica fires each occurrence: the one whose claim on it succeeds,
    /// through the bound [`OccurrenceLock`](crate::OccurrenceLock). An `#[every]`
    /// job ticks on multiples of its period since the Unix epoch, so replicas
    /// started at different times share their instants.
    One,
}

impl Replicas {
    /// The value as the decorator spells it — the form the boot line and the
    /// tick's operation line carry.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Each => "each",
            Self::One => "one",
        }
    }
}
