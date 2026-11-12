//! `#[module]`, reached through the prelude: the naming rules fix its file, so
//! its prelude witness lives here rather than in `prelude.rs`.

use nest_rs::prelude::*;

use crate::lifecycle::HygieneLifecycle;

#[module(providers = [HygieneLifecycle])]
pub struct MacroHygieneModule;
