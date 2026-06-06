use nest_rs_core::module;

use crate::notify::gateway::NotifyGateway;

#[module(providers = [NotifyGateway])]
pub struct NotifyModule;
