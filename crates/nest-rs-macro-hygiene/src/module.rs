//! `#[module]` wiring every witness provider — the one DI module of the crate.

use nest_rs_core::module;

use crate::gateway::{HygieneGateway, HygieneWsGuard};
use crate::lifecycle::HygieneLifecycle;
use crate::listener::HygieneListener;
use crate::tasks::HygieneTasks;

/// Root module for the witness providers.
#[module(providers = [
    HygieneGateway,
    HygieneWsGuard,
    HygieneLifecycle,
    HygieneListener,
    HygieneTasks,
])]
pub struct MacroHygieneModule;
