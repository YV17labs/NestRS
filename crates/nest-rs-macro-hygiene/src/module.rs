//! `#[module]` wiring every witness provider — the one DI module of the crate.

use nest_rs::core::module;

use crate::gateway::{HygieneGateway, HygieneWsGuard};
use crate::lifecycle::HygieneLifecycle;
use crate::listener::HygieneListener;
use crate::tasks::HygieneTasks;
use crate::tool::HygieneTool;

/// Root module for the witness providers.
#[module(providers = [
    HygieneGateway,
    HygieneWsGuard,
    HygieneLifecycle,
    HygieneListener,
    HygieneTasks,
    HygieneTool,
])]
pub struct MacroHygieneModule;
