//! The producer binding, against a live Redis: [`module`] for the two names
//! `RedisQueueModule` binds, [`producer`] for a push held back until its delay
//! ends, a push under a unique key and a cancel, and [`promoter`] for the
//! producer moving a delayed job onto its queue when no worker runs to do it.

mod composition;
mod module;
mod producer;
mod promoter;
