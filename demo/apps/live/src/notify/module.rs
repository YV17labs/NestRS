use features::authn::AuthnModule;
use nest_rs_core::module;

use crate::notify::gateway::NotifyGateway;

#[module(imports = [AuthnModule], providers = [NotifyGateway])]
pub struct NotifyModule;
