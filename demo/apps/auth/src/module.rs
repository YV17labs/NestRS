use nest_rs::core::module;
use nest_rs::health::HealthModule;
use nest_rs::seaorm::SeaOrmDatabaseModule;
use nest_rs::social::{GithubSocialModule, GoogleSocialModule};

use features::sign_in::SignInHttpModule;

#[module(
    imports = [
        SeaOrmDatabaseModule,
        HealthModule,
        GithubSocialModule,
        GoogleSocialModule,
        SignInHttpModule,
    ],
)]
pub struct AuthModule;
