//! [`GraphqlConfig`] — loaded from `<PREFIX>_GRAPHQL__*`, every field
//! defaulting production-safe, the development surfaces left to the declared
//! profile.

use std::path::PathBuf;
use std::time::Duration;

use nest_rs_config::__private::{RetiredKey, RetiredKeys};
use nest_rs_config::{Bound, CountBounds, DurationBounds, Floor, Surface, config};

pub(crate) const DEFAULT_PATH: &str = "/graphql";

/// 64 KiB, as `<PREFIX>_WS__MAX_MESSAGE_BYTES` defaults: a `subscribe` carries
/// one document and its variables.
const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Three seconds, the graphql-ws reference server's `connectionInitWaitTimeout`.
const DEFAULT_CONNECTION_INIT_TIMEOUT_SECS: u64 = 3;

/// The range of the wait for `connection_init`.
const CONNECTION_INIT_TIMEOUT: DurationBounds = DurationBounds::secs(
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

const MAX_DEPTH: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "async-graphql checks `depth > limit` strictly and every field has depth 1 or more, so \
         a limit under one refuses every query",
    ),
    Bound::new(
        33,
        "async-graphql refuses a document nested past 33 fields by its own recursion bound \
         whatever this says, so a limit above it limits nothing",
    ),
);

/// Past ten million selections one document is a denial of service rather than a
/// read, which `max_complexity`'s off leaves a fragment's repeats held to.
const MOST_COMPLEXITY: usize = 10_000_000;

const MAX_COMPLEXITY: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "every field scores at least one, so a limit under one refuses every query",
    ),
    Bound::new(
        MOST_COMPLEXITY as u64,
        "past ten million resolved fields one query is a denial of service rather than a read; \
         a deployment that means no ceiling on the score turns it off",
    ),
);

const MAX_BATCH_SIZE: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a batch of none refuses every batch, a batch of one included",
    ),
    Bound::new(
        1024,
        "a batch runs every operation for one request's price, past the rate limiter and the \
         limits that see one document, and no client assembles more than a few dozen, so past \
         a thousand it is an amplification rather than a batch",
    ),
);

const MAX_SOCKET_OPERATIONS: CountBounds = CountBounds::new(
    Bound::new(1, "a cap of none refuses every `subscribe` on the socket"),
    Bound::new(
        10_000,
        "each operation holds a running stream on one socket, whose client a page of hundreds \
         already serves, so past ten thousand one peer holds what a replica's worth of \
         clients would; a deployment that means no ceiling turns it off",
    ),
);

const MAX_REPRESENTATIONS: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a cap of none refuses every `_entities` call a router sends",
    ),
    Bound::new(
        100_000,
        "each reference is a full resolution run concurrently, and a router sends a page's \
         worth, so past a hundred thousand one call is a fan-out no pool answers; a deployment \
         that means no ceiling turns it off",
    ),
);

/// What the boot says when a deployment serves the playground.
const PLAYGROUND_OPEN: &str = "GraphQL playground is enabled and public outside a dev profile";

/// What the boot says when a deployment answers introspection.
const INTROSPECTION_OPEN: &str =
    "GraphQL introspection is enabled and public outside a dev profile";

// `INTROSPECTION` is `DISABLE_INTROSPECTION`'s opposite, so a deployment that set
// it closed would find it answering under a development profile, and a rename by
// hand inverts it: refused, where the emit's two only move a committed artifact.
nest_rs_core::inventory::submit! {
    RetiredKeys::under("graphql", &[
        RetiredKey::renamed("EMIT_SDL", "EMIT"),
        RetiredKey::renamed("SCHEMA_PATH", "EMIT_PATH"),
        RetiredKey::refused(
            "DISABLE_INTROSPECTION",
            "INTROSPECTION",
            "its opposite, so `true` here is `false` there, and read by nothing it would leave \
             introspection answering under a development profile",
        ),
    ])
}

/// What the boot says when a deployment writes the committed SDL.
const EMIT_OPEN: &str = "GraphQL schema is written to disk outside a dev profile";

/// GraphQL endpoint options, settable via `<PREFIX>_GRAPHQL__*` or pinned through
/// [`GraphqlModule::for_root`](crate::GraphqlModule::for_root).
///
/// `Default` is the production-safe value: its development surfaces are
/// [`Surface::ByProfile`], so a deployment, or a process declaring no profile,
/// gets no playground, no introspection and no schema written. A declared
/// development or test profile serves the playground and answers introspection,
/// and a development profile writes the schema, on the bare import and under a
/// pin over `..Default::default()` alike.
#[config(namespace = "graphql")]
#[derive(Clone, Debug)]
pub struct GraphqlConfig {
    /// Endpoint path. Default `/graphql`. One literal path: a template's `{`
    /// `}`, or a `:`, `*` or `<` the router reads as a parameter, fails the
    /// boot.
    pub path: String,
    /// Serve the GraphQL Playground on `GET` for a browser's request.
    ///
    /// The page is public, so `Default` leaves it to the profile: served under
    /// a declared development or test profile alone. Serving it outside one,
    /// [`Surface::Open`] or `true` from the environment, is honoured and logged
    /// at `warn`.
    pub playground: Surface,
    /// (Re)write [`emit_path`](Self::emit_path) with the schema's SDL once at
    /// boot — the committed copy a pull request diffs. `Default` leaves it to
    /// the profile: written under a declared development profile alone, since
    /// under test parallel suites would rewrite a committed file. Writing it
    /// outside one, [`Surface::Open`] or `true` from the environment, is honoured
    /// and logged at `warn`. A write failure is logged, never fatal.
    pub emit: Surface,
    /// Where [`emit`](Self::emit) writes the SDL. Default `schema.graphql`; a
    /// relative path is the app crate's — the one `#[nest_rs::main]` expanded
    /// in — so each app of a workspace writes its own.
    pub emit_path: PathBuf,
    /// Maximum nesting depth of an incoming query AST. Defaults to `Some(15)`;
    /// `None` disables the check, as `0` does from
    /// `<PREFIX>_GRAPHQL__MAX_DEPTH`, from 1 to 33. async-graphql's own recursion
    /// bound refuses a document nested past 33 fields whatever this says, `None`
    /// included.
    ///
    /// A pinned `Some(0)` is rejected at boot: async-graphql checks
    /// `depth > limit` strictly and every field has depth ≥ 1, so `0` would
    /// refuse every query.
    #[config(bounds = MAX_DEPTH)]
    pub max_depth: Option<usize>,
    /// Maximum complexity score of an incoming query AST: 1 per field, plus
    /// the multiplier `#[expose]` sets on list relations. Defaults to
    /// `Some(2000)`, from 1 to ten million; `None` disables the check, as `0`
    /// does from the environment, and a pinned `Some(0)` is rejected at boot,
    /// as for `max_depth`.
    ///
    /// It also holds what spreading a fragment again adds to a document's
    /// selections, counted before anything walks them, which `None` leaves at the
    /// ten million no score may pass: a document of a kilobyte would otherwise hold
    /// a worker for minutes.
    #[config(bounds = MAX_COMPLEXITY)]
    pub max_complexity: Option<usize>,
    /// Answer introspection queries. The schema is the API's documentation, so
    /// `Default` leaves this to the profile: answered under a declared
    /// development or test profile alone. Answering them outside one,
    /// [`Surface::Open`] or `true` from the environment, is honoured and logged
    /// at `warn`.
    pub introspection: Surface,
    /// Maximum number of operations in a single HTTP batch request. Default
    /// `10`, from 1 to 1024; there is no off.
    #[config(bounds = MAX_BATCH_SIZE)]
    pub max_batch_size: usize,
    /// Maximum lifetime of one graphql-ws subscription socket; it then closes
    /// with `1001 Going Away`, so the peer must re-upgrade. A **security**
    /// control: a subscription captures its principal once at the upgrade.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_CONNECTION_SECS`, whole seconds from 1 to
    /// 86400 or `0` for unlimited; defaults to 4 hours,
    /// [`nest_rs_http::DEFAULT_MAX_CONNECTION`].
    #[config(bounds = nest_rs_http::MAX_CONNECTION_BOUNDS)]
    pub max_connection: Option<Duration>,
    /// The graphql-ws socket's per-message and per-frame byte cap, held at the
    /// protocol layer so a larger frame is refused before it is buffered, and
    /// the socket closed.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_MESSAGE_BYTES`, from 1 to a gibibyte
    /// ([`nest_rs_http::MAX_MESSAGE_BYTES_BOUNDS`]); defaults to 64 KiB, as
    /// `<PREFIX>_WS__MAX_MESSAGE_BYTES` does.
    #[config(bounds = nest_rs_http::MAX_MESSAGE_BYTES_BOUNDS)]
    pub max_message_bytes: usize,
    /// How long an upgraded graphql-ws socket may wait for `connection_init`
    /// before it is closed `4408 Connection initialisation timeout`.
    ///
    /// Read from `<PREFIX>_GRAPHQL__CONNECTION_INIT_TIMEOUT_SECS`, whole seconds
    /// from 1 to 60, never off; defaults to 3 seconds.
    #[config(bounds = CONNECTION_INIT_TIMEOUT)]
    pub connection_init_timeout: Duration,
    /// Operations one graphql-ws socket may run at once; a `subscribe` past it
    /// is answered with an `error` message for its id, and the socket stays.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_SOCKET_OPERATIONS`, from 1 to ten
    /// thousand; defaults to 100. `None` is unlimited, as `0` is from the
    /// environment; a pinned `Some(0)` fails the boot, as for every count here.
    #[config(bounds = MAX_SOCKET_OPERATIONS)]
    pub max_socket_operations: Option<usize>,
    /// Serve this schema as an Apollo **subgraph**: the federation directives
    /// are declared, and the emitted SDL is the subgraph form.
    ///
    /// async-graphql serves `_service` and `_entities` as soon as any `#[entity]`
    /// resolver registers its keys, whatever this flag says, so an entity while
    /// this is `false` **fails the boot**.
    ///
    /// Default `false`: **`_service` cannot be switched off** —
    /// `introspection` does not cover it — so a subgraph whose guards identify
    /// no one publishes its SDL to anyone who can reach it, and belongs behind a
    /// router.
    ///
    /// Read from `<PREFIX>_GRAPHQL__FEDERATION`.
    pub federation: bool,
    /// Maximum number of entity references one `_entities` call may carry —
    /// a count neither `max_depth`, `max_complexity` nor `max_batch_size` sees,
    /// though each reference is a full resolution run concurrently. Over the
    /// ceiling is a GraphQL error naming it, never a truncation.
    ///
    /// Read from `<PREFIX>_GRAPHQL__MAX_REPRESENTATIONS`, from 1 to a hundred
    /// thousand; defaults to 100. `None` is unlimited, as `0` is from the
    /// environment; a pinned `Some(0)` fails the boot, as for every count here.
    #[config(bounds = MAX_REPRESENTATIONS)]
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
            playground: Surface::ByProfile,
            emit: Surface::ByProfile,
            emit_path: "schema.graphql".into(),
            max_depth: Some(15),
            max_complexity: Some(2000),
            introspection: Surface::ByProfile,
            max_batch_size: 10,
            max_connection: Some(nest_rs_http::DEFAULT_MAX_CONNECTION),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            connection_init_timeout: Duration::from_secs(DEFAULT_CONNECTION_INIT_TIMEOUT_SECS),
            max_socket_operations: Some(DEFAULT_MAX_SOCKET_OPERATIONS),
            federation: false,
            max_representations: Some(DEFAULT_MAX_REPRESENTATIONS),
            strict_resolver_membership: false,
        }
    }
}

impl GraphqlConfig {
    /// What spreading a fragment again may add to a document, however
    /// [`max_complexity`](Self::max_complexity) scores its fields.
    pub(crate) fn repeated_selections(&self) -> usize {
        self.max_complexity.unwrap_or(MOST_COMPLEXITY)
    }

    /// One `warn` per surface open outside a development or test profile:
    /// honoured, since the deployment chose it, and said.
    pub(crate) fn warn_when_open(&self) {
        for (surface, said) in [
            (self.playground, PLAYGROUND_OPEN),
            (self.introspection, INTROSPECTION_OPEN),
            (self.emit, EMIT_OPEN),
        ] {
            if let Some(environment) = surface.opened_in_deployment() {
                tracing::warn!(
                    target: crate::TARGET,
                    environment = environment.as_str(),
                    "{said}",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::{Config, ConfigService};

    use super::*;

    /// A subscription socket's ceiling defaults to the value the HTTP transport
    /// exports for every long-lived connection, never a copy of it.
    #[test]
    fn the_socket_ceiling_is_the_transports_own() {
        assert_eq!(
            GraphqlConfig::default().max_connection,
            Some(nest_rs_http::DEFAULT_MAX_CONNECTION)
        );
    }

    #[test]
    fn a_fragments_repeats_are_held_to_the_limit_or_to_the_most_it_may_say() {
        assert_eq!(
            GraphqlConfig {
                max_complexity: Some(5),
                ..Default::default()
            }
            .repeated_selections(),
            5,
        );
        let off = GraphqlConfig {
            max_complexity: None,
            ..Default::default()
        }
        .repeated_selections();
        assert_eq!(
            Some(off),
            usize::try_from(GraphqlConfig::MAX_COMPLEXITY.bounds().most().count).ok(),
        );
    }

    #[test]
    fn defaults_are_production_safe() {
        let d = GraphqlConfig::default();
        assert_eq!(d.path, "/graphql");
        assert_eq!(d.playground, Surface::ByProfile);
        assert_eq!(d.introspection, Surface::ByProfile);
        assert_eq!(d.emit, Surface::ByProfile);
        assert_eq!(d.emit_path, PathBuf::from("schema.graphql"));
        assert_eq!(d.max_depth, Some(15));
        assert_eq!(d.max_complexity, Some(2000));
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
    fn the_default_opens_the_playground_and_introspection_in_development_and_test_alone() {
        for (profile, open) in [
            ("development", true),
            ("test", true),
            ("staging", false),
            ("production", false),
            ("", false),
        ] {
            let d = GraphqlConfig::default();
            assert_eq!(
                in_profile(profile, || d.playground.is_open()),
                open,
                "{profile:?}"
            );
            assert_eq!(
                in_profile(profile, || d.introspection.is_open()),
                open,
                "{profile:?}"
            );
        }
    }

    #[test]
    fn the_default_emits_the_schema_under_development_alone() {
        for (profile, emits) in [
            ("development", true),
            ("test", false),
            ("staging", false),
            ("production", false),
            ("", false),
        ] {
            assert_eq!(
                in_profile(profile, || GraphqlConfig::default().emit.writes()),
                emits,
                "{profile:?}"
            );
        }
    }

    #[test]
    fn a_surface_opened_outside_a_dev_profile_is_reported_by_name() {
        let logs = nest_rs_testing::LogCapture::install();
        let cfg = in_profile("production", || {
            let cfg = GraphqlConfig::from_env(
                &ConfigService::with_vars(
                    "graphql",
                    [("PLAYGROUND", "true"), ("INTROSPECTION", "true")],
                ),
                GraphqlConfig::default(),
            )
            .expect("an explicit opening is honoured");
            cfg.warn_when_open();
            cfg
        });
        assert_eq!(cfg.playground, Surface::Open);
        assert_eq!(cfg.introspection, Surface::Open);
        for surface in [PLAYGROUND_OPEN, INTROSPECTION_OPEN] {
            let event = logs.expect_one("nest_rs::graphql", surface);
            assert_eq!(event.level, "warn");
            assert_eq!(event.field("environment").as_deref(), Some("production"));
        }
    }

    /// A deployment writing the committed SDL is honoured, and says so.
    #[test]
    fn an_emit_opened_outside_a_dev_profile_is_reported() {
        for (profile, said) in [
            ("production", true),
            ("staging", true),
            ("", true),
            ("development", false),
            ("test", false),
        ] {
            let logs = nest_rs_testing::LogCapture::install();
            in_profile(profile, || {
                GraphqlConfig {
                    emit: Surface::Open,
                    ..GraphqlConfig::default()
                }
                .warn_when_open();
            });
            let line = logs.find(crate::TARGET, EMIT_OPEN);
            assert_eq!(line.len(), usize::from(said), "{profile:?}");
            if said {
                assert_eq!(line[0].level, "warn");
            }
        }
        let logs = nest_rs_testing::LogCapture::install();
        in_profile("production", || GraphqlConfig::default().warn_when_open());
        assert!(
            logs.find(crate::TARGET, EMIT_OPEN).is_empty(),
            "the default writes nothing in a deployment"
        );
    }

    #[test]
    fn the_surfaces_open_in_development_are_silent() {
        let logs = nest_rs_testing::LogCapture::install();
        in_profile("development", || {
            GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", []),
                GraphqlConfig::default(),
            )
            .expect("the default resolves")
            .warn_when_open();
        });
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
    fn a_pinned_zero_is_refused_so_some_zero_does_not_brick_the_endpoint() {
        for zero in [
            GraphqlConfig {
                max_depth: Some(0),
                ..GraphqlConfig::default()
            },
            GraphqlConfig {
                max_complexity: Some(0),
                ..GraphqlConfig::default()
            },
        ] {
            assert!(
                zero.check().is_err(),
                "Some(0) must fail the check — off in code is `None`"
            );
        }
        let tight = GraphqlConfig {
            max_depth: Some(1),
            max_complexity: Some(1),
            ..GraphqlConfig::default()
        };
        assert!(tight.check().is_ok());
        assert!(GraphqlConfig::default().check().is_ok());
    }

    /// One rule for every count: `None` is unlimited and nothing else, `0`
    /// from the environment is that `None`, and a pinned `Some(0)` is refused.
    #[test]
    fn every_count_reads_zero_as_unlimited_and_refuses_a_pinned_zero() {
        let keys = [
            "MAX_DEPTH",
            "MAX_COMPLEXITY",
            "MAX_SOCKET_OPERATIONS",
            "MAX_REPRESENTATIONS",
        ];
        let unlimited = GraphqlConfig::from_env(
            &ConfigService::with_vars("graphql", keys.map(|key| (key, "0"))),
            GraphqlConfig::default(),
        )
        .expect("`0` is unlimited");
        assert_eq!(
            [
                unlimited.max_depth,
                unlimited.max_complexity,
                unlimited.max_socket_operations,
                unlimited.max_representations,
            ],
            [None; 4]
        );
        for pinned in [
            GraphqlConfig {
                max_socket_operations: Some(0),
                ..GraphqlConfig::default()
            },
            GraphqlConfig {
                max_representations: Some(0),
                ..GraphqlConfig::default()
            },
        ] {
            let refused = pinned
                .check()
                .expect_err("off in code is `None`")
                .to_string();
            assert!(refused.contains("`None` turns it off"), "{refused}");
        }
    }

    /// Each count, its key, what is past its ceiling and that ceiling: refused
    /// past it from the environment and in code, naming the variable and the
    /// range and never the value it was given.
    #[test]
    fn every_count_is_held_to_its_range_from_either_side() {
        type Pin = fn(usize) -> GraphqlConfig;
        let cases: [(&str, &str, &str, Pin); 6] = [
            ("MAX_DEPTH", "34", "33", |count| GraphqlConfig {
                max_depth: Some(count),
                ..GraphqlConfig::default()
            }),
            ("MAX_COMPLEXITY", "10000001", "10000000", |count| {
                GraphqlConfig {
                    max_complexity: Some(count),
                    ..GraphqlConfig::default()
                }
            }),
            ("MAX_SOCKET_OPERATIONS", "10001", "10000", |count| {
                GraphqlConfig {
                    max_socket_operations: Some(count),
                    ..GraphqlConfig::default()
                }
            }),
            ("MAX_REPRESENTATIONS", "100001", "100000", |count| {
                GraphqlConfig {
                    max_representations: Some(count),
                    ..GraphqlConfig::default()
                }
            }),
            ("MAX_BATCH_SIZE", "1025", "1024", |count| GraphqlConfig {
                max_batch_size: count,
                ..GraphqlConfig::default()
            }),
            ("MAX_MESSAGE_BYTES", "1073741825", "1073741824", |count| {
                GraphqlConfig {
                    max_message_bytes: count,
                    ..GraphqlConfig::default()
                }
            }),
        ];
        for (key, past, ceiling, pin) in cases {
            let var = nest_rs_config::var_name("graphql", key);
            let refused = GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", [(key, past)]),
                GraphqlConfig::default(),
            )
            .expect_err("past the ceiling")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains(&format!("must be at most {ceiling}")),
                "{refused}"
            );
            assert!(!refused.contains(past), "{refused}");
            let top = GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", [(key, ceiling)]),
                GraphqlConfig::default(),
            );
            assert!(top.is_ok(), "{key}: the ceiling itself is accepted");
            let pinned = pin(past.parse().expect("a count"))
                .check()
                .expect_err("a pin past the ceiling")
                .to_string();
            assert!(
                pinned.contains(&var)
                    && pinned.contains("set in code")
                    && pinned.contains(&format!("above the {ceiling} it must be at most")),
                "{pinned}"
            );
        }
    }

    /// The two with no off: `0` is below the floor, where the other four read
    /// it as off.
    #[test]
    fn a_count_with_no_off_refuses_zero_from_either_side() {
        let pins = [
            (
                "MAX_BATCH_SIZE",
                GraphqlConfig {
                    max_batch_size: 0,
                    ..Default::default()
                },
            ),
            (
                "MAX_MESSAGE_BYTES",
                GraphqlConfig {
                    max_message_bytes: 0,
                    ..Default::default()
                },
            ),
        ];
        for (key, pin) in pins {
            let read = GraphqlConfig::from_env(
                &ConfigService::with_vars("graphql", [(key, "0")]),
                GraphqlConfig::default(),
            )
            .expect_err("read below the floor")
            .to_string();
            assert!(read.contains("must be at least 1 — "), "{read}");
            let pinned = pin.check().expect_err("pinned below the floor").to_string();
            assert!(
                pinned.contains("below the 1 it must be at least — "),
                "{pinned}"
            );
            for refused in [read, pinned] {
                assert!(
                    refused.contains(&nest_rs_config::var_name("graphql", key)),
                    "{refused}"
                );
                assert!(!refused.contains("turn it off"), "{refused}");
            }
        }
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
        assert_eq!(cfg.playground, Surface::Open);
        assert_eq!(cfg.emit_path, PathBuf::from("./schema-out.graphql"));
        assert_eq!(cfg.emit, Surface::Open);
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
        assert!(d.check().is_ok());
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
        assert!(zero.check().is_err(), "a zero cap would refuse every frame");
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
