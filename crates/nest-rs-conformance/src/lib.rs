//! Conformance joins — the executed half of *What is missing is a cell, not a
//! feeling* (`.claude/rules/testing.md`).
//!
//! A family the framework declares (its decorator pairs, its `for_root` seams,
//! its `warn`+ events) has members **derived from the source**, never listed by
//! hand. The suite here puts such a family beside the tests covering it and
//! fails on a member nobody names.
//!
//! `src/` carries only what every join needs — where to read, and how a
//! baseline behaves. A join itself is always a test: it asserts, so it lives in
//! `tests/`, and this crate exports no way to run one outside of that.
//!
//! The crate exists because a join's population is the **whole workspace**, and
//! no capability crate owns that. `nest-rs-macro-hygiene` has its own mandate
//! and is not it.

pub mod baseline;
pub mod sources;
