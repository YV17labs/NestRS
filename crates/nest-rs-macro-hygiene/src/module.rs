//! `#[module]` wiring every witness provider — the one DI module of the crate.
//!
//! Reached through the **prelude**, which is the other thing this file
//! witnesses. `#[module]` is the one decorator whose *file* the naming tables
//! fix (`architecture.md`: "One `#[module]` per file, one `module.rs` per
//! folder"), so it cannot be relocated into `prelude.rs` for a witness's
//! convenience — the witness comes here instead.

use nest_rs::prelude::*;

use crate::gateway::{HygieneGateway, HygieneWsGuard};
use crate::lifecycle::HygieneLifecycle;
use crate::listener::HygieneListener;
use crate::tasks::HygieneTasks;
use crate::tool::HygieneTool;

#[module(providers = [
    HygieneGateway,
    HygieneWsGuard,
    HygieneLifecycle,
    HygieneListener,
    HygieneTasks,
    HygieneTool,
])]
pub struct MacroHygieneModule;
