//! Every first-party `for_root` imported twice — unpinned, pinned, beside a
//! bare import, in both orders — boots wired as if imported once, or refuses
//! naming both imports: a feature declaring its dependency beside the app's
//! own import never wires the app twice.

use std::time::Duration;

use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_config::ConfigModule;
use nest_rs_core::{
    App, CloseContribution, Container, ContestedDeclarationError, Discovery, Module,
    TransportContribution, UnresolvedFactoryError, WiringContribution, instance, module,
};
use nest_rs_graphql::{GraphqlConfig, GraphqlModule};
use nest_rs_health::{HealthConfig, HealthModule};
use nest_rs_http::{
    HttpConfig, HttpControllerMeta, HttpEndpointMeta, HttpFallbackMeta, HttpModule,
};
use nest_rs_mcp::{McpConfig, McpModule};
use nest_rs_oauth_client::{OAuthClientConfig, OAuthClientModule};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
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

/// What a row seeds into every app it boots, so its resource opens in process.
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

/// A row of the table: the setup unpinned, pinned, and — where the module is a
/// dependency a feature may declare bare — that bare import.
macro_rules! imported_twice {
    ($row:ident, seeds = $seeds:expr, unpinned = [$($u:tt)*] $(,)?) => {
        mod $row {
            use super::*;

            #[module(imports = [$($u)*])]
            struct OnceUnpinned;

            #[module(imports = [$($u)*, $($u)*])]
            struct TwiceUnpinned;

            #[tokio::test]
            async fn imported_twice_wires_once() {
                let seeds: Seeds = $seeds;
                assert_eq!(
                    wiring::<TwiceUnpinned>(seeds).await,
                    wiring::<OnceUnpinned>(seeds).await,
                );
            }
        }
    };
    (
        $row:ident,
        seeds = $seeds:expr,
        unpinned = [$($u:tt)*],
        pinned = [$($p:tt)*],
        bare = [$($b:tt)*] $(,)?
    ) => {
        imported_twice!(@row $row, $seeds, [$($u)*], [$($p)*], [BareThenPinned, PinnedThenBare], {
            #[module(imports = [$($b)*, $($p)*])]
            struct BareThenPinned;

            #[module(imports = [$($p)*, $($b)*])]
            struct PinnedThenBare;
        });
    };
    ($row:ident, seeds = $seeds:expr, unpinned = [$($u:tt)*], pinned = [$($p:tt)*] $(,)?) => {
        imported_twice!(@row $row, $seeds, [$($u)*], [$($p)*], [], {});
    };
    (
        @row $row:ident,
        $seeds:expr,
        [$($u:tt)*],
        [$($p:tt)*],
        [$($beside:ident),*],
        { $($items:tt)* }
    ) => {
        mod $row {
            use super::*;

            #[module(imports = [$($u)*])]
            struct OnceUnpinned;

            #[module(imports = [$($p)*])]
            struct OncePinned;

            #[module(imports = [$($u)*, $($u)*])]
            struct TwiceUnpinned;

            #[module(imports = [$($u)*, $($p)*])]
            struct UnpinnedThenPinned;

            #[module(imports = [$($p)*, $($u)*])]
            struct PinnedThenUnpinned;

            #[module(imports = [$($p)*, $($p)*])]
            struct TwicePinned;

            $($items)*

            #[tokio::test]
            async fn imported_twice_wires_once_or_names_both_imports() {
                let seeds: Seeds = $seeds;
                assert_eq!(
                    wiring::<TwiceUnpinned>(seeds).await,
                    wiring::<OnceUnpinned>(seeds).await,
                    "TwiceUnpinned",
                );
                let pinned = wiring::<OncePinned>(seeds).await;
                assert_eq!(wiring::<UnpinnedThenPinned>(seeds).await, pinned, "UnpinnedThenPinned");
                assert_eq!(wiring::<PinnedThenUnpinned>(seeds).await, pinned, "PinnedThenUnpinned");
                $(
                    assert_eq!(wiring::<$beside>(seeds).await, pinned, stringify!($beside));
                )*
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
        enabled: true,
        emit_document: false,
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
    config,
    seeds = unseeded,
    unpinned = [ConfigModule::for_root()]
);

imported_twice!(
    http,
    seeds = unseeded,
    unpinned = [HttpModule::for_root(None)],
    pinned = [HttpModule::for_root(pinned_http())],
    bare = [HttpModule],
);

imported_twice!(
    graphql,
    seeds = unseeded,
    unpinned = [GraphqlModule::for_root(None)],
    pinned = [GraphqlModule::for_root(GraphqlConfig::default())],
    bare = [GraphqlModule],
);

imported_twice!(
    openapi,
    seeds = unseeded,
    unpinned = [OpenApiModule::for_root(None)],
    pinned = [OpenApiModule::for_root(pinned_openapi())],
    bare = [OpenApiModule],
);

imported_twice!(
    mcp,
    seeds = unseeded,
    unpinned = [McpModule::for_root(None)],
    pinned = [McpModule::for_root(McpConfig::default())],
    bare = [McpModule],
);

imported_twice!(
    queue,
    seeds = unseeded,
    unpinned = [QueueModule::for_root(None)],
    pinned = [QueueModule::for_root(QueueConfig::default())],
    bare = [QueueModule],
);

imported_twice!(
    outbox,
    seeds = unseeded,
    unpinned = [OutboxModule::for_root(None)],
    pinned = [OutboxModule::for_root(OutboxConfig::default())],
    bare = [OutboxModule],
);

imported_twice!(
    ws,
    seeds = unseeded,
    unpinned = [WsModule::for_root(None)],
    pinned = [WsModule::for_root(WsConfig::default())],
    bare = [WsModule],
);

imported_twice!(
    health,
    seeds = unseeded,
    unpinned = [HealthModule::for_root(None)],
    pinned = [HealthModule::for_root(HealthConfig::default())],
    bare = [HealthModule],
);

imported_twice!(
    server_timing,
    seeds = unseeded,
    unpinned = [ServerTimingModule::for_root(None)],
    pinned = [ServerTimingModule::for_root(ServerTimingConfig::default())],
    bare = [ServerTimingModule],
);

imported_twice!(
    static_files,
    seeds = |app| app.provide(pinned_static_files()),
    unpinned = [StaticFilesModule::for_root(None)],
    pinned = [StaticFilesModule::for_root(pinned_static_files())],
    bare = [StaticFilesModule],
);

imported_twice!(
    storage,
    seeds = |app| app.provide(pinned_storage()),
    unpinned = [StorageModule::for_root(None)],
    pinned = [StorageModule::for_root(pinned_storage())],
    bare = [StorageModule],
);

imported_twice!(
    throttler,
    seeds = unseeded,
    unpinned = [ThrottlerModule::for_root(None)],
    pinned = [ThrottlerModule::for_root(ThrottlerConfig::default())],
);

imported_twice!(
    authn,
    seeds = |app| app.provide(pinned_authn()),
    unpinned = [AuthnModule::for_root(None)],
    pinned = [AuthnModule::for_root(pinned_authn())],
);

imported_twice!(
    oauth_client,
    seeds = |app| app.provide(pinned_oauth_client()),
    unpinned = [OAuthClientModule::for_root(None)],
    pinned = [OAuthClientModule::for_root(pinned_oauth_client())],
);

imported_twice!(
    oauth_resource,
    seeds = |app| app.provide(pinned_oauth_resource()),
    unpinned = [OAuthResourceModule::for_root(None)],
    pinned = [OAuthResourceModule::for_root(pinned_oauth_resource())],
);

imported_twice!(
    seaorm,
    seeds = |app| {
        app.provide(nest_rs_seaorm::sea_orm::DatabaseConnection::default())
            .provide(SeaOrmConfig::default())
    },
    unpinned = [SeaOrmModule::for_root(None)],
    pinned = [SeaOrmModule::for_root(SeaOrmConfig::default())],
);

/// A Redis connection opens only against a server, so the Redis rows stop
/// where the boot stops without one: after the declarations, before any
/// factory runs — the synchronous boot, which drains none.
fn declarations_of<M: Module>() -> anyhow::Error {
    let Err(refused) = App::new::<M>() else {
        panic!(
            "{} queues a connection no synchronous boot opens",
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
    RedisModule::for_root(None),
    RedisModule::for_root(pinned_redis()),
    RedisModule::for_root((Cache, pinned_redis())),
])]
struct RedisEachInstancePinnedOnce;

#[module(imports = [
    RedisModule::for_root(None),
    RedisQueueModule::for_root(RedisQueueConfig::default()),
    RedisQueueModule::for_root(RedisQueueConfig::default()),
])]
struct RedisQueueTwicePinned;

/// One instance pinned twice is contested, the default's as a named one's;
/// two instances each pinned once never contest.
#[test]
fn a_redis_instance_pinned_twice_names_both_imports_and_two_instances_never_contest() {
    for (refused, root) in [
        (declarations_of::<RedisTwicePinned>(), "RedisTwicePinned"),
        (
            declarations_of::<RedisInstanceTwicePinned>(),
            "RedisInstanceTwicePinned",
        ),
    ] {
        names_both_imports(&refused, root, [0, 1]);
    }
    let refused = declarations_of::<RedisEachInstancePinnedOnce>();
    assert!(
        refused.downcast_ref::<UnresolvedFactoryError>().is_some(),
        "past the declarations, refused only for the connection it cannot open: {refused:#}"
    );
}

#[test]
fn a_redis_binding_pinned_twice_names_both_imports() {
    names_both_imports(
        &declarations_of::<RedisQueueTwicePinned>(),
        "RedisQueueTwicePinned",
        [1, 2],
    );
}
