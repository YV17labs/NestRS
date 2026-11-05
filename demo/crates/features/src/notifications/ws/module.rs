use nest_rs::core::module;

use super::gateway::NotificationsGateway;
use crate::authn::AuthnModule;

#[module(imports = [AuthnModule], providers = [NotificationsGateway])]
pub struct NotificationsWsModule;
