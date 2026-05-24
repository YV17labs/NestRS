use nestrs_core::module;
use nestrs_graphql::GraphqlModule;
use nestrs_health::HealthModule;
use nestrs_openapi::OpenApiModule;
use nestrs_server_timing::ServerTiming;
use nestrs_telemetry::OtelHttp;

use crate::auth::AuthGuard;
use crate::authz::{AbilityGuard, AppAbility};
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, GraphqlModule, HealthModule, OpenApiModule],
    providers = [ServerTiming, OtelHttp, AuthGuard, AbilityGuard, AppAbility],
)]
pub struct AppModule;
