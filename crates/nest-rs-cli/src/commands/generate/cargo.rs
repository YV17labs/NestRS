//! Dependency auto-wiring for generated code.
//!
//! Adding a transport adapter to a fresh workspace usually needs a crate the
//! starter `Cargo.toml` doesn't carry yet (a resource needs `nest-rs-seaorm`,
//! a GraphQL adapter needs `async-graphql`, …). These [`Transform`]s splice
//! the missing entries into the root `[workspace.dependencies]` and the
//! `crates/features` manifest — idempotently, so an already-equipped workspace
//! (the nestrs repo itself) is a no-op.

use toml_edit::{DocumentMut, Item, Value};

use crate::naming::Transport;
use crate::scaffold::Transform;
use crate::version::framework_req;

/// One dependency the generator may need to introduce.
pub(crate) struct Dep {
    name: &'static str,
    /// TOML value placed in `[workspace.dependencies]` when absent. **Ignored
    /// for `nest-rs-*` crates** — their version tracks the CLI's own release
    /// line (see [`framework_req`]), so leave it `""` for those.
    workspace_value: &'static str,
    /// Features to enable in the `features` crate (`[]` ⇒ `{ workspace = true }`).
    features: &'static [&'static str],
}

impl Dep {
    /// The `[workspace.dependencies]` value to insert. `nest-rs-*` crates pin
    /// the lockstep framework requirement; everything else uses its literal.
    fn workspace_item(&self) -> Item {
        if self.name.starts_with("nest-rs-") {
            parse_value(&format!("\"{}\"", framework_req()))
        } else {
            parse_value(self.workspace_value)
        }
    }
}

// `nest-rs-*` crates: `workspace_value` is unused — `workspace_item` derives
// the version from the CLI's own release line (`framework_req`).
const SEAORM: Dep = Dep {
    name: "nest-rs-seaorm",
    workspace_value: "",
    features: &["http"],
};
const RESOURCE: Dep = Dep {
    name: "nest-rs-resource",
    workspace_value: "",
    features: &[],
};
const GRAPHQL: Dep = Dep {
    name: "nest-rs-graphql",
    workspace_value: "",
    features: &[],
};
const WS: Dep = Dep {
    name: "nest-rs-ws",
    workspace_value: "",
    features: &[],
};
const QUEUE: Dep = Dep {
    name: "nest-rs-queue",
    workspace_value: "",
    features: &[],
};
const SCHEDULE: Dep = Dep {
    name: "nest-rs-schedule",
    workspace_value: "",
    features: &[],
};
const MCP: Dep = Dep {
    name: "nest-rs-mcp",
    workspace_value: "",
    features: &[],
};
const AUTHN: Dep = Dep {
    name: "nest-rs-authn",
    workspace_value: "",
    features: &[],
};
const AUTHZ: Dep = Dep {
    name: "nest-rs-authz",
    workspace_value: "",
    features: &["http"],
};
// The GraphQL trio. `#[resolver]` expands to `nest_rs_guards::{GraphqlChainCell,
// GraphqlChainSources, run_layered_graphql_chain}`, which live behind that
// crate's `graphql` feature — and `nest-rs-guards` is already a dependency of
// every scaffolded `features` crate, so it is the *feature*, not the entry,
// that has to be added. The other two carry the authz bridge
// (`GraphqlAbilityBridge`) and its loader scope (`LoaderScope`); both keep
// `http` because the bridge is written against the HTTP guards.
const GUARDS_GRAPHQL: Dep = Dep {
    name: "nest-rs-guards",
    workspace_value: "",
    features: &["graphql"],
};
const AUTHZ_GRAPHQL: Dep = Dep {
    name: "nest-rs-authz",
    workspace_value: "",
    features: &["http", "graphql"],
};
const SEAORM_GRAPHQL: Dep = Dep {
    name: "nest-rs-seaorm",
    workspace_value: "",
    features: &["http", "graphql"],
};
const RESOURCE_GRAPHQL: Dep = Dep {
    name: "nest-rs-resource",
    workspace_value: "",
    features: &["graphql"],
};
// `#[messages]` expands to `nest_rs_guards::GuardAsWsMessageCheck`, which that
// crate gates behind `ws`. Same shape as `GUARDS_GRAPHQL`: every scaffolded
// workspace already depends on `nest-rs-guards`, so it is the feature that has
// to be turned on. Leaving it off compiles only by feature unification with a
// dev-dependency — `cargo check -p features` fails while `--workspace` passes.
const GUARDS_WS: Dep = Dep {
    name: "nest-rs-guards",
    workspace_value: "",
    features: &["ws"],
};
// Mirrors the feature set `nest-rs-seaorm` itself resolves — a divergent list
// (or a release-candidate floor) would be a manifest the user inherits and has
// to un-learn later.
const SEA_ORM: Dep = Dep {
    name: "sea-orm",
    workspace_value: "{ version = \"2.0\", default-features = false, features = [\"sqlx-postgres\", \"runtime-tokio-rustls\", \"macros\", \"with-uuid\", \"with-chrono\"] }",
    features: &[],
};
const SERDE: Dep = Dep {
    name: "serde",
    workspace_value: "{ version = \"1\", features = [\"derive\"] }",
    features: &[],
};
const UUID: Dep = Dep {
    name: "uuid",
    workspace_value: "{ version = \"1\", features = [\"v7\", \"serde\"] }",
    features: &[],
};
const VALIDATOR: Dep = Dep {
    name: "validator",
    workspace_value: "{ version = \"0.20\", features = [\"derive\"] }",
    features: &[],
};
const ASYNC_GRAPHQL: Dep = Dep {
    name: "async-graphql",
    workspace_value: "{ version = \"7\", features = [\"dataloader\"] }",
    features: &[],
};
// `nest-rs-mcp`'s macros expand to bare `rmcp::` paths, so the user's manifest
// genuinely needs the crate — which makes this line a *contract* with what
// `nest-rs-mcp` itself compiled against. A different major puts two
// `ServerHandler` traits in one graph and every `#[tool_handler]` method
// mismatches. `rmcp_pin_matches_the_frameworks_own` pins the two together.
const RMCP: Dep = Dep {
    name: "rmcp",
    workspace_value: "{ version = \"2.2\", features = [\"server\", \"macros\", \"transport-streamable-http-server\"] }",
    features: &[],
};
// Every adapter skeleton that logs (`queue`, `schedule`, `ws`) writes a
// `tracing::` call in the handler body, and a workspace scaffolded by
// `nestrs new` carries no `tracing` in its features crate — the first generated
// adapter is usually the first code to reach for it.
const TRACING: Dep = Dep {
    name: "tracing",
    workspace_value: "\"0.1\"",
    features: &[],
};
const ANYHOW: Dep = Dep {
    name: "anyhow",
    workspace_value: "\"1\"",
    features: &[],
};
const SCHEMARS: Dep = Dep {
    name: "schemars",
    workspace_value: "{ version = \"1\", features = [\"uuid1\"] }",
    features: &[],
};
const CHRONO: Dep = Dep {
    name: "chrono",
    workspace_value: "{ version = \"0.4\", features = [\"serde\"] }",
    features: &[],
};
const SEA_ORM_MIGRATION: Dep = Dep {
    name: "sea-orm-migration",
    workspace_value: "{ version = \"2.0\", features = [\"sqlx-postgres\", \"runtime-tokio-rustls\"] }",
    features: &[],
};
const TRACING_SUBSCRIBER: Dep = Dep {
    name: "tracing-subscriber",
    workspace_value: "{ version = \"0.3\", features = [\"env-filter\"] }",
    features: &[],
};
const TOKIO: Dep = Dep {
    name: "tokio",
    workspace_value: "{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }",
    features: &[],
};

/// The crates a resource port (DB-backed CRUD + HTTP) needs.
///
/// `schemars` and `nest-rs-authz` are call-site deps of the decorators, not of
/// the developer's own code: `#[expose]` derives `::schemars::JsonSchema` and
/// `#[crud]` emits `::nest_rs_authz::http::Authorize<…>` parameters. Omitting
/// either turns the very first `cargo check` after `g resource` into a wall of
/// macro-expansion errors.
pub fn resource_deps() -> Vec<&'static Dep> {
    vec![
        &SEAORM, &RESOURCE, &AUTHZ, &SEA_ORM, &SERDE, &UUID, &VALIDATOR, &SCHEMARS, &CHRONO,
    ]
}

/// The crates the authn/authz adapter (`g auth`) needs.
pub fn auth_deps() -> Vec<&'static Dep> {
    vec![&AUTHN, &AUTHZ, &SERDE, &UUID]
}

/// The crates the `migrations` + `seed` bootstrap crates need — the union of
/// what `templates::migration`'s two manifests declare `workspace = true`. A
/// name missing here is a generated crate whose own `Cargo.toml` names a
/// workspace dependency the root does not define, so keep the two in step.
/// (`async-trait` is deliberately absent: the migration template writes
/// `#[async_trait::async_trait]`, which `sea_orm_migration::prelude` re-exports
/// — the demo's migrations crate does not depend on it either.)
pub fn migrations_deps() -> Vec<&'static Dep> {
    vec![
        &SEAORM,
        &SEA_ORM,
        &SEA_ORM_MIGRATION,
        &ANYHOW,
        &TOKIO,
        &TRACING_SUBSCRIBER,
    ]
}

/// The crates an adapter for `transport` needs on top of the port.
pub fn adapter_deps(transport: Transport) -> Vec<&'static Dep> {
    match transport {
        Transport::Http => vec![],
        Transport::Graphql => vec![&GRAPHQL, &ASYNC_GRAPHQL, &GUARDS_GRAPHQL],
        // `serde` is not optional the moment a handler takes a typed payload —
        // the shape `/websockets/messages/` presents as the normal case, with
        // `#[derive(serde::Deserialize)]` on a DTO. `nest_rs_ws` re-exports only
        // `serde_json`, so without this the first typed handler fails to compile
        // and the page's install list was wrong by omission.
        Transport::Ws => vec![&WS, &GUARDS_WS, &SERDE, &TRACING],
        Transport::Queue => vec![&QUEUE, &SERDE, &ANYHOW, &TRACING],
        Transport::Schedule => vec![&SCHEDULE, &ANYHOW, &TRACING],
        Transport::Mcp => vec![&MCP, &RMCP],
    }
}

/// The crates the GraphQL authz adapter (`authz/graphql/`) needs — the
/// per-operation bridge and the dataloader scope.
pub fn graphql_authz_deps() -> Vec<&'static Dep> {
    vec![&AUTHZ_GRAPHQL, &SEAORM_GRAPHQL, &GRAPHQL]
}

/// What exposing an entity over GraphQL needs: `#[expose(graphql)]` derives the
/// async-graphql object through `nest_rs_resource::graphql`, which that crate
/// only compiles under its own `graphql` feature.
pub fn graphql_port_deps() -> Vec<&'static Dep> {
    vec![&RESOURCE_GRAPHQL]
}

/// Edit the root manifest: add any missing `[workspace.dependencies]` entries.
pub fn ensure_workspace_deps(deps: Vec<&'static Dep>) -> Transform {
    Box::new(move |content: &str| {
        let mut doc = content.parse::<DocumentMut>().ok()?;
        let table = doc["workspace"]["dependencies"]
            .or_insert(toml_edit::table())
            .as_table_mut()?;
        let mut changed = false;
        for dep in &deps {
            if table.get(dep.name).is_none() {
                table.insert(dep.name, dep.workspace_item());
                changed = true;
            }
        }
        changed.then(|| doc.to_string())
    })
}

/// Edit the `features` manifest: add any missing `[dependencies]` entries as
/// `{ workspace = true, features = [...] }` — and, for an entry that is already
/// there, enable any feature it is missing. The second half is what a generator
/// bolting a transport onto a crate the starter manifest already depends on
/// needs: `nest-rs-guards` ships with every workspace, so `g graphql` can only
/// reach `nest_rs_guards::run_layered_graphql_chain` by turning its `graphql`
/// feature on.
pub fn ensure_features_deps(deps: Vec<&'static Dep>) -> Transform {
    Box::new(move |content: &str| {
        let mut doc = content.parse::<DocumentMut>().ok()?;
        let table = doc["dependencies"]
            .or_insert(toml_edit::table())
            .as_table_mut()?;
        let mut changed = false;
        for dep in &deps {
            if table.get(dep.name).is_none() {
                table.insert(dep.name, Item::Value(workspace_value()));
                changed = true;
            }
            let entry = table.get_mut(dep.name)?;
            changed |= enable_features(entry, dep.features);
        }
        changed.then(|| doc.to_string())
    })
}

/// Turn on every feature `wanted` that this dependency entry does not already
/// enable, in place. Handles the three shapes a manifest writes: the dotted
/// `dep.workspace = true`, the inline `dep = { workspace = true }`, and the
/// bare `dep = "1"` (widened to a table so the list has somewhere to live).
fn enable_features(entry: &mut Item, wanted: &[&str]) -> bool {
    if wanted.is_empty() {
        return false;
    }
    if let Some(version) = entry.as_str() {
        let mut table = toml_edit::InlineTable::new();
        table.insert("version", Value::from(version));
        *entry = Item::Value(Value::InlineTable(table));
    }
    let Some(table) = entry.as_table_like_mut() else {
        return false;
    };
    let mut features = table
        .get("features")
        .and_then(Item::as_array)
        .cloned()
        .unwrap_or_default();
    let mut changed = false;
    for feature in wanted {
        if !features.iter().any(|f| f.as_str() == Some(*feature)) {
            features.push(*feature);
            changed = true;
        }
    }
    if changed {
        table.insert("features", Item::Value(Value::Array(features)));
        // Re-space the entry we just widened, so the manifest the developer
        // inherits reads as if it had been written by hand.
        if let Some(inline) = entry.as_value_mut().and_then(Value::as_inline_table_mut) {
            inline.fmt();
        }
    }
    changed
}

fn parse_value(raw: &str) -> Item {
    format!("x = {raw}\n")
        .parse::<DocumentMut>()
        .ok()
        .and_then(|frag| frag.get("x").cloned())
        .unwrap_or_else(|| Item::Value(Value::from(raw)))
}

/// A bare `{ workspace = true }` entry — [`enable_features`] then adds whatever
/// the dependency needs on top, so the feature list is built in one place
/// whether the entry is new or already there.
fn workspace_value() -> Value {
    let mut table = toml_edit::InlineTable::new();
    table.insert("workspace", Value::from(true));
    Value::InlineTable(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensures_workspace_dep_idempotently() {
        let src = "[workspace.dependencies]\nnest-rs-core = \"0.1\"\n";
        let t = ensure_workspace_deps(vec![&SEAORM]);
        let out = t(src).expect("adds nest-rs-seaorm");
        // The pin tracks the CLI's own release line, not a hard-coded literal.
        assert!(out.contains(&format!("nest-rs-seaorm = \"{}\"", framework_req())));
        // already present → no-op
        assert!(ensure_workspace_deps(vec![&SEAORM])(&out).is_none());
    }

    #[test]
    fn ensures_features_dep_with_features() {
        let src = "[dependencies]\nnest-rs-core.workspace = true\n";
        let out = ensure_features_deps(vec![&SEAORM])(src).expect("adds dep");
        assert!(out.contains("nest-rs-seaorm"));
        assert!(out.contains("workspace = true"));
        assert!(out.contains("\"http\""));
    }

    // The `g graphql` case: the crate is already a dependency (every scaffolded
    // workspace carries `nest-rs-guards`), so only its feature is missing —
    // and without it `#[resolver]` expands to names that do not exist.
    #[test]
    fn enables_a_missing_feature_on_a_dependency_already_declared() {
        let src = "[dependencies]\nnest-rs-guards.workspace = true\n";
        let out = ensure_features_deps(vec![&GUARDS_GRAPHQL])(src).expect("enables graphql");
        assert!(out.contains("graphql"), "{out}");
        assert!(
            ensure_features_deps(vec![&GUARDS_GRAPHQL])(&out).is_none(),
            "a second run is a no-op: {out}",
        );
        let doc = out.parse::<DocumentMut>().expect("still valid TOML");
        assert_eq!(
            doc["dependencies"]["nest-rs-guards"]["workspace"].as_bool(),
            Some(true),
            "the existing keys survive: {out}",
        );
    }

    #[test]
    fn enabling_a_feature_keeps_the_ones_already_listed() {
        let src = "[dependencies]\nnest-rs-seaorm = { workspace = true, features = [\"http\"] }\n";
        let out = ensure_features_deps(vec![&SEAORM_GRAPHQL])(src).expect("adds graphql");
        assert!(
            out.contains("\"http\"") && out.contains("\"graphql\""),
            "{out}"
        );
    }

    /// Every crate a generated skeleton *names* has to be in that transport's
    /// dependency list. Derived from the template text rather than from a
    /// hand-kept list, so a skeleton that starts logging (or starts returning
    /// `anyhow::Result`) drags its dependency along on the same commit.
    ///
    /// The class this closes: three generators wrote `tracing::info!` into the
    /// handler body while adding only their own `nest-rs-*` crate, so the first
    /// `cargo check` after `nestrs g queue` was `cannot find module or crate
    /// `tracing``.
    #[test]
    fn a_skeleton_that_names_a_crate_declares_it() {
        // (token appearing in a skeleton, crate that must then be a dependency)
        const NAMED: &[(&str, &str)] = &[
            ("tracing::", "tracing"),
            ("anyhow::", "anyhow"),
            ("use anyhow", "anyhow"),
            ("serde::", "serde"),
            ("use serde", "serde"),
            ("async_graphql::", "async-graphql"),
            ("use async_graphql", "async-graphql"),
            ("rmcp::", "rmcp"),
        ];
        for transport in Transport::ALL {
            let declared: Vec<&str> = adapter_deps(transport).iter().map(|d| d.name).collect();
            for crud_port in [false, true] {
                let (handler, module) =
                    crate::commands::generate::adapter::templates_for(transport, crud_port);
                // The queue payload rides at the port, but `g queue` is what
                // writes it — so it counts against the same dependency list.
                let extra = if transport == Transport::Queue {
                    crate::templates::adapter::QUEUE_COMMAND
                } else {
                    ""
                };
                let src = format!("{handler}{module}{extra}");
                for (token, krate) in NAMED {
                    if src.contains(token) {
                        assert!(
                            declared.contains(krate),
                            "the {} skeleton writes `{token}` but `nestrs g {}` does not add \
                             `{krate}` — the first `cargo check` after generating fails",
                            transport.folder(),
                            transport.folder(),
                        );
                    }
                }
            }
        }
    }

    /// `rmcp` is the one third-party crate a generated manifest must pin to the
    /// *same* major the framework compiled against: `#[tool_handler]` expands
    /// against `nest-rs-mcp`'s `ServerHandler` while the user's `impl` resolves
    /// against theirs, so two majors in one graph mismatch every method.
    ///
    /// Read from the workspace manifest rather than restated, so bumping the
    /// framework's `rmcp` fails here until the generator follows.
    #[test]
    fn the_rmcp_pin_matches_the_frameworks_own() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Cargo.toml")
            .canonicalize()
            .expect("the framework workspace manifest");
        let doc = std::fs::read_to_string(&root)
            .expect("readable workspace manifest")
            .parse::<DocumentMut>()
            .expect("valid TOML");
        let ours = doc["workspace"]["dependencies"]["rmcp"].to_string();
        let ours = ours.trim();
        let generated = RMCP.workspace_value;
        assert_eq!(
            normalize(generated),
            normalize(ours),
            "`nestrs g mcp` writes {generated} while the framework builds against {ours} — \
             two rmcp majors in one graph make every `#[tool_handler]` method mismatch",
        );
    }

    /// Whitespace-insensitive compare of two inline-table literals.
    fn normalize(raw: &str) -> String {
        raw.chars().filter(|c| !c.is_whitespace()).collect()
    }

    // A hand-rolled manifest may pin a version literally; the feature list then
    // has nowhere to go until the entry is widened into a table.
    #[test]
    fn a_version_pinned_dependency_is_widened_to_carry_features() {
        let src = "[dependencies]\nnest-rs-guards = \"1.1\"\n";
        let out = ensure_features_deps(vec![&GUARDS_GRAPHQL])(src).expect("widens the entry");
        let doc = out.parse::<DocumentMut>().expect("still valid TOML");
        assert_eq!(
            doc["dependencies"]["nest-rs-guards"]["version"].as_str(),
            Some("1.1"),
            "the pin survives: {out}",
        );
        assert!(out.contains("graphql"), "{out}");
    }
}
