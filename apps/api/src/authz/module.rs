use domain::Claims;
use nestrs_authz_graphql::LoaderScope;
use nestrs_authz_ws::WsDataContext;
use nestrs_core::module;
use nestrs_graphql::{BatchContext, OperationGuard};
use nestrs_ws::SocketContext;

use crate::authz::bridge::ApiGraphqlGuard;

/// Wires product authz from [`domain::authz::AuthzModule`] onto this app's GraphQL and
/// WebSocket transports. HTTP controllers bind [`domain::authz::AppAbilityGuard`] directly.
#[module(
    imports = [domain::authz::AuthzModule],
    providers = [
        ApiGraphqlGuard as dyn OperationGuard,
        LoaderScope as dyn BatchContext,
        WsDataContext as dyn SocketContext,
    ],
)]
pub struct AuthzModule;

nestrs_graphql::forward_principal!(Claims);
