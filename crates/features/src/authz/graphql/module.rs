use nestrs_core::module;
use nestrs_seaorm::graphql::LoaderScope;
use nestrs_graphql::{BatchContext, OperationGuard};

use super::bridge::AppGraphqlGuard;
use super::guard::GraphqlAuthGuard;
use crate::Claims;
use crate::authz::http::AuthzHttpModule;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn OperationGuard,
        GraphqlAuthGuard,
        LoaderScope as dyn BatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

// Gated on `GraphqlAuthGuard` so the forwarder is silent in apps that do not
// import this module — keeps a second GraphQL app with a different principal
// type from double-forwarding.
nestrs_graphql::forward_principal!(Claims, GraphqlAuthGuard);
