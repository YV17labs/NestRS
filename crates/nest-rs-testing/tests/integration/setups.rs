//! Every first-party module owning a config imported twice — bare, pinned
//! through its `for_root`, the two together in both orders — boots wired as if
//! imported once, or refuses naming both pins: a feature declaring its
//! dependency bare beside the app's own pin never wires the app twice.

use std::time::Duration;

use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_core::{
    App, CloseContribution, Container, ContestedDeclarationError, Discovery, Module,
    TransportContribution, WiringContribution, instance, module,
};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule};
use nest_rs_health::{HealthConfig, HealthModule};
use nest_rs_http::{
    HttpConfig, HttpControllerMeta, HttpEndpointMeta, HttpFallbackMeta, HttpModule,
};
use nest_rs_mcp::{McpConfig, McpModule};
use nest_rs_oauth_client::{OAuthClientConfig, OAuthClientModule};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
use nest_rs_oauth_server::{OAuthServerConfig, OAuthServerModule, RegisteredClient};
use nest_rs_openapi::{OpenApiConfig, OpenApiModule};
use nest_rs_queue::{OutboxConfig, OutboxModule, QueueConfig, QueueModule};
use nest_rs_redis::{RedisConfig, RedisModule, RedisQueueConfig, RedisQueueModule};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use nest_rs_server_timing::{ServerTimingConfig, ServerTimingModule};
use nest_rs_static_files::{StaticFilesConfig, StaticFilesModule};
use nest_rs_storage::{StorageConfig, StorageModule};
use nest_rs_testing::{HeadlessApp, TestAppBuilder};
use nest_rs_throttler::{ThrottlerConfig, ThrottlerModule};
use nest_rs_ws::{WsConfig, WsModule};

/// What a row seeds into every app it boots, so its resource opens in process,
/// and the root a module requires beside it.
type Seeds = fn(TestAppBuilder) -> TestAppBuilder;

/// What an app wired once per app: the kernel's steps and the HTTP surfaces.
#[derive(Debug, PartialEq, Eq)]
struct Wiring {
    transports: Vec<String>,
    wirings: Vec<&'static str>,
    closes: Vec<String>,
    controllers: usize,
    endpoints: usize,
    fallbacks: usize,
}

impl Wiring {
    fn of(container: &Container) -> Self {
        let discovery = Discovery::new(container);
        Self {
            transports: discovery
                .meta::<TransportContribution>()
                .iter()
                .map(|found| found.meta.name().to_owned())
                .collect(),
            wirings: discovery
                .meta::<WiringContribution>()
                .iter()
                .map(|found| found.meta.name())
                .collect(),
            closes: discovery
                .meta::<CloseContribution>()
                .iter()
                .map(|found| found.meta.name().to_owned())
                .collect(),
            controllers: discovery.meta::<HttpControllerMeta>().len(),
            endpoints: discovery.meta::<HttpEndpointMeta>().len(),
            fallbacks: discovery.meta::<HttpFallbackMeta>().len(),
        }
    }
}

/// How `M`'s app is wired once booted over `seeds`.
async fn wiring<M: Module>(seeds: Seeds) -> Wiring {
    let app = seeds(HeadlessApp::builder())
        .module::<M>()
        .build_headless()
        .await
        .unwrap_or_else(|refused| panic!("{} boots: {refused:#}", std::any::type_name::<M>()));
    Wiring::of(app.container())
}

/// `refused` is a contest naming `root`'s imports at `first` and `second`.
fn names_both_imports(refused: &anyhow::Error, root: &str, [first, second]: [usize; 2]) {
    let contested = refused
        .downcast_ref::<ContestedDeclarationError>()
        .unwrap_or_else(|| panic!("{root}: not the contest: {refused:#}"));
    assert!(
        contested
            .first
            .ends_with(&format!("at `imports[{first}]` of `{root}`"))
            && contested
                .second
                .ends_with(&format!("at `imports[{second}]` of `{root}`")),
        "{root}: both imports are named: {contested}",
    );
}

/// `M`'s boot is refused as a contest naming its two imports, `M`'s first
/// and second, before any factory runs.
async fn contested<M: Module>(seeds: Seeds, root: &str) {
    let Err(refused) = seeds(HeadlessApp::builder())
        .module::<M>()
        .build_headless()
        .await
    else {
        panic!("{root} must not boot: two imports declare one value");
    };
    names_both_imports(&refused, root, [0, 1]);
}

/// A row of the table: the module imported bare, and pinned through its
/// `for_root`.
macro_rules! imported_twice {
    ($row:ident, seeds = $seeds:expr, bare = [$($b:tt)*], pinned = [$($p:tt)*] $(,)?) => {
        mod $row {
            use super::*;

            #[module(imports = [$($b)*])]
            struct OnceBare;

            #[module(imports = [$($p)*])]
            struct OncePinned;

            #[module(imports = [$($b)*, $($b)*])]
            struct TwiceBare;

            #[module(imports = [$($b)*, $($p)*])]
            struct BareThenPinned;

            #[module(imports = [$($p)*, $($b)*])]
            struct PinnedThenBare;

            #[module(imports = [$($p)*, $($p)*])]
            struct TwicePinned;

            #[tokio::test]
            async fn imported_twice_wires_once_or_names_both_imports() {
                let seeds: Seeds = $seeds;
                assert_eq!(
                    wiring::<TwiceBare>(seeds).await,
                    wiring::<OnceBare>(seeds).await,
                    "TwiceBare",
                );
                let pinned = wiring::<OncePinned>(seeds).await;
                assert_eq!(wiring::<BareThenPinned>(seeds).await, pinned, "BareThenPinned");
                assert_eq!(wiring::<PinnedThenBare>(seeds).await, pinned, "PinnedThenBare");
                contested::<TwicePinned>(seeds, "TwicePinned").await;
            }
        }
    };
}

fn unseeded(app: TestAppBuilder) -> TestAppBuilder {
    app
}

fn pinned_http() -> HttpConfig {
    HttpConfig {
        port: 3999,
        ..HttpConfig::default()
    }
}

fn pinned_openapi() -> OpenApiConfig {
    OpenApiConfig {
        enabled: nest_rs_config::Surface::Open,
        ..OpenApiConfig::default()
    }
}

fn pinned_static_files() -> StaticFilesConfig {
    StaticFilesConfig {
        root: Some(env!("CARGO_MANIFEST_DIR").into()),
        ..StaticFilesConfig::default()
    }
}

fn pinned_authn() -> AuthnConfig {
    AuthnConfig {
        secret: Some("setups-secret-padded-to-thirty-two".into()),
        ..AuthnConfig::default()
    }
}

fn pinned_oauth_server() -> OAuthServerConfig<()> {
    OAuthServerConfig {
        clients: vec![RegisteredClient::new(
            "setups-client",
            "setups-secret",
            ["setups:read"],
            ["https://api.example.com"],
            (),
        )],
    }
}

fn pinned_oauth_client() -> OAuthClientConfig {
    OAuthClientConfig {
        client_id: "setups-client".into(),
        client_secret: "setups-secret".into(),
        auth_url: "https://provider.example/authorize".into(),
        token_url: "https://provider.example/token".into(),
        redirect_url: "https://app.example/callback".into(),
        userinfo_url: "https://provider.example/userinfo".into(),
        scopes: vec!["read:user".into()],
        ..OAuthClientConfig::default()
    }
}

fn pinned_oauth_resource() -> OAuthResourceConfig {
    OAuthResourceConfig {
        resource: Some("https://api.example".into()),
        authorization_servers: vec!["https://auth.example".into()],
        ..OAuthResourceConfig::default()
    }
}

fn pinned_storage() -> StorageConfig {
    StorageConfig {
        url: "memory:///".into(),
        ..StorageConfig::default()
    }
}

imported_twice!(
    http,
    seeds = unseeded,
    bare = [HttpModule],
    pinned = [HttpModule::for_root(pinned_http())],
);

imported_twice!(
    graphql,
    seeds = unseeded,
    bare = [GraphqlModule],
    pinned = [GraphqlModule::for_root(GraphqlConfig::default())],
);

imported_twice!(
    openapi,
    seeds = unseeded,
    bare = [OpenApiModule],
    pinned = [OpenApiModule::for_root(pinned_openapi())],
);

imported_twice!(
    mcp,
    seeds = unseeded,
    bare = [McpModule],
    pinned = [McpModule::for_root(McpConfig::default())],
);

imported_twice!(
    queue,
    seeds = unseeded,
    bare = [QueueModule],
    pinned = [QueueModule::for_root(QueueConfig::default())],
);

imported_twice!(
    outbox,
    seeds = unseeded,
    bare = [OutboxModule],
    pinned = [OutboxModule::for_root(OutboxConfig::default())],
);

imported_twice!(
    ws,
    seeds = unseeded,
    bare = [WsModule],
    pinned = [WsModule::for_root(WsConfig::default())],
);

imported_twice!(
    health,
    seeds = unseeded,
    bare = [HealthModule],
    pinned = [HealthModule::for_root(HealthConfig::default())],
);

imported_twice!(
    server_timing,
    seeds = unseeded,
    bare = [ServerTimingModule],
    pinned = [ServerTimingModule::for_root(ServerTimingConfig::default())],
);

imported_twice!(
    static_files,
    seeds = |app| app.provide(pinned_static_files()),
    bare = [StaticFilesModule],
    pinned = [StaticFilesModule::for_root(pinned_static_files())],
);

imported_twice!(
    storage,
    seeds = |app| app.provide(pinned_storage()),
    bare = [StorageModule],
    pinned = [StorageModule::for_root(pinned_storage())],
);

imported_twice!(
    throttler,
    seeds = unseeded,
    bare = [ThrottlerModule],
    pinned = [ThrottlerModule::for_root(ThrottlerConfig::default())],
);

imported_twice!(
    authn,
    seeds = |app| app.provide(pinned_authn()),
    bare = [AuthnModule],
    pinned = [AuthnModule::for_root(pinned_authn())],
);

imported_twice!(
    oauth_client,
    seeds = |app| app.provide(pinned_oauth_client()),
    bare = [OAuthClientModule],
    pinned = [OAuthClientModule::for_root(pinned_oauth_client())],
);

imported_twice!(
    oauth_server,
    seeds = |app| app.provide(pinned_oauth_server()),
    bare = [OAuthServerModule::<()>],
    pinned = [OAuthServerModule::for_root(pinned_oauth_server())],
);

/// The token verifier `OAuthResourceModule` binds to its resource, which an app
/// imports beside it: a second root, so the row's contest stays at its own
/// imports.
#[module(imports = [AuthnModule])]
struct ProtectedVerifier;

imported_twice!(
    oauth_resource,
    seeds = |app| {
        app.provide(pinned_oauth_resource())
            .provide(pinned_authn())
            .module::<ProtectedVerifier>()
    },
    bare = [OAuthResourceModule],
    pinned = [OAuthResourceModule::for_root(pinned_oauth_resource())],
);

imported_twice!(
    seaorm,
    seeds = |app| {
        app.provide(nest_rs_seaorm::sea_orm::DatabaseConnection::default())
            .provide(SeaOrmConfig::default())
    },
    bare = [SeaOrmModule],
    pinned = [SeaOrmModule::for_root(SeaOrmConfig::default())],
);

/// A row of the second table: `$import` alone attaches the HTTP transport its
/// surface mounts on, so an app importing it serves it with no import of
/// `HttpModule` of its own.
macro_rules! mounts_on_http {
    ($row:ident, seeds = $seeds:expr, import = [$($i:tt)*] $(,)?) => {
        mod $row {
            use super::*;

            #[module(imports = [$($i)*])]
            struct Alone;

            #[tokio::test]
            async fn attaches_the_http_transport_it_mounts_on() {
                let seeds: Seeds = $seeds;
                assert_eq!(wiring::<Alone>(seeds).await.transports, ["http"]);
            }
        }
    };
}

mod mounts_on_http {
    use super::*;

    mounts_on_http!(graphql, seeds = unseeded, import = [GraphqlModule]);

    mounts_on_http!(
        openapi,
        seeds = unseeded,
        import = [OpenApiModule::for_root(pinned_openapi())],
    );

    mounts_on_http!(mcp, seeds = unseeded, import = [McpModule]);

    mounts_on_http!(health, seeds = unseeded, import = [HealthModule]);

    mounts_on_http!(
        static_files,
        seeds = |app| app.provide(pinned_static_files()),
        import = [StaticFilesModule],
    );

    mounts_on_http!(
        oauth_resource,
        seeds = |app| {
            app.provide(pinned_oauth_resource())
                .provide(pinned_authn())
                .module::<ProtectedVerifier>()
        },
        import = [OAuthResourceModule],
    );
}

/// The boot of `M`, refused by its declarations before any factory opens a
/// Redis connection.
async fn declarations_of<M: Module>() -> anyhow::Error {
    let Err(refused) = App::builder().module::<M>().build().await else {
        panic!(
            "{} declares twice, which refuses before any connection opens",
            std::any::type_name::<M>()
        );
    };
    refused
}

#[instance]
struct Cache;

fn pinned_redis() -> RedisConfig {
    RedisConfig {
        connect_timeout: Duration::from_secs(2),
        ..RedisConfig::default()
    }
}

#[module(imports = [RedisModule::for_root(pinned_redis()), RedisModule::for_root(pinned_redis())])]
struct RedisTwicePinned;

#[module(imports = [
    RedisModule::for_root((Cache, pinned_redis())),
    RedisModule::for_root((Cache, pinned_redis())),
])]
struct RedisInstanceTwicePinned;

#[module(imports = [
    RedisModule,
    RedisModule::for_root(pinned_redis()),
    RedisModule::<Cache>,
    RedisModule::for_root((Cache, pinned_redis())),
    RedisModule::<Cache>,
])]
struct RedisEachInstancePinnedOnce;

#[module(imports = [
    RedisModule,
    RedisQueueModule::for_root(RedisQueueConfig::default()),
    RedisQueueModule::for_root(RedisQueueConfig::default()),
])]
struct RedisQueueTwicePinned;

/// One instance pinned twice is contested, the default's as a named one's;
/// two instances each pinned once never contest, nor does either's bare
/// import beside its pin.
#[tokio::test]
async fn a_redis_instance_pinned_twice_names_both_imports_and_two_instances_never_contest() {
    for (refused, root) in [
        (
            declarations_of::<RedisTwicePinned>().await,
            "RedisTwicePinned",
        ),
        (
            declarations_of::<RedisInstanceTwicePinned>().await,
            "RedisInstanceTwicePinned",
        ),
    ] {
        names_both_imports(&refused, root, [0, 1]);
    }
    // Booted or refused for the connection its server answers or not, never
    // for its declarations.
    if let Err(refused) = App::builder()
        .module::<RedisEachInstancePinnedOnce>()
        .build()
        .await
    {
        assert!(
            refused
                .downcast_ref::<ContestedDeclarationError>()
                .is_none(),
            "two instances each pinned once never contest: {refused:#}"
        );
    }
}

#[tokio::test]
async fn a_redis_binding_pinned_twice_names_both_imports() {
    names_both_imports(
        &declarations_of::<RedisQueueTwicePinned>().await,
        "RedisQueueTwicePinned",
        [1, 2],
    );
}
