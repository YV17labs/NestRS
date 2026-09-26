//! The consumer binding, against a live worker: [`consumer`] for what the
//! transport owns — a method's concurrency, the exclusive fetch, each replica's
//! identity and the bounded drain — [`delivery`] for what one delivery does with
//! a job — the port's retries as filings on the schedule, a hand-back, a
//! connection that drops under it — and [`lease`] for the guard that keeps a job
//! from running twice when apalis delivers it twice.

mod consumer;
mod delivery;
mod lease;
