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
//! One binary reads the same sources without asserting anything: `canon`
//! prints the framework facts the docs linter checks pages against
//! (`docs/scripts/lint-docs.mjs` runs it on start).
//!
//! The crate exists because a join's population is the **whole workspace**, and
//! no capability crate owns that. `nest-rs-macro-hygiene` has its own mandate
//! and is not it.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a join fails the suite by panicking, as the test it runs in would"
)]

pub mod baseline;
pub mod imports;
pub mod sources;
