use nestrs_core::module;

use crate::notify::gateway::NotifyGateway;

#[module(providers = [NotifyGateway])]
pub struct NotifyModule;
