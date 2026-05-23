use nestrs_core::module;
use nestrs_graphql::GraphqlModule;
use nestrs_health::HealthModule;
use nestrs_server_timing::ServerTiming;
use nestrs_telemetry::OtelHttp;

use crate::auth::ApiKeyGuard;
use crate::users::UsersModule;

#[module(
    imports = [UsersModule, GraphqlModule, HealthModule],
    providers = [ServerTiming, OtelHttp, ApiKeyGuard],
)]
pub struct AppModule;
