//! `#[module]` — the one DI module of the crate, wiring the kernel's witness
//! provider.
//!
//! Reached through the **prelude**, which is the other thing this file
//! witnesses. `#[module]` is the one decorator whose *file* the naming tables
//! fix (`architecture.md`: "One `#[module]` per file, one `module.rs` per
//! folder"), so it cannot be relocated into `prelude.rs` for a witness's
//! convenience — the witness comes here instead. Its expansion names each
//! provider by type and nothing else, so one provider witnesses it as well as
//! every one would, under no feature.

use nest_rs::prelude::*;

use crate::lifecycle::HygieneLifecycle;

#[module(providers = [HygieneLifecycle])]
pub struct MacroHygieneModule;
