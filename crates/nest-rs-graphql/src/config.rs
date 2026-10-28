//! [`GraphqlConfig`] — loaded from `<PREFIX>_GRAPHQL__*`, every field
//! defaulting production-safe, the development surfaces armed by the declared
//! profile alone.

use std::path::PathBuf;
use std::time::Duration;

use nest_rs_config::{
    Bound, Config, ConfigService, DurationBounds, Environment, Floor, Result, config,
};

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
    Floor::Units(Bound::new(
        1,
        "a client on a slow link cannot finish its first message below it",
    )),
    Bound::new(
        60,
        "an idle socket held longer is the exhaustion this bound exists for",
    ),
);

/// A hundred operations at once on one socket.
const DEFAULT_MAX_SOCKET_OPERATIONS: usize = 100;

/// A hundred entity references per `_entities` call — a page of parents.
const DEFAULT_MAX_REPRESENTATIONS: usize = 100;

/// What the boot says when a deployment serves the playground.
const PLAYGROUND_OPEN: &str = "GraphQL playground is enabled and public outside a dev profile";

/// What the boot says when a deployment answers introspection.
const INTROSPECTION_OPEN: &str =
    "GraphQL introspection is enabled and public outside a dev profile";

/// GraphQL endpoint options, settable via `<PREFIX>_GRAPHQL__*` or pinned through
/// [`GraphqlModule::for_root`](crate::GraphqlModule::for_root).
///
/// `Default` is the production-safe value: no playground, no introspection, no
/// schema written. A declared development or test profile's
/// [`Config::defaults`] serves the playground and answers introspection, and a
/// development profile's writes the schema; a config pinned over
/// `..Default::default()` gets none of it in any profile.
#[config(namespace = "graphql")]
#[derive(Clone, Debug)]
pub struct GraphqlConfig {
    /// Endpoint path. Default `/graphql`. One literal path: a template's `{`
    /// `}`, or a `:`, `*` or `<` the router reads as a parameter, fails the
    /// boot.
    pub path: String,
    /// Serve the GraphQL Playground on `GET` for a browser's request.
    ///
    /// The page is public, so `Default` keeps it off and only a development or
    /// test profile's [`Config::defaults`] turns it on. Serving it outside such
    /// a profile is honoured and logged at `warn`.
    pub playground: bool,
    /// (Re)write [`emit_path`](Self::emit_path) with the schema's SDL once at
    /// boot — the committed copy a pull request diffs. `Default` keeps it off,
    /// and only a development profile's [`Config::defaults`] turns it on: under
    /// test, parallel suites would rewrite a committed file. A write failure is
    /// logged, never fatal.
    pub emit: bool,
    /// Where [`emit`](Self::emit) writes the SDL. Default `schema.graphql`; a
    /// relative path is the app crate's — the one `#[nest_rs::main]` expanded
    /// in — so each app of a workspace writes its own.
    pub emit_path: PathBuf,
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
    /// Answer introspection queries. The schema is the API's documentation,
    /// so `Default` keeps this off and only a development or test profile's
    /// [`Config::defaults`] turns it on. Answering them outside such a profile
    /// is honoured and logged at `warn`.
    pub introspection: bool,
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
    /// `introspection` does not cover it — so a subgraph publishes its
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
            emit: false,
            emit_path: "schema.graphql".into(),
            max_depth: Some(15),
            max_complexity: Some(2000),
            introspection: false,
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
    // Not in `from_env`: overlaying it would rewrite a pinned value.
    fn defaults() -> Self {
        let environment = Environment::from_env();
        let open = !environment.is_deployed();
        Self {
            playground: open,
            introspection: open,
            emit: environment == Environment::Development,
            ..Self::default()
        }
    }

    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let d = base;
        let playground = env.flag("PLAYGROUND", d.playground)?;
        let introspection = env.flag("INTROSPECTION", d.introspection)?;
        let environment = Environment::from_env();
        if environment.is_deployed() {
            if playground {
                tracing::warn!(
                    target: crate::TARGET,
                    environment = environment.as_str(),
                    "{PLAYGROUND_OPEN}",
                );
            }
            if introspection {
                tracing::warn!(
                    target: crate::TARGET,
                    environment = environment.as_str(),
                    "{INTROSPECTION_OPEN}",
                );
            }
        }
        Ok(Self {
            path: env.get("PATH")?.unwrap_or(d.path),
            playground,
            emit: env.flag("EMIT", d.emit)?,
            emit_path: env
                .get("EMIT_PATH")?
                .map(PathBuf::from)
                .unwrap_or(d.emit_path),
            max_depth: env.parse("MAX_DEPTH")?.or(d.max_depth),
            max_complexity: env.parse("MAX_COMPLEXITY")?.or(d.max_complexity),
            introspection,
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
        assert!(!d.emit, "writing SDL from prod is unwanted side effect");
        assert_eq!(d.emit_path, PathBuf::from("schema.graphql"));
        assert_eq!(d.max_depth, Some(15));
        assert_eq!(d.max_complexity, Some(2000));
        assert!(!d.introspection);
        assert_eq!(d.max_batch_size, 10);
        assert!(
            !d.federation,
            "a subgraph publishes its own SDL through `_service`, which no config \
             can switch off — so being one is opted into, and an `#[entity]` \
             declared without it fails the boot rather than inheriting it",
        );
    }

    /// `profile` declared on the process for `read`, `""` declaring none.
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn in_profile<T>(profile: &str, read: impl FnOnce() -> T) -> T {
        let mut out = None;
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::Environment::var_name(), profile);
            out = Some(read());
            Ok(())
        });
        out.expect("the jailed read ran")
    }

    #[test]
    fn the_unpinned_default_opens_the_playground_and_introspection_in_development_and_test_alone() {
        for (profile, open) in [
            ("development", true),
            ("test", true),
            ("staging", false),
            ("production", false),
            ("", false),
        ] {
            let d = in_profile(profile, <GraphqlConfig as Config>::defaults);
            assert_eq!(d.playground, open, "{profile:?}");
            assert_eq!(d.introspection, open, "{profile:?}");
        }
    }

    #[test]
    fn the_unpinned_default_emits_the_schema_under_development_alone() {
        for (profile, emits) in [
            ("development", true),
            ("test", false),
            ("staging", false),
            ("production", false),
            ("", false),
        ] {
            let d = in_profile(profile, <GraphqlConfig as Config>::defaults);
            assert_eq!(d.emit, emits, "{profile:?}");
        }
    }

    #[test]
    fn a_pin_over_the_struct_default_opens_nothing_in_development() {
        let cfg = in_profile("development", || {
            GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", []),
                GraphqlConfig {
                    max_depth: Some(9),
                    ..GraphqlConfig::default()
                },
            )
            .expect("ok")
        });
        assert!(!cfg.playground && !cfg.introspection && !cfg.emit);
    }

    #[test]
    fn a_surface_opened_outside_a_dev_profile_is_reported_by_name() {
        let logs = nest_rs_testing::LogCapture::install();
        let cfg = in_profile("production", || {
            GraphqlConfig::from_env(
                &ConfigService::with_vars(
                    "graphql",
                    [("PLAYGROUND", "true"), ("INTROSPECTION", "true")],
                ),
                GraphqlConfig::default(),
            )
            .expect("an explicit opening is honoured")
        });
        assert!(cfg.playground && cfg.introspection);
        for surface in [PLAYGROUND_OPEN, INTROSPECTION_OPEN] {
            let event = logs.expect_one("nest_rs::graphql", surface);
            assert_eq!(event.level, "warn");
            assert_eq!(event.field("environment").as_deref(), Some("production"));
        }
    }

    #[test]
    fn the_surfaces_open_in_development_are_silent() {
        let logs = nest_rs_testing::LogCapture::install();
        in_profile("development", || {
            GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", []),
                <GraphqlConfig as Config>::defaults(),
            )
        })
        .expect("the profile's defaults resolve");
        for surface in [PLAYGROUND_OPEN, INTROSPECTION_OPEN] {
            assert!(
                logs.find("nest_rs::graphql", surface).is_empty(),
                "the default posture is not an incident: {:#?}",
                logs.events(),
            );
        }
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
        assert_eq!(cfg.emit_path, d.emit_path);
        assert_eq!(cfg.emit, d.emit);
        assert_eq!(cfg.max_depth, d.max_depth);
        assert_eq!(cfg.max_complexity, d.max_complexity);
        assert_eq!(cfg.introspection, d.introspection);
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
                ("EMIT_PATH", "./schema-out.graphql"),
                ("EMIT", "true"),
                ("MAX_DEPTH", "15"),
                ("MAX_COMPLEXITY", "2000"),
            ],
        );
        let cfg = GraphqlConfig::from_env(&service, Default::default()).expect("ok");
        assert_eq!(cfg.path, "/api/graphql");
        assert!(cfg.playground);
        assert_eq!(cfg.emit_path, PathBuf::from("./schema-out.graphql"));
        assert!(cfg.emit);
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
