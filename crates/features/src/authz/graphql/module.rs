use nestrs_core::module;
use nestrs_database::graphql::LoaderScope;
use nestrs_graphql::{BatchContext, OperationGuard};

use super::bridge::AppGraphqlGuard;
use super::guard::GraphqlAuthGuard;
use crate::authz::http::AuthzHttpModule;
use crate::Claims;

#[module(
    imports = [AuthzHttpModule],
    providers = [
        AppGraphqlGuard as dyn OperationGuard,
        GraphqlAuthGuard,
        LoaderScope as dyn BatchContext,
    ],
)]
pub struct AuthzGraphqlModule;

// Forward `Claims` from the poem request into the GraphQL context, so a
// resolver reads the actor via `ctx.data::<Claims>()` exactly as a
// controller reads it via `Ctx<Claims>`. Gated on `GraphqlAuthGuard` — the
// provider declared by this same module — so the forwarder is silent in
// every app that does not import `AuthzGraphqlModule`.
nestrs_graphql::forward_principal!(Claims, GraphqlAuthGuard);
