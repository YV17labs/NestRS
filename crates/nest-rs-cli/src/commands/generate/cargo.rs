//! Dependency auto-wiring for generated code: [`Transform`]s splice what a
//! generator's output needs into the root `[workspace.dependencies]` and the
//! `crates/features` manifest, idempotently.

use toml_edit::{DocumentMut, Item, Value};

use crate::naming::Transport;
use crate::scaffold::Transform;
use crate::version::framework_req;

/// One dependency the generator may need to introduce.
pub(crate) struct Dep {
    name: &'static str,
    /// TOML value placed in `[workspace.dependencies]` when absent. **Ignored
    /// for the umbrella** — see [`nest_rs`].
    workspace_value: &'static str,
    /// Features to enable in the `features` crate (`[]` ⇒ `{ workspace = true }`).
    features: &'static [&'static str],
}

/// A capability of the umbrella; its version tracks the CLI's own release line
/// ([`framework_req`]).
const fn nest_rs(features: &'static [&'static str]) -> Dep {
    Dep {
        name: "nest-rs",
        workspace_value: "",
        features,
    }
}

impl Dep {
    /// The `[workspace.dependencies]` value to insert. The umbrella pins the
    /// lockstep framework requirement; everything else uses its literal.
    fn workspace_item(&self) -> Item {
        if self.name.starts_with("nest-rs") {
            parse_value(&format!("\"{}\"", framework_req()))
        } else {
            parse_value(self.workspace_value)
        }
    }
}

pub(super) const SEAORM: Dep = nest_rs(&["seaorm", "http"]);
// `#[expose]` and `#[wire_enum]` ride `seaorm`: the `nest-rs-resource` and
// `nest-rs-seaorm` expansions name each other's crate, so they are one feature.
pub(super) const RESOURCE: Dep = nest_rs(&["seaorm"]);
pub(super) const GRAPHQL: Dep = nest_rs(&["graphql"]);
pub(super) const WS: Dep = nest_rs(&["ws"]);
pub(super) const SCHEDULE: Dep = nest_rs(&["schedule"]);
pub(super) const EVENTS: Dep = nest_rs(&["events"]);
// `redis-queue` implies `redis` and `queue`: the connection (`RedisModule`),
// the queue's binding over it (`RedisQueueModule`) and the port arrive
// together; `redis` alone is the connection.
pub(super) const REDIS_QUEUE: Dep = nest_rs(&["redis-queue"]);
pub(super) const MCP: Dep = nest_rs(&["mcp"]);
pub(super) const AUTHN: Dep = nest_rs(&["authn"]);
pub(super) const AUTHZ: Dep = nest_rs(&["authz", "http"]);
// Mirrors the feature set `nest-rs-seaorm` itself resolves.
const SEA_ORM: Dep = Dep {
    name: "sea-orm",
    workspace_value: "{ version = \"2.0\", default-features = false, features = [\"sqlx-postgres\", \"runtime-tokio\", \"macros\", \"with-uuid\", \"with-chrono\"] }",
    features: &[],
};
const UUID: Dep = Dep {
    name: "uuid",
    workspace_value: "{ version = \"1.27\", features = [\"v7\", \"serde\"] }",
    features: &[],
};
const SERDE: Dep = Dep {
    name: "serde",
    workspace_value: "{ version = \"1.0\", features = [\"derive\"] }",
    features: &[],
};
const ASYNC_GRAPHQL: Dep = Dep {
    name: "async-graphql",
    workspace_value: "{ version = \"7.2\", features = [\"dataloader\"] }",
    features: &[],
};
// The `ws` skeleton writes `tracing::warn!`; `queue`/`schedule` return
// `anyhow::Result`. No-ops on a tree `nestrs new` wrote.
const TRACING: Dep = Dep {
    name: "tracing",
    workspace_value: "\"0.1\"",
    features: &[],
};
const ANYHOW: Dep = Dep {
    name: "anyhow",
    workspace_value: "\"1.0\"",
    features: &[],
};
const SEA_ORM_MIGRATION: Dep = Dep {
    name: "sea-orm-migration",
    workspace_value: "{ version = \"2.0\", features = [\"sqlx-postgres\", \"runtime-tokio\"] }",
    features: &[],
};
const TRACING_SUBSCRIBER: Dep = Dep {
    name: "tracing-subscriber",
    workspace_value: "{ version = \"0.3\", features = [\"env-filter\"] }",
    features: &[],
};

/// The crates a resource port (DB-backed CRUD + HTTP) needs; `authz` because
/// `#[crud]` emits `Authorize<…>` parameters.
pub(super) fn resource_deps() -> Vec<&'static Dep> {
    vec![&SEAORM, &RESOURCE, &AUTHZ, &SEA_ORM, &SERDE]
}

/// The crates one `#[expose]`d entity needs — [`resource_deps`] without `authz`.
pub(super) fn entity_deps() -> Vec<&'static Dep> {
    vec![&SEAORM, &RESOURCE, &SEA_ORM, &SERDE]
}

/// The crates the authn/authz adapter (`g auth`) needs.
pub(super) fn auth_deps() -> Vec<&'static Dep> {
    // `RESOURCE` for `#[wire_enum]` on `Role`.
    vec![&AUTHN, &AUTHZ, &RESOURCE, &SERDE, &UUID]
}

/// The crates the `migrations` + `seed` bootstrap crates need — the union of
/// what `templates::migration`'s two manifests declare `workspace = true`; keep
/// the two in step. No `async-trait`: `sea_orm_migration::prelude` re-exports it.
pub(super) fn migrations_deps() -> Vec<&'static Dep> {
    vec![
        &SEAORM,
        &SEA_ORM,
        &SEA_ORM_MIGRATION,
        &ANYHOW,
        &TRACING_SUBSCRIBER,
    ]
}

/// The crates an adapter for `transport` needs on top of the port.
pub(super) fn adapter_deps(transport: Transport) -> Vec<&'static Dep> {
    match transport {
        Transport::Http => vec![],
        Transport::Graphql => vec![&GRAPHQL, &ASYNC_GRAPHQL],
        Transport::Ws => vec![&WS, &TRACING],
        // `SERDE`: the payload carries plain derives, not `#[input]`, whose
        // `deny_unknown_fields` would dead-letter a newer producer's jobs.
        Transport::Queue => vec![&REDIS_QUEUE, &ANYHOW, &SERDE],
        Transport::Schedule => vec![&SCHEDULE, &ANYHOW],
        Transport::Mcp => vec![&MCP],
        Transport::Events => vec![&EVENTS],
    }
}

/// What an **app crate** needs to depend on to name the transport's root
/// module; empty where every scaffolded app crate already carries it.
pub(super) fn app_host_deps(transport: Transport) -> Vec<&'static Dep> {
    match transport {
        Transport::Http | Transport::Ws | Transport::Mcp => vec![],
        Transport::Graphql => vec![&GRAPHQL],
        Transport::Queue => vec![&REDIS_QUEUE],
        Transport::Schedule => vec![&SCHEDULE],
        Transport::Events => vec![&EVENTS],
    }
}

/// What exposing an entity over GraphQL needs: `#[expose(graphql)]` derives the
/// async-graphql object through `nest_rs_resource::graphql`, which that crate
/// only compiles under its own `graphql` feature.
pub(super) fn graphql_port_deps() -> Vec<&'static Dep> {
    vec![&RESOURCE, &GRAPHQL]
}

/// Edit the root manifest: add any missing `[workspace.dependencies]` entries.
pub(super) fn ensure_workspace_deps(deps: Vec<&'static Dep>) -> Transform {
    ensure_deps(deps, &["workspace", "dependencies"], Dep::workspace_item)
}

/// Edit the `features` manifest: add any missing `[dependencies]` entries as
/// `{ workspace = true, features = [...] }`.
pub(super) fn ensure_features_deps(deps: Vec<&'static Dep>) -> Transform {
    ensure_deps(deps, &["dependencies"], |_| Item::Value(workspace_value()))
}

/// Insert what is absent, then enable any feature the entry is missing: a
/// capability is a feature of the single `nest-rs` entry, already present or not.
fn ensure_deps(
    deps: Vec<&'static Dep>,
    path: &'static [&'static str],
    missing: fn(&Dep) -> Item,
) -> Transform {
    Box::new(move |content: &str| {
        let mut doc = content.parse::<DocumentMut>().ok()?;
        let mut table = doc.as_table_mut() as &mut dyn toml_edit::TableLike;
        for key in path {
            table = table
                .entry(key)
                .or_insert(toml_edit::table())
                .as_table_like_mut()?;
        }
        let mut changed = false;
        for dep in &deps {
            if table.get(dep.name).is_none() {
                table.insert(dep.name, missing(dep));
                changed = true;
            }
            changed |= enable_features(table.get_mut(dep.name)?, dep.features);
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

/// A bare `{ workspace = true }` entry; [`enable_features`] adds the features.
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
        let src = "[workspace.dependencies]\nanyhow = \"1\"\n";
        let t = ensure_workspace_deps(vec![&SEAORM]);
        let out = t(src).expect("adds nest-rs");
        assert!(out.contains("nest-rs"), "{out}");
        assert!(out.contains("seaorm"), "{out}");
        assert!(ensure_workspace_deps(vec![&SEAORM])(&out).is_none());
    }

    #[test]
    fn ensures_features_dep_with_features() {
        let src = "[dependencies]\nanyhow.workspace = true\n";
        let out = ensure_features_deps(vec![&SEAORM])(src).expect("adds dep");
        assert!(out.contains("nest-rs"));
        assert!(out.contains("workspace = true"));
        assert!(out.contains("\"http\""));
    }

    // `g graphql` on a workspace already depending on `nest-rs`: only its feature
    // is missing.
    #[test]
    fn enables_a_missing_feature_on_a_dependency_already_declared() {
        let src = "[dependencies]\nnest-rs.workspace = true\n";
        let out = ensure_features_deps(vec![&GRAPHQL])(src).expect("enables graphql");
        assert!(out.contains("graphql"), "{out}");
        assert!(
            ensure_features_deps(vec![&GRAPHQL])(&out).is_none(),
            "a second run is a no-op: {out}",
        );
        let doc = out.parse::<DocumentMut>().expect("still valid TOML");
        assert_eq!(
            doc["dependencies"]["nest-rs"]["workspace"].as_bool(),
            Some(true),
            "the existing keys survive: {out}",
        );
    }

    #[test]
    fn enabling_a_feature_keeps_the_ones_already_listed() {
        let src = "[dependencies]\nnest-rs = { workspace = true, features = [\"http\"] }\n";
        let out = ensure_features_deps(vec![&SEAORM, &GRAPHQL])(src).expect("adds graphql");
        assert!(
            out.contains("\"http\"") && out.contains("\"graphql\""),
            "{out}"
        );
    }

    /// Every crate a generated skeleton *names* is in that transport's dependency
    /// list, derived from the template text.
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
                // The queue payload rides at the port, but `g queue` writes it.
                let extra = if transport == Transport::Queue {
                    crate::templates::adapter::QUEUE_COMMAND
                } else {
                    ""
                };
                let src = format!("{handler}{module}{extra}");
                for (token, krate) in NAMED {
                    if !src.contains(token) {
                        continue;
                    }
                    // Or imported through the framework's re-export (`use nest_rs::mcp::rmcp;`).
                    let reexported = src.contains(&format!("::{};", krate.replace('-', "_")));
                    assert!(
                        declared.contains(krate) || reexported,
                        "the {} skeleton writes `{token}` but `nestrs g {}` neither adds \
                         `{krate}` nor imports it through the framework — the first \
                         `cargo check` after generating fails",
                        transport.folder(),
                        transport.folder(),
                    );
                }
            }
        }
    }

    /// Render an adapter's handler the way the generator does — template plus
    /// the `crud_vars` that supply the differing handler.
    fn rendered_handler(transport: Transport, crud_port: bool) -> String {
        let names = crate::naming::Names::parse("posts");
        let (handler, _) = crate::commands::generate::adapter::templates_for(transport, crud_port);
        let mut r = crate::scaffold::Renderer::new(&names)
            .with("handler", names.handler_for(transport))
            .with("handler_mod", transport.handler_mod())
            .with("tmodule", names.module_for(transport));
        for (key, value) in crate::templates::crud::crud_vars(crud_port, transport) {
            r = r.with(key, value);
        }
        r.render(handler)
    }

    /// `count()` exists only on the `g feature` service, so no CRUD skeleton may
    /// call it (rustc would blame `Iterator::count`).
    #[test]
    fn no_crud_port_skeleton_calls_the_plain_features_count() {
        for transport in Transport::ALL {
            let rendered = rendered_handler(transport, true);
            assert!(
                !rendered.contains("svc.count()"),
                "the {} adapter renders `svc.count()` over a resource port, which a \
                 CrudService does not have:
{rendered}",
                transport.folder(),
            );
        }
    }

    /// The plain-feature skeletons keep delegating.
    #[test]
    fn the_plain_feature_skeletons_still_delegate_to_the_port() {
        for transport in [Transport::Http, Transport::Graphql, Transport::Ws] {
            let rendered = rendered_handler(transport, false);
            assert!(
                rendered.contains("svc.count()"),
                "the {} adapter over a `g feature` port should still show the delegation:
\
                 {rendered}",
                transport.folder(),
            );
        }
    }

    #[test]
    fn a_version_pinned_dependency_is_widened_to_carry_features() {
        let src = "[dependencies]\nnest-rs = \"1.1\"\n";
        let out = ensure_features_deps(vec![&GRAPHQL])(src).expect("widens the entry");
        let doc = out.parse::<DocumentMut>().expect("still valid TOML");
        assert_eq!(
            doc["dependencies"]["nest-rs"]["version"].as_str(),
            Some("1.1"),
            "the pin survives: {out}",
        );
        assert!(out.contains("graphql"), "{out}");
    }
}
