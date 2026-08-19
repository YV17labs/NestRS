//! [`GraphqlConfig`] — loaded from `NESTRS_GRAPHQL__*`. Every field defaults
//! production-safe (playground off, SDL emit off, depth/complexity limits on,
//! introspection disabled); an `.env.development` opts the tooling in and an
//! app's `module.rs` can pin tighter limits so `app.rs` carries no config
//! literal.

use std::path::PathBuf;
use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};

pub(crate) const DEFAULT_PATH: &str = "/graphql";

/// Four hours — the same default ceiling `NESTRS_WS__MAX_CONNECTION_SECS`
/// carries, because it is the same control on the same kind of socket.
const DEFAULT_MAX_CONNECTION_SECS: u64 = 4 * 60 * 60;

/// A hundred entity references per `_entities` call — a page of parents on the
/// router's side, which is what a query plan turns into one such call.
const DEFAULT_MAX_REPRESENTATIONS: usize = 100;

/// GraphQL endpoint options, settable via `NESTRS_GRAPHQL__*` or pinned through
/// [`GraphqlModule::for_root`](crate::GraphqlModule::for_root). Every field
/// defaults production-safe.
#[config(namespace = "graphql")]
#[derive(Clone, Debug)]
pub struct GraphqlConfig {
    /// Endpoint path. Default `/graphql`.
    pub path: String,
    /// Default `false` (production-safe).
    pub playground: bool,
    /// Where the committed SDL lives. Default `schema.graphql`.
    pub schema_path: PathBuf,
    /// (Re)write `schema_path` from the live schema once at boot. Default
    /// `false`. A write failure is logged, never fatal.
    pub emit_sdl: bool,
    /// Maximum nesting depth of an incoming query AST. Defaults to `Some(15)`
    /// (production-safe); set `None` to disable the check. A sensible value is
    /// in the 10-20 range:
    /// caps recursive bombs (`{ a { a { a { … } } } }`) without rejecting
    /// legitimate nested queries. Cheap to enforce (one AST walk).
    ///
    /// `Some(0)` is rejected at boot: async-graphql checks `depth > limit`
    /// strictly and every field has depth ≥ 1, so `0` would brick every
    /// query. Use `None` to disable.
    #[validate(range(min = 1))]
    pub max_depth: Option<usize>,
    /// Maximum complexity score of an incoming query AST. Defaults to
    /// `Some(2000)` (production-safe); set `None` to disable the check. Score =
    /// 1 per field + per-field overrides emitted
    /// by `#[expose]` on list relations (multiplier on the unbounded fanout).
    /// A sensible production value sits in the 1000-5000 range and should be
    /// tuned from observed legitimate queries.
    ///
    /// `Some(0)` is rejected at boot for the same reason as `max_depth`.
    #[validate(range(min = 1))]
    pub max_complexity: Option<usize>,
    /// Disable GraphQL introspection. Default `true` (production-safe).
    pub disable_introspection: bool,
    /// Maximum number of operations in a single HTTP batch request.
    /// Default `10`.
    #[validate(range(min = 1))]
    pub max_batch_size: usize,
    /// Maximum lifetime of one graphql-ws subscription socket. When it elapses
    /// the server closes the socket, so the peer must re-upgrade — re-running
    /// the operation guard and re-checking token `exp`.
    ///
    /// A **security** control, not a resource knob, and the same one
    /// [`WsConfig::max_connection`] is: a subscription captures its principal
    /// once at the upgrade and replays it for every item it pushes. Without a
    /// ceiling the socket keeps those privileges after expiry, logout or
    /// revocation, for as long as the peer holds it open.
    ///
    /// Read from `NESTRS_GRAPHQL__MAX_CONNECTION_SECS` (whole seconds; `0` ⇒
    /// unlimited); defaults to 4 hours.
    ///
    /// [`WsConfig::max_connection`]: https://docs.rs/nest-rs-ws
    pub max_connection: Option<Duration>,
    /// Serve this schema as an Apollo **subgraph**: the federation directives
    /// are declared, and the emitted SDL is the subgraph form (`@key` present,
    /// `_service` / `_entities` stripped, as the spec requires of an exported
    /// subgraph schema).
    ///
    /// **What it does not do is switch the federation surface on**, and the
    /// distinction is the whole reason the boot enforces it. async-graphql
    /// serves `_service` and `_entities` as soon as any `#[entity]` resolver has
    /// registered its keys, whatever this flag says — so an entity plus
    /// `federation = false` would publish the schema's own SDL while claiming
    /// not to be a subgraph, and the flag would be a comment. Declaring an
    /// entity while this is `false` therefore **fails the boot**, naming the
    /// resolver: an entity resolver *is* the subgraph, and being one is a
    /// deployment decision.
    ///
    /// Default `false`, and it stays an explicit opt-in for a reason a
    /// deployment cannot undo: **`_service` cannot be switched off**.
    /// `disable_introspection` does not cover it — its field sits outside that
    /// gate — so a federated schema publishes its own SDL to anyone who can
    /// reach the endpoint. A subgraph belongs behind a router, on a network the
    /// router is on, and not on the internet.
    ///
    /// Turning it on without an `#[entity]` anywhere is legal and nearly empty:
    /// `_service` answers, `_entities` does not exist, because the keys a router
    /// matches on come from the entity resolvers' own arguments.
    ///
    /// Read from `NESTRS_GRAPHQL__FEDERATION`.
    pub federation: bool,
    /// Maximum number of entity references one `_entities` call may carry.
    ///
    /// The **third** limit on an incoming document and the only one that sees
    /// this number: `max_depth` and `max_complexity` score the document's shape,
    /// `max_batch_size` counts *operations* — and a single `_entities` operation
    /// of depth 2 and trivial complexity carries a list whose length the caller
    /// picks. Each element is a full resolution: the entity body, its posture
    /// gate and mask, and whatever reads it makes, launched concurrently by
    /// async-graphql's `try_join_all`. A hundred thousand references passed
    /// every existing check.
    ///
    /// Over the ceiling is a GraphQL error naming it, never a silent truncation
    /// — a router that quietly received fewer entities than it asked for would
    /// render a page with holes and no way to tell why.
    ///
    /// Read from `NESTRS_GRAPHQL__MAX_REPRESENTATIONS` (`0` ⇒ unlimited);
    /// defaults to 100. Raise it for a router whose parent pages are larger than
    /// that; it costs nothing when no `_entities` reaches the schema.
    ///
    /// **`Some(0)` is unlimited here, where its neighbours refuse it.** This
    /// field carries a sentinel and `max_depth` / `max_complexity` do not — for
    /// them `0` could only mean "reject every query", so the validator refuses it
    /// rather than let it read as *off*. `None` and `Some(0)` therefore mean the
    /// same thing on this one, pinned in code exactly as through the variable.
    pub max_representations: Option<usize>,
    /// Promote the default unreachable-resolver `warn` into a boot failure.
    ///
    /// A `#[resolver]` composes into the schema from a link-time registry, so
    /// listing it in a module's `providers = [...]` is what brings its injected
    /// dependencies under the access contract — and one that is listed nowhere
    /// is filtered out of the schema instead of failing the boot. The default
    /// names it at `warn`; this makes it fatal, for apps where a forgotten
    /// `providers` entry should not reach a deployment.
    ///
    /// Default `false`, because a workspace shipping several binaries over one
    /// feature library legitimately links resolvers a given app does not serve.
    ///
    /// Read from `NESTRS_GRAPHQL__STRICT_RESOLVER_MEMBERSHIP`.
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
            path: env.get("PATH").unwrap_or(d.path),
            playground: env.flag("PLAYGROUND", d.playground)?,
            schema_path: env
                .get("SCHEMA_PATH")
                .map(PathBuf::from)
                .unwrap_or(d.schema_path),
            emit_sdl: env.flag("EMIT_SDL", d.emit_sdl)?,
            max_depth: env.parse("MAX_DEPTH")?.or(d.max_depth),
            max_complexity: env.parse("MAX_COMPLEXITY")?.or(d.max_complexity),
            disable_introspection: env.flag("DISABLE_INTROSPECTION", d.disable_introspection)?,
            max_batch_size: env.parse("MAX_BATCH_SIZE")?.unwrap_or(d.max_batch_size),
            max_connection: env.seconds("MAX_CONNECTION_SECS", d.max_connection)?,
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
        // App code reads this path string indirectly through the module — a
        // rename here breaks every reverse proxy.
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
        // async-graphql's depth/complexity check is strict `>`, and every
        // non-empty selection has depth ≥ 1, so `Some(0)` would reject every
        // query at boot — a footgun the validator must catch.
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
        // Sanity: Some(1) is meaningfully tight but legal; defaults are fine.
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
}
