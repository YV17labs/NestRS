use identity::Claims;
use nestrs_authz_graphql::LoaderScope;
use nestrs_authz_ws::WsDataContext;
use nestrs_core::module;
use nestrs_graphql::{BatchContext, OperationGuard};
use nestrs_ws::SocketContext;

use crate::authn::AuthnModule;
use crate::authz::ability::AppAbility;
use crate::authz::guard::{AppAbilityGuard, GraphqlAuthGuard};

#[module(
    imports = [AuthnModule],
    providers = [
        AppAbility,
        AppAbilityGuard,
        GraphqlAuthGuard as dyn OperationGuard,
        LoaderScope as dyn BatchContext,
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AuthzModule;

nestrs_graphql::forward_principal!(Claims);
