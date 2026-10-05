//! The queue binding against a live Redis: [`module`] for the names
//! `RedisQueueModule` binds, [`producer`] for a push held back until its delay
//! ends, a push under a unique key and a cancel, and [`consumer`] for the
//! behaviour kit every queue backend runs and what only Redis decides beside
//! it — the fence, the records a job leaves, the dead letters.

mod consumer;
mod module;
mod producer;
