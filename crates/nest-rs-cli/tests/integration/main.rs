//! `nestrs` CLI integration suite — drives the built binary against a scratch
//! workspace on disk. No live infrastructure, so it is the `integration` suite.
//!
//! The module tree mirrors `src/`: one file per command, `generate/` per
//! generator, and the shared fixtures in [`harness`].

mod cli;
mod doctor;
mod generate;
mod harness;
mod info;
mod new;
