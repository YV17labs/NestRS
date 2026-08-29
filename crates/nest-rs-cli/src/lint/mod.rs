//! The one naming rule of `architecture.md` that a path cannot derive.
//!
//! Every other rule in that file is *derivable*: a `module.rs` under
//! `redis/queue/` is a `RedisQueueModule` whatever it declares, so the suite
//! reads the path and compares. The pairing between a vocabulary file's stem
//! and the types it declares is not — nothing about `principal.rs` predicts
//! `Principal`, and only reading the file says whether the two meet.
//!
//! So it is checked instead of derived, and refused in exactly one shape: a
//! stem that reaches **nothing** the file declares. That file was named for a
//! *slot* — "who acts", "what we pass around" — rather than for a subject, and
//! a slot has no admission test, so the next type about that slot lands there
//! too. It is a `shared/` folder at the scale of a file, and it is invisible
//! from outside: both names read perfectly well alone, and only the pair is
//! wrong.

mod finding;
mod scan;

pub use finding::Finding;
pub use scan::{Scan, scan};
