//! The producer binding, against a live Redis: [`module`] for the two names
//! `RedisQueueModule` binds, [`producer`] for a push held back until its delay
//! ends, and [`promoter`] for the producer moving that job onto its queue when no
//! worker runs to do it.

mod module;
mod producer;
mod promoter;
