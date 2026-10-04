//! The consumer binding, against a live worker: [`consumer`] for what the
//! transport owns — a method's concurrency, the exclusive fetch, each replica's
//! identity and the bounded drain — [`config`] for its settings at their
//! bounds, [`delivery`] for what one delivery does with
//! a job — the port's retries as filings on the schedule, a hand-back, a
//! connection that drops under it — [`lease`] for the guard that keeps a second
//! delivery from running beside the first, meets a cancel, lets go of
//! a unique key and counts a throttle, and [`checkpoint`] for a job's progress
//! across its attempts.

mod checkpoint;
mod config;
mod consumer;
mod delivery;
mod lease;
