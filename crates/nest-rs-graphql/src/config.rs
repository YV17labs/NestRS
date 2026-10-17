//! [`GraphqlConfig`] — loaded from `<PREFIX>_GRAPHQL__*`, every field
//! defaulting production-safe.

use std::path::PathBuf;
use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};

pub(crate) const DEFAULT_PATH: &str = "/graphql";

/// Four hours, as `<PREFIX>_WS__MAX_CONNECTION_SECS` defaults.
const DEFAULT_MAX_CONNECTION_SECS: u64 = 4 * 60 * 60;

/// The subscription socket ceiling's range and the variable that sets it.
const MAX_CONNECTION: DurationBounds = DurationBounds::secs(
    "MAX_CONNECTION_SECS",
    "GraphqlConfig::max_connection",
    nest_rs_http::MAX_CONNECTION_FLOOR,
    nest_rs_http::MAX_CONNECTION_CEILING,
);

/// 64 KiB, as `<PREFIX>_WS__MAX_MESSAGE_BYTES` defaults: a `subscribe` carries
/// one document and its variables.
const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Three seconds, the graphql-ws reference server's `connectionInitWaitTimeout`.
const DEFAULT_CONNECTION_INIT_TIMEOUT_SECS: u64 = 3;

/// The wait for `connection_init`'s range and the variable that sets it.
const CONNECTION_INIT_TIMEOUT: DurationBounds = DurationBounds::secs(
    "CONNECTION_INIT_TIMEOUT_SECS",
    "GraphqlConfig::connection_init_timeout",
    Floor::Units(Bound {
        count: 1,
        why: "a client on a slow link cannot finish its first message below it",
    }),
    Bound {
        count: 60,
        why: "an idle socket held longer is the exhaustion this bound exists for",
    },
);

/// A hundred operations at once on one socket.
const DEFAULT_MAX_SOCKET_OPERATIONS: usize = 100;

/// A hundred entity references per `_entities` call — a page of parents.
const DEFAULT_MAX_REPRESENTATIONS: usize = 100;

/// GraphQL endpoint options, settable via `<PREFIX>_GRAPHQL__*` or pinned through
/// [`GraphqlModule::for_root`](crate::GraphqlModule::for_root). Every field
/// defaults production-safe.
#[config(namespace = "graphql")]
#[derive(Clone, Debug)]
pub struct GraphqlConfig {
    /// Endpoint path. Default `/graphql`. One literal path: a template's `{`
    /// `}`, or a `:`, `*` or `<` the router reads as a parameter, fails the
    /// boot.
    pub path: String,
    /// Default `false` (production-safe).
    pub playground: bool,
    /// Where the committed SDL lives. Default `schema.graphql`.
    pub schema_path: PathBuf,
    /// (Re)write `schema_path` from the live schema once at boot. Default
    /// `false`. A write failure is logged, never fatal.
    pub emit_sdl: bool,
    /// Maximum nesting depth of an incoming query AST. Defaults to `Some(15)`;
    /// `None` disables the check.
    ///
    /// `Some(0)` is rejected at boot: async-graphql checks `depth > limit`
    /// strictly and every field has depth ≥ 1, so `0` would refuse every query.
    #[validate(range(min = 1))]
    pub max_depth: Option<usize>,
    /// Maximum complexity score of an incoming query AST: 1 per field, plus
    /// the multiplier `#[expose]` sets on list relations. Defaults to
    /// `Some(2000)`; `None` disables the check. `Some(0)` is rejected at boot,
    /// as for `max_depth`.
    #[validate(range(min = 1))]
    pub max_complexity: Option<usize>,
    /// Disable GraphQL introspection. Default `true` (production-safe).
    pub disable_introspection: bool,
    /// Maximum number of operations in a single HTTP batch request.
    /// Default `10`.
    #[validate(range(min = 1))]
    pub max_batch_size: usize,
    /// Maximum lifetime of one graphql-ws subscription socket; it then closes
    /// with `1001 Going Away`, so the peer must re-upgrade. A **security**
    /// control: a subscription captures its principal once at the upgrade.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_CONNECTION_SECS`, whole seconds from 1 to
    /// 86400 or `0` for unlimited; defaults to 4 hours.
    pub max_connection: Option<Duration>,
    /// The graphql-ws socket's per-message and per-frame byte cap, held at the
    /// protocol layer so a larger frame is refused before it is buffered, and
    /// the socket closed.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_MESSAGE_BYTES`; defaults to 64 KiB, as
    /// `<PREFIX>_WS__MAX_MESSAGE_BYTES` does.
    #[validate(range(min = 1, message = "must be at least 1 byte"))]
    pub max_message_bytes: usize,
    /// How long an upgraded graphql-ws socket may wait for `connection_init`
    /// before it is closed `4408 Connection initialisation timeout`.
    ///
    /// Read from `<PREFIX>_GRAPHQL__CONNECTION_INIT_TIMEOUT_SECS`, whole seconds
    /// from 1 to 60, never off; defaults to 3 seconds.
    pub connection_init_timeout: Duration,
    /// Operations one graphql-ws socket may run at once; a `subscribe` past it
    /// is answered with an `error` message for its id, and the socket stays.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_SOCKET_OPERATIONS`; defaults to 100.
    /// **`None` and `Some(0)` both mean unlimited**, as for
    /// `max_representations`.
    pub max_socket_operations: Option<usize>,
    /// Serve this schema as an Apollo **subgraph**: the federation directives
    /// are declared, and the emitted SDL is the subgraph form.
    ///
    /// async-graphql serves `_service` and `_entities` as soon as any `#[entity]`
    /// resolver registers its keys, whatever this flag says, so an entity while
    /// this is `false` **fails the boot**.
    ///
    /// Default `false`: **`_service` cannot be switched off** —
    /// `disable_introspection` does not cover it — so a subgraph publishes its
    /// SDL to anyone who can reach it, and belongs behind a router.
    ///
    /// Read from `<PREFIX>_GRAPHQL__FEDERATION`.
    pub federation: bool,
    /// Maximum number of entity references one `_entities` call may carry —
    /// a count neither `max_depth`, `max_complexity` nor `max_batch_size` sees,
    /// though each reference is a full resolution run concurrently. Over the
    /// ceiling is a GraphQL error naming it, never a truncation.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_REPRESENTATIONS`; defaults to 100.
    /// **`None` and `Some(0)` both mean unlimited here**, where `max_depth` and
    /// `max_complexity` refuse `0`.
    pub max_representations: Option<usize>,
    /// Promote the default `warn` on a linked `#[resolver]` no module lists in
    /// `providers` — filtered out of the schema — into a boot failure.
    ///
    /// Default `false`: several binaries over one feature library legitimately
    /// link resolvers a given app does not serve.
    ///
    /// Read from `<PREFIX>_GRAPHQL__STRICT_RESOLVER_MEMBERSHIP`.
    pub strict_resolver_membership: bool,
}

impl Default for GraphqlConfig {
    fn default() -> Self {
        Self {
            path: DEFAULT_PATH.into(),
            playground: false,
            schema_path: "schema.graphql".into(),
            emit_sdl: false,
            max_depth: Some(15),
            max_complexity: Some(2000),
            disable_introspection: true,
            max_batch_size: 10,
            max_connection: Some(Duration::from_secs(DEFAULT_MAX_CONNECTION_SECS)),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            connection_init_timeout: Duration::from_secs(DEFAULT_CONNECTION_INIT_TIMEOUT_SECS),
            max_socket_operations: Some(DEFAULT_MAX_SOCKET_OPERATIONS),
            federation: false,
            max_representations: Some(DEFAULT_MAX_REPRESENTATIONS),
            strict_resolver_membership: false,
        }
    }
}

impl Config for GraphqlConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let d = base;
        Ok(Self {
            path: env.get("PATH")?.unwrap_or(d.path),
            playground: env.flag("PLAYGROUND", d.playground)?,
            schema_path: env
                .get("SCHEMA_PATH")?
                .map(PathBuf::from)
                .unwrap_or(d.schema_path),
            emit_sdl: env.flag("EMIT_SDL", d.emit_sdl)?,
            max_depth: env.parse("MAX_DEPTH")?.or(d.max_depth),
            max_complexity: env.parse("MAX_COMPLEXITY")?.or(d.max_complexity),
            disable_introspection: env.flag("DISABLE_INTROSPECTION", d.disable_introspection)?,
            max_batch_size: env.parse("MAX_BATCH_SIZE")?.unwrap_or(d.max_batch_size),
            max_connection: MAX_CONNECTION
                .read_optional(env, d.max_connection)?
                .map(|read| read.value),
            max_message_bytes: env
                .parse("MAX_MESSAGE_BYTES")?
                .unwrap_or(d.max_message_bytes),
            connection_init_timeout: CONNECTION_INIT_TIMEOUT
                .read(env, d.connection_init_timeout)?
                .value,
            max_socket_operations: env.count("MAX_SOCKET_OPERATIONS", d.max_socket_operations)?,
            federation: env.flag("FEDERATION", d.federation)?,
            max_representations: env.count("MAX_REPRESENTATIONS", d.max_representations)?,
            strict_resolver_membership: env
                .flag("STRICT_RESOLVER_MEMBERSHIP", d.strict_resolver_membership)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    #[test]
    fn defaults_are_production_safe() {
        let d = GraphqlConfig::default();
        assert_eq!(d.path, "/graphql");
        assert!(!d.playground, "playground exposed in prod is a CVE");
        assert!(!d.emit_sdl, "writing SDL from prod is unwanted side effect");
        assert_eq!(d.schema_path, PathBuf::from("schema.graphql"));
        assert_eq!(d.max_depth, Some(15));
        assert_eq!(d.max_complexity, Some(2000));
        assert!(d.disable_introspection);
        assert_eq!(d.max_batch_size, 10);
        assert!(
            !d.federation,
            "a subgraph publishes its own SDL through `_service`, which no config \
             can switch off — so being one is opted into, and an `#[entity]` \
             declared without it fails the boot rather than inheriting it",
        );
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = GraphqlConfig {
            path: "/pinned-graphql".into(),
            max_depth: Some(3),
            ..Default::default()
        };
        let cfg = GraphqlConfig::from_env(
            &ConfigService::with_vars("graphql", [("MAX_DEPTH", "9")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.max_depth, Some(9), "the env outranks the pin");
        assert_eq!(
            cfg.path, "/pinned-graphql",
            "and the field the env is silent about keeps the pin",
        );
    }

    #[test]
    fn default_path_constant_pins_the_mount_point() {
        assert_eq!(DEFAULT_PATH, "/graphql");
    }

    #[test]
    fn from_env_falls_back_to_defaults_when_unset() {
        let cfg =
            GraphqlConfig::from_env(&ConfigService::with_vars("graphql", []), Default::default())
                .expect("ok");
        let d = GraphqlConfig::default();
        assert_eq!(cfg.path, d.path);
        assert_eq!(cfg.playground, d.playground);
        assert_eq!(cfg.schema_path, d.schema_path);
        assert_eq!(cfg.emit_sdl, d.emit_sdl);
        assert_eq!(cfg.max_depth, d.max_depth);
        assert_eq!(cfg.max_complexity, d.max_complexity);
        assert_eq!(cfg.disable_introspection, d.disable_introspection);
        assert_eq!(cfg.max_batch_size, d.max_batch_size);
    }

    #[test]
    fn validate_rejects_zero_limits_so_some_zero_does_not_brick_the_endpoint() {
        let zero_depth = GraphqlConfig {
            max_depth: Some(0),
            ..GraphqlConfig::default()
        };
        assert!(
            zero_depth.validate().is_err(),
            "Some(0) must fail validation — none of the documented `disable` opts is `0`"
        );
        let zero_complexity = GraphqlConfig {
            max_complexity: Some(0),
            ..GraphqlConfig::default()
        };
        assert!(zero_complexity.validate().is_err());
        let tight = GraphqlConfig {
            max_depth: Some(1),
            max_complexity: Some(1),
            ..GraphqlConfig::default()
        };
        assert!(tight.validate().is_ok());
        assert!(GraphqlConfig::default().validate().is_ok());
    }

    #[test]
    fn from_env_reads_each_field_when_set() {
        let service = ConfigService::with_vars(
            "graphql",
            [
                ("PATH", "/api/graphql"),
                ("PLAYGROUND", "true"),
                ("SCHEMA_PATH", "./schema-out.graphql"),
                ("EMIT_SDL", "true"),
                ("MAX_DEPTH", "15"),
                ("MAX_COMPLEXITY", "2000"),
            ],
        );
        let cfg = GraphqlConfig::from_env(&service, Default::default()).expect("ok");
        assert_eq!(cfg.path, "/api/graphql");
        assert!(cfg.playground);
        assert_eq!(cfg.schema_path, PathBuf::from("./schema-out.graphql"));
        assert!(cfg.emit_sdl);
        assert_eq!(cfg.max_depth, Some(15));
        assert_eq!(cfg.max_complexity, Some(2000));
    }

    #[test]
    fn the_socket_is_bounded_in_size_handshake_and_concurrency_by_default() {
        let d = GraphqlConfig::default();
        assert_eq!(d.max_message_bytes, 64 * 1024, "as a gateway's socket");
        assert_eq!(
            d.connection_init_timeout,
            Duration::from_secs(3),
            "graphql-ws's reference wait"
        );
        assert_eq!(d.max_socket_operations, Some(100));
        assert!(d.validate().is_ok());
    }

    #[test]
    fn each_socket_bound_is_overlaid_by_its_own_variable() {
        let cfg = GraphqlConfig::from_env(
            &ConfigService::with_vars(
                "graphql",
                [
                    ("MAX_MESSAGE_BYTES", "2048"),
                    ("CONNECTION_INIT_TIMEOUT_SECS", "10"),
                    ("MAX_SOCKET_OPERATIONS", "5"),
                ],
            ),
            GraphqlConfig::default(),
        )
        .expect("every bound inside its range");
        assert_eq!(cfg.max_message_bytes, 2048);
        assert_eq!(cfg.connection_init_timeout, Duration::from_secs(10));
        assert_eq!(cfg.max_socket_operations, Some(5));

        let unlimited = GraphqlConfig::from_env(
            &ConfigService::with_vars("graphql", [("MAX_SOCKET_OPERATIONS", "0")]),
            GraphqlConfig::default(),
        )
        .expect("`0` is the unlimited sentinel");
        assert_eq!(unlimited.max_socket_operations, None);

        let pinned = GraphqlConfig {
            connection_init_timeout: Duration::from_secs(30),
            max_socket_operations: Some(7),
            ..GraphqlConfig::default()
        };
        let kept = GraphqlConfig::from_env(&ConfigService::with_vars("graphql", []), pinned)
            .expect("a pin inside the range");
        assert_eq!(kept.connection_init_timeout, Duration::from_secs(30));
        assert_eq!(kept.max_socket_operations, Some(7));
    }

    #[test]
    fn a_message_cap_of_zero_bytes_is_refused() {
        let zero = GraphqlConfig {
            max_message_bytes: 0,
            ..GraphqlConfig::default()
        };
        assert!(
            zero.validate().is_err(),
            "a zero cap would refuse every frame"
        );
    }

    #[test]
    fn the_wait_for_connection_init_is_held_to_a_second_and_a_minute_never_off() {
        let var = nest_rs_config::var_name("graphql", "CONNECTION_INIT_TIMEOUT_SECS");
        for (raw, said) in [
            ("0", "must be at least 1 second"),
            ("61", "must be at most 60 seconds"),
        ] {
            let refused = GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", [("CONNECTION_INIT_TIMEOUT_SECS", raw)]),
                GraphqlConfig::default(),
            )
            .expect_err("outside the range")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains(said),
                "{refused}"
            );
            assert!(!refused.contains("turn it off"), "{refused}");
        }
        for pinned in [Duration::ZERO, Duration::from_secs(120)] {
            let refused = GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", []),
                GraphqlConfig {
                    connection_init_timeout: pinned,
                    ..GraphqlConfig::default()
                },
            )
            .expect_err("a pin outside the range")
            .to_string();
            assert!(
                refused.contains("GraphqlConfig::connection_init_timeout"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_subscription_ceiling_outside_its_range_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("graphql", "MAX_CONNECTION_SECS");
        let from_env = GraphqlConfig::from_env(
            &ConfigService::with_vars("graphql", [("MAX_CONNECTION_SECS", "86401")]),
            GraphqlConfig::default(),
        )
        .expect_err("past a day")
        .to_string();
        assert!(
            from_env.contains(&var) && from_env.contains("must be at most 86400 seconds"),
            "{from_env}"
        );
        let pinned = GraphqlConfig::from_env(
            &ConfigService::with_vars("graphql", []),
            GraphqlConfig {
                max_connection: Some(Duration::ZERO),
                ..GraphqlConfig::default()
            },
        )
        .expect_err("a pinned zero")
        .to_string();
        assert!(pinned.contains("`None` turns it off"), "{pinned}");
        let off = GraphqlConfig::from_env(
            &ConfigService::with_vars("graphql", [("MAX_CONNECTION_SECS", "0")]),
            GraphqlConfig::default(),
        )
        .expect("`0` is off");
        assert_eq!(off.max_connection, None);
    }
}
