//! In-process integration suite root for `nest-rs-redis` — no Redis. Every
//! test lives in the module named for the `src/` concern it covers:
//! [`worker`] for the consumer side (`QueueWorker` configure fail-fast,
//! `JobContext` wrapping, and the wire-format envelope contract of the
//! `#[process]`-emitted handler the worker drains).

mod worker;
