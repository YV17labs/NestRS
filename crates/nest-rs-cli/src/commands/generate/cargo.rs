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
    /// for the umbrella** — see [`nest_rs`].
    workspace_value: &'static str,
    /// Features to enable in the `features` crate (`[]` ⇒ `{ workspace = true }`).
    features: &'static [&'static str],
}

/// A capability of the umbrella. `workspace_value` is unused for these — the
/// version tracks the CLI's own release line (see [`framework_req`]) — and the
/// name is always the single `nest-rs` entry every generated manifest carries.
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
// `#[expose]` and `#[wire_enum]` ride `seaorm`, which is the one feature that
// activates both `nest-rs-resource` and `nest-rs-seaorm`. They cannot be two
// features: `#[expose]` emits twelve `::nest_rs_seaorm::` paths and `#[crud]`
// emits `::nest_rs_resource::`, so each half's expansion names the other's
// crate — two features that must imply each other, which Cargo rejects as a
// cycle. `cargo add nest-rs --features resource` could not compile `#[expose]`.
pub(super) const RESOURCE: Dep = nest_rs(&["seaorm"]);
pub(super) const GRAPHQL: Dep = nest_rs(&["graphql"]);
pub(super) const WS: Dep = nest_rs(&["ws"]);
pub(super) const SCHEDULE: Dep = nest_rs(&["schedule"]);
// `redis` implies `queue`: the abstractions and the Redis bindings
// (`RedisModule`, `RedisQueueModule`, `RedisWorkerModule`) arrive together.
pub(super) const REDIS: Dep = nest_rs(&["redis"]);
pub(super) const MCP: Dep = nest_rs(&["mcp"]);
pub(super) const AUTHN: Dep = nest_rs(&["authn"]);
pub(super) const AUTHZ: Dep = nest_rs(&["authz", "http"]);
// Mirrors the feature set `nest-rs-seaorm` itself resolves — a divergent list
// (or a release-candidate floor) would be a manifest the user inherits and has
// to un-learn later.
const SEA_ORM: Dep = Dep {
    name: "sea-orm",
    workspace_value: "{ version = \"2.0\", default-features = false, features = [\"sqlx-postgres\", \"runtime-tokio-rustls\", \"macros\", \"with-uuid\", \"with-chrono\"] }",
    features: &[],
};
const UUID: Dep = Dep {
    name: "uuid",
    workspace_value: "{ version = \"1.24\", features = [\"v7\", \"serde\"] }",
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
// Every adapter skeleton that logs (`queue`, `schedule`, `ws`) writes a
// `tracing::` call in the handler body, and `queue`/`schedule` return
// `anyhow::Result`. `templates::workspace` now ships both from `nestrs new`, so
// on a scaffolded tree these two entries are idempotent no-ops; they stay for
// the workspace assembled by hand, where the generator is the only thing that
// knows what its own skeleton names.
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
    workspace_value: "{ version = \"1.53\", features = [\"macros\", \"rt-multi-thread\"] }",
    features: &[],
};

/// The crates a resource port (DB-backed CRUD + HTTP) needs.
///
/// `#[expose]` carries its own derives now, each routed back through the
/// framework, so `schemars` / `validator` / `uuid` / `chrono` are no longer
/// call-site deps. `authz` stays because `#[crud]` emits `Authorize<…>`
/// parameters — the developer's `#[authorize]` is what turns it on.
pub fn resource_deps() -> Vec<&'static Dep> {
    vec![&SEAORM, &RESOURCE, &AUTHZ, &SEA_ORM, &SERDE]
}

/// The crates one `#[expose]`d entity needs, and no more — [`resource_deps`]
/// without `authz`, which belongs to the `#[crud]` controller rather than to the
/// entity. `seaorm` is the entity's own: `soft_delete` expands to the
/// `SoftDeletable` impl that crate declares.
pub fn entity_deps() -> Vec<&'static Dep> {
    vec![&SEAORM, &RESOURCE, &SEA_ORM, &SERDE]
}

/// The crates the authn/authz adapter (`g auth`) needs.
pub fn auth_deps() -> Vec<&'static Dep> {
    // `RESOURCE` is what `#[wire_enum]` on `Role` needs: the principal's role
    // enum is named by `Claims` *and* by the development token DTO, so it
    // carries the wire derives rather than serde alone.
    vec![&AUTHN, &AUTHZ, &RESOURCE, &SERDE, &UUID]
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
        Transport::Graphql => vec![&GRAPHQL, &ASYNC_GRAPHQL],
        Transport::Ws => vec![&WS, &TRACING],
        // `SERDE`: the payload `g queue` writes at the port carries plain
        // derives rather than `#[input]` — a producer↔worker contract has to
        // accept a field a newer producer added, and `deny_unknown_fields`
        // would dead-letter those jobs on their first attempt.
        Transport::Queue => vec![&REDIS, &ANYHOW, &TRACING, &SERDE],
        Transport::Schedule => vec![&SCHEDULE, &ANYHOW, &TRACING],
        Transport::Mcp => vec![&MCP],
    }
}

/// What an **app crate** needs to depend on to name the transport's root
/// module — the one the generator's own printed next step tells the reader to
/// import. Empty where the scaffold already carries it: every app crate
/// `nestrs new` writes depends on `nest-rs-http`, so HTTP, WS and MCP add
/// nothing.
pub fn app_host_deps(transport: Transport) -> Vec<&'static Dep> {
    match transport {
        Transport::Http | Transport::Ws | Transport::Mcp => vec![],
        Transport::Graphql => vec![&GRAPHQL],
        Transport::Queue => vec![&REDIS],
        Transport::Schedule => vec![&SCHEDULE],
    }
}

// The crates each `authz/<transport>/` bridge needs are listed on the bridge
// itself (`generate::auth`), beside the files that name them — one row per
// transport rather than a helper per transport.

/// What exposing an entity over GraphQL needs: `#[expose(graphql)]` derives the
/// async-graphql object through `nest_rs_resource::graphql`, which that crate
/// only compiles under its own `graphql` feature.
pub fn graphql_port_deps() -> Vec<&'static Dep> {
    vec![&RESOURCE, &GRAPHQL]
}

/// Edit the root manifest: add any missing `[workspace.dependencies]` entries.
pub fn ensure_workspace_deps(deps: Vec<&'static Dep>) -> Transform {
    ensure_deps(deps, &["workspace", "dependencies"], Dep::workspace_item)
}

/// Edit the `features` manifest: add any missing `[dependencies]` entries as
/// `{ workspace = true, features = [...] }`.
pub fn ensure_features_deps(deps: Vec<&'static Dep>) -> Transform {
    ensure_deps(deps, &["dependencies"], |_| Item::Value(workspace_value()))
}

/// The shared half: insert what is absent, then enable any feature the entry is
/// missing. The second step is what a capability needs — it is a **feature** of
/// the single `nest-rs` entry, so an already-present entry still has to gain
/// it, or `g graphql` on an equipped workspace silently leaves it off.
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
        let src = "[workspace.dependencies]\nanyhow = \"1\"\n";
        let t = ensure_workspace_deps(vec![&SEAORM]);
        let out = t(src).expect("adds nest-rs");
        // The pin tracks the CLI's own release line, not a hard-coded literal.
        assert!(out.contains("nest-rs"), "{out}");
        assert!(out.contains("seaorm"), "{out}");
        // already present → no-op
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

    // The `g graphql` case: the crate is already a dependency (every scaffolded
    // workspace carries `nest-rs-guards`), so only its feature is missing —
    // and without it `#[resolver]` expands to names that do not exist.
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
                    if !src.contains(token) {
                        continue;
                    }
                    // Two legal ways to reach it, and a skeleton must take one:
                    // declare the crate, or import the framework's re-export
                    // (`use nest_rs::mcp::rmcp;`) — which is what keeps the
                    // generated manifest at a single `nest-rs` entry.
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
        for (key, value) in crate::templates::crud_vars(crud_port, transport) {
            r = r.with(key, value);
        }
        r.render(handler)
    }

    /// A2: `count()` exists only on the `g feature` service. A `g resource`
    /// port's service is a `CrudService`, so a skeleton calling it produced a
    /// workspace that did not compile — and rustc blamed `Iterator::count`,
    /// sending the reader after an iterator bug. The CLI page's guarantee is
    /// unconditional ("a freshly-generated port plus **any** adapter compiles
    /// immediately"), so no CRUD skeleton may name it.
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

    /// …and the plain-feature skeletons must keep delegating, so the two
    /// variants cannot silently collapse into one inert stub.
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

    /// Every third-party requirement in the repo is spelled `major.minor` —
    /// the floor we actually build against, patch left to the publisher.
    /// `"1"` claims less than we know (it accepts a 1.0 that never compiled
    /// here); `"1.53.1"` claims more than we mean (it rejects the patch
    /// releases we want to inherit for free).
    ///
    /// This lives in the generator's suite because the generator is what
    /// propagates the form: a scaffolded workspace inherits these pins
    /// verbatim, so a manifest that drifts teaches every new project to drift
    /// with it. It walks the repo's manifests rather than the CLI's own so the
    /// rule is enforced where it is written, not restated per crate.
    #[test]
    fn versions_are_major_minor() {
        // Exact pin on the patch, with a bump procedure in the root manifest:
        // `nest-rs-graphql` reads async-graphql's public-but-internal registry
        // API, so a minor may silently change what it spells out.
        //
        // **The exemption is a property of the table, not of the name.** It was
        // keyed on the crate name alone, which excused the pin in every manifest
        // in the repo *and* in every manifest the CLI generates — including the
        // template that spells `async-graphql = "7.2"` today and would have been
        // waved through had it drifted. `manifests-ci.md` scopes it twice over:
        // "One documented exception, **at its pin**", and "Third-party versions
        // live in `[workspace.dependencies]` **only**". So a `=` outside a
        // workspace table is a second, undocumented pin whatever the crate is.
        const EXACT: [&str; 2] = ["async-graphql", "async-graphql-poem"];
        // Every manifest the repo owns, walked rather than listed — the same
        // population its twin reads. Three literal paths reached the two
        // workspace roots and the bench SUT, and nothing else: a member crate
        // hardcoding a requirement instead of `{ workspace = true }` was outside
        // what this could see.
        let mut sources = repo_manifests();
        // The manifests the CLI **generates**, which the rule names and this
        // test did not reach: a drift there ships to every scaffolded project
        // and fails no suite in this repo. Discovered rather than listed, so a
        // template added later is covered the day it is written.
        sources.extend(generated_manifests());
        let mut checked = 0usize;
        for (rel, raw) in &sources {
            let doc = raw.parse::<DocumentMut>().expect("valid TOML");
            for (is_workspace_policy, table) in dep_tables(&doc) {
                let Some(table) = table.and_then(|t| t.as_table_like()) else {
                    continue;
                };
                for (name, entry) in table.iter() {
                    // `nest-rs-*` tracks the release line, not a third party.
                    if name.starts_with("nest-rs") || (is_workspace_policy && EXACT.contains(&name))
                    {
                        continue;
                    }
                    let req = match entry.as_str() {
                        Some(literal) => literal,
                        // A path-only entry (a sibling product crate) has no
                        // requirement to spell.
                        None => match entry.get("version").and_then(Item::as_str) {
                            Some(version) => version,
                            None => continue,
                        },
                    };
                    checked += 1;
                    if req.contains(PLACEHOLDER) {
                        // Rendered from a `{{…}}`: the only requirement allowed to
                        // be one is the umbrella's, whose value is `framework_req`
                        // and tracks our release line rather than this rule.
                        assert!(
                            name.starts_with("nest-rs"),
                            "{rel}: `{name}` takes its version from a template \
                             placeholder; only the umbrella may, since only its \
                             requirement is the framework's own",
                        );
                        continue;
                    }
                    assert!(
                        is_major_minor(req.trim_start_matches('=')),
                        "{rel}: `{name} = \"{req}\"` — third-party requirements are \
                         `major.minor`; a bare major accepts releases we never built \
                         against, a patch component rejects the fixes we want",
                    );
                }
            }
        }
        assert!(checked > 0, "no manifest was reachable to check");
        // Not `starts_with("templates:")`: the spliced `workspace_value` table
        // is pushed unconditionally and satisfied that on its own, so the guard
        // could not see the template-file walk breaking — which it silently does
        // the day a body is written `r##"…"##`.
        assert!(
            sources.iter().any(
                |(rel, _)| rel.starts_with("templates:") && rel.ends_with(".rs#0")
                    || rel.contains(".rs#")
            ),
            "no manifest was discovered in `src/templates/` — the raw-string scan \
             stopped matching, and a drift in a generated manifest would now ship \
             to every scaffolded project without failing anything here",
        );
    }

    // `manifests-ci.md` states that `rust-toolchain.toml` "pins the toolchain and
    // matches the workspace `rust-version`" — and nothing read it. The floor is
    // restated by three workspaces, three images, the publish workflow, the
    // doctor's own floor, the documented requirement and everything `nestrs new`
    // writes — so a bump that misses one ships a scaffold pinning a compiler the
    // framework no longer builds on, failing no suite here. Same reason its twin walks the generated manifests:
    // the half nobody runs is the half that drifts.
    #[test]
    fn toolchain_pins_agree() {
        // Every shape, and how its match is to be read. Three obligations, not
        // one: every value **agrees** with the anchor, every shape is **present**
        // (a scan that stops matching finds nothing, and finding nothing reads
        // exactly like finding nothing wrong), and every value a `Pin::Exact`
        // marker introduces is **well formed** — the case that motivated the
        // distinction is `channel = "stable"` in the scaffold's own
        // `rust-toolchain.toml`, which pins no version at all and which a
        // value-comparing scan walks straight past.
        // The third field is whether the shape must still be found when the
        // repo is not readable — i.e. whether the *templates* carry it. Kept on
        // the shape rather than in a second list: a list is edited by a
        // different hand than the one that renames a marker, and a `matches!`
        // that stops matching turns its presence check into a silent no-op.
        const SHAPES: [(&str, Pin, bool); 9] = [
            ("rust-version = \"", Pin::Exact, true),
            ("channel = \"", Pin::Exact, true),
            ("ARG RUST_VERSION=", Pin::Exact, true),
            ("toolchain: '", Pin::Exact, false),
            ("const MIN_RUST_VERSION: (u32, u32) = (", Pin::Tuple, false),
            ("FROM rust:", Pin::Image, true),
            ("**Rust ", Pin::Prose, false),
            ("pins Rust ", Pin::Prose, false),
            ("`rustc` \u{2265} ", Pin::Prose, false),
        ];

        let (in_repo, sources) = floor_sources();

        // The anchor is the toolchain the repo actually builds on; away from the
        // repo it is whatever the scaffold writes, which is the only floor a
        // packaged crate can be wrong about.
        let floor = sources
            .iter()
            .filter(|(rel, _)| !in_repo || rel == "rust-toolchain.toml")
            .find_map(|(_, raw)| read_channel(raw))
            .expect("the pinned toolchain channel");
        // The anchor is checked before it is trusted. Unvalidated, a
        // `channel = "1.97.1"` or `channel = "stable"` reported every correctly
        // pinned file in the repo as the stale one, naming whichever sorted
        // first — one edit, an accusation against every other site, and the
        // culprit named nowhere.
        assert!(
            is_major_minor(&floor),
            "rust-toolchain.toml pins `channel = \"{floor}\"`, which is not a bare \
             `major.minor`. It is the anchor every other spelling in the repo is \
             compared against, so a channel name or a patch component here \
             reports every correctly pinned site as stale and none of them is",
        );

        let mut seen = [0usize; SHAPES.len()];
        for (rel, raw) in &sources {
            for (shape, (marker, kind, _)) in SHAPES.iter().enumerate() {
                for chunk in raw.split(marker).skip(1) {
                    // `MIN_RUST_VERSION` spells the pair as a tuple; every other
                    // shape spells it dotted.
                    let read = match kind {
                        Pin::Tuple => read_version(&chunk.replacen(", ", ".", 1)),
                        _ => read_version(chunk),
                    };
                    match read.as_deref().filter(|value| is_major_minor(value)) {
                        Some(value) => {
                            seen[shape] += 1;
                            assert_eq!(
                                value, floor,
                                "{rel}: `{marker}{value}` — the Rust floor is \
                                 pinned at `{floor}` by `rust-toolchain.toml`. \
                                 Every spelling moves in one edit: a stale one \
                                 here is a scaffold, an image or a doc promising \
                                 a compiler this workspace no longer builds on",
                            );
                        }
                        // An image may name the `ARG` carrying the floor
                        // rather than the floor itself, which is what all three
                        // Dockerfiles do. A *literal* tag there is a pin nobody
                        // would think to move, so it is read and compared above.
                        None if *kind == Pin::Image && chunk.starts_with("${RUST_VERSION}") => {
                            seen[shape] += 1;
                        }
                        // A sentence that merely opens with the marker —
                        // `**Rust 1.97+**` carries the floor, `**Rust and Cargo**`
                        // does not.
                        None if *kind == Pin::Prose => {}
                        None => panic!(
                            "{rel}: `{marker}{}` — this marker spells the Rust \
                             floor, so what follows it must be a bare \
                             `major.minor`, and `{floor}` is what \
                             `rust-toolchain.toml` pins. A channel name, a patch \
                             component or a suffixed tag pins something else, and \
                             a scan that only compares *values* passes over it \
                             without a word",
                            preview(chunk),
                        ),
                    }
                }
            }
        }

        // Away from the repo only the templates are readable, so only the
        // shapes they carry can be required to appear.
        for ((marker, _, in_templates), count) in SHAPES.iter().zip(seen) {
            if in_repo || *in_templates {
                assert!(
                    count > 0,
                    "no `{marker}…` was found — the scan for that shape stopped \
                     matching, so a stale floor written that way would now pass \
                     unread",
                );
            }
        }
    }

    // Presence per *shape* catches a scan that stopped matching, but not a pin
    // deleted from one of several files sharing a shape: `rust-version` is
    // written five times, so removing it from `demo/Cargo.toml` — or from the
    // workspace scaffold — left the count positive and `toolchain_pins_agree`
    // green. This is the obligation that sees it, derived rather than listed,
    // since a list is extended by the same edit that forgets.
    #[test]
    fn every_workspace_root_declares_the_floor() {
        let (_, sources) = floor_sources();
        let mut checked = 0;
        for (rel, raw) in &sources {
            // Only a manifest can root a workspace, and TOML-parsing 129 docs
            // pages to watch every one of them fail was a third of the scan.
            if !rel.ends_with("Cargo.toml") && !rel.starts_with("templates:") {
                continue;
            }
            let Ok(doc) = fill_placeholders(raw).parse::<DocumentMut>() else {
                continue; // a template body that is not TOML
            };
            if doc.get("workspace").is_none() {
                continue;
            }
            checked += 1;
            let declares = doc
                .get("workspace")
                .and_then(|w| w.get("package"))
                .and_then(|p| p.get("rust-version"))
                .is_some()
                || doc
                    .get("package")
                    .and_then(|p| p.get("rust-version"))
                    .is_some();
            assert!(
                declares,
                "{rel} roots a workspace but declares no `rust-version` — the \
                 floor is what tells cargo to refuse an old compiler with a \
                 sentence instead of a page of type errors, and a root that \
                 states none states it for every crate beneath it",
            );
        }
        assert!(checked > 0, "no workspace root was reachable to check");
    }

    /// How a marker's match is to be read.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pin {
        /// The marker is syntax: whatever follows it *is* the floor, so a value
        /// that is not a bare `major.minor` is a malformed or stale pin, never
        /// prose — and is reported rather than skipped.
        Exact,
        /// [`Pin::Exact`], spelled as the Rust tuple `(major, minor)`.
        Tuple,
        /// A base image tag: either the floor, or the `ARG` carrying it.
        Image,
        /// The marker is English that *may* open a version. Only a leading
        /// version is read; anything else is a sentence, and skipping it is
        /// correct rather than a hole.
        Prose,
    }

    /// The version a marker introduces: the leading run of digits and dots.
    ///
    /// Every site terminates its own version — `1.97"`, `1.97+**`, `1.97)`,
    /// `1.97-slim`, or the end of the line — which closing *characters* did not.
    /// `**Rust ` closed on the next `+` anywhere in the file, so `**Rust 1.96**`
    /// read a paragraph, failed the shape test, and was skipped in silence.
    fn read_version(chunk: &str) -> Option<String> {
        let end = chunk
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(chunk.len());
        (end > 0).then(|| chunk[..end].to_owned())
    }

    /// Two components, both digits — the form `manifests-ci.md` requires of
    /// every version this repo writes.
    fn is_major_minor(value: &str) -> bool {
        // A third component fails the digit test rather than needing its own
        // arm: the extra dot lands inside `minor`.
        let Some((major, minor)) = value.split_once('.') else {
            return false;
        };
        !major.is_empty()
            && !minor.is_empty()
            && major
                .bytes()
                .chain(minor.bytes())
                .all(|b| b.is_ascii_digit())
    }

    /// What a marker was actually followed by, for an assertion message: to the
    /// end of the line and capped, so a whole file never lands in the output.
    fn preview(chunk: &str) -> String {
        chunk
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(24)
            .collect()
    }

    /// Every raw-string body under `src/templates/`, unfiltered.
    ///
    /// `generated_manifests` keeps only bodies that parse as TOML **and** declare
    /// dependencies — right for a requirement rule, wrong for this one: the
    /// scaffold's `rust-toolchain.toml` declares no dependency and its Dockerfile
    /// is not TOML at all, so the two pins a developer inherits most directly
    /// would both have been invisible to the test written to guard them.
    fn template_bodies() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/templates");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("the templates directory")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
            .collect();
        files.sort();
        let mut found = Vec::new();
        for path in files {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let source = std::fs::read_to_string(&path).expect("a template source");
            for (index, body) in raw_string_bodies(&source).into_iter().enumerate() {
                found.push((format!("templates:{name}#{index}"), body));
            }
        }
        found
    }

    /// The anchor's `channel = "…"` value, **literally**.
    ///
    /// Literally, not as a version: `channel = "stable"` has to reach the
    /// assertion that rejects it, which a version reader would have dropped on
    /// the way.
    fn read_channel(raw: &str) -> Option<String> {
        let after = raw.split("channel = \"").nth(1)?;
        Some(after[..after.find('"')?].to_owned())
    }

    /// Whether a file may spell the Rust floor: manifests, the toolchain pin,
    /// the images, the workflows, the CLI's own floor and the documented
    /// requirement. A predicate rather than a list of paths — a new app, image
    /// or page is covered the day it is written, which a list never is.
    fn version_bearing(name: &str, path: &std::path::Path) -> bool {
        matches!(name, "Cargo.toml" | "rust-toolchain.toml" | "doctor.rs")
            || name.starts_with("Dockerfile")
            || matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("yml") | Some("mdx")
            )
    }

    /// Every source that may spell the Rust floor, and whether the repo itself
    /// was readable.
    ///
    /// Away from the repo — a packaged crate — the sibling workspaces and the
    /// docs are gone, so only the templates can be read and only they can be
    /// required to carry anything.
    fn floor_sources() -> (bool, Vec<(String, String)>) {
        let in_repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../rust-toolchain.toml")
            .is_file();
        let mut sources = if in_repo {
            repo_sources(&version_bearing)
        } else {
            Vec::new()
        };
        sources.extend(template_bodies());
        (in_repo, sources)
    }

    /// The stand-in a `{{placeholder}}` renders to: TOML-safe, and impossible to
    /// mistake for a version component.
    const PLACEHOLDER: &str = "PLACEHOLDER";

    /// Every raw-string body in a Rust source — `r#"…"#` and `r##"…"##` alike.
    ///
    /// Rust *forces* the second form the moment a body contains `"#`, so a scan
    /// for the first alone stops finding a template the day someone writes a
    /// `Cargo.toml` fragment holding one — silently, since finding nothing looks
    /// exactly like finding nothing to check.
    fn raw_string_bodies(source: &str) -> Vec<String> {
        let mut bodies = Vec::new();
        for hashes in 1..=3usize {
            let open = format!("r{}\"", "#".repeat(hashes));
            let close = format!("\"{}", "#".repeat(hashes));
            for chunk in source.split(&open).skip(1) {
                if let Some(body) = chunk.split(&close).next() {
                    bodies.push(body.to_owned());
                }
            }
        }
        bodies
    }

    /// Every manifest the CLI generates, discovered from its own sources.
    ///
    /// Two shapes, both scanned rather than listed: the raw-string templates
    /// under `src/templates/`, and the `workspace_value` literals in this file
    /// — the two places a scaffolded project's requirements actually come from.
    /// A list would have to be extended by the same edit that adds a template,
    /// which is exactly the edit that forgets.
    fn generated_manifests() -> Vec<(String, String)> {
        // `template_bodies` owns the walk, including the `r#"…"#` / `r##"…"##`
        // discovery rule that goes *quiet* rather than failing when it breaks —
        // a rule with two copies is a rule that gets fixed in one of them.
        let mut found: Vec<(String, String)> = template_bodies()
            .into_iter()
            .filter_map(|(rel, body)| {
                let rendered = fill_placeholders(&body);
                let doc = rendered.parse::<DocumentMut>().ok()?; // a Rust or Markdown template
                let declares_deps = doc.get("dependencies").is_some()
                    || doc
                        .get("workspace")
                        .and_then(|w| w.get("dependencies"))
                        .is_some();
                declares_deps.then_some((rel, rendered))
            })
            .collect();
        // `[workspace.dependencies]` entries this file splices in on demand.
        // Read off the generator's own accessors, which *are* the exhaustive
        // enumeration by construction — a `Dep` no accessor reaches is a `Dep`
        // that is never generated. Scraping this file's `workspace_value:`
        // literals said the same thing through a hand-written Rust lexer, and
        // would have gone quiet the day rustfmt wrapped one of them.
        let mut spliced = String::from("[workspace.dependencies]\n");
        let mut every: Vec<&'static Dep> = Vec::new();
        every.extend(resource_deps());
        every.extend(entity_deps());
        every.extend(auth_deps());
        every.extend(migrations_deps());
        every.extend(graphql_port_deps());
        for transport in Transport::ALL {
            every.extend(adapter_deps(transport));
            every.extend(app_host_deps(transport));
        }
        // Deduplicated: several accessors legitimately reach the same `Dep`, and
        // one TOML table cannot repeat a key.
        every.sort_by_key(|dep| dep.name);
        every.dedup_by_key(|dep| dep.name);
        for dep in every {
            // The umbrella's requirement is `framework_req`, which tracks our own
            // release line rather than this rule.
            if dep.name.starts_with("nest-rs") {
                continue;
            }
            spliced.push_str(&format!("{} = {}\n", dep.name, dep.workspace_value));
        }
        found.push(("generator dependency table".to_owned(), spliced));
        found
    }

    /// Replace every `{{placeholder}}` with a marker that keeps the TOML
    /// parseable **and** stays visible in the result.
    ///
    /// A version requirement *is* sometimes a placeholder — the umbrella's is
    /// (`nest-rs = { version = "{{nestrs_version}}" }`), and its value tracks our
    /// own release line through `framework_req` rather than the `major.minor`
    /// rule. So the check cannot simply count components on a rendered value: a
    /// tail placeholder (`"1.{{v}}"`) would render two components and pass
    /// whatever the real value turns out to be. The marker is what lets the
    /// caller tell those apart and demand that they belong to the umbrella.
    fn fill_placeholders(body: &str) -> String {
        let mut out = String::with_capacity(body.len());
        let mut rest = body;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    out.push_str(PLACEHOLDER);
                    rest = &after[end + 2..];
                }
                None => {
                    rest = after;
                    break;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Every manifest that **consumes** the framework names exactly one
    /// `nest-rs*` dependency: the umbrella. A second line is the defect *The
    /// umbrella is the front door* describes — a capability whose decorators
    /// oblige the developer to declare a satellite.
    ///
    /// It lives beside [`versions_are_major_minor`] for the same reason and
    /// walks the same way: these manifests are what the generator propagates
    /// and what a reader copies, so the rule is enforced where it is written.
    /// `bench/sut/nestrs` is on the list deliberately — it sits outside both
    /// workspaces, so `cargo clippy --workspace` never reaches it and it is the
    /// one consumer that can drift unobserved. It did.
    #[test]
    fn consumers_name_only_the_umbrella() {
        let manifests = consumer_manifests();
        // The eleven the literal list held, and every consumer added since.
        // Below that the walk is reading the wrong tree, and a green run would
        // mean nothing.
        assert!(
            manifests.len() >= 11,
            "the walk found {} consumer manifest(s) — below eleven it is reading \
             the wrong tree, and passing proves nothing",
            manifests.len(),
        );
        let mut checked = 0usize;
        for (rel, raw) in &manifests {
            let doc = raw.parse::<DocumentMut>().expect("valid TOML");
            for (_, table) in dep_tables(&doc) {
                let Some(table) = table.and_then(|t| t.as_table_like()) else {
                    continue;
                };
                for (name, _) in table.iter() {
                    if !name.starts_with("nest-rs") {
                        continue;
                    }
                    checked += 1;
                    assert_eq!(
                        name, "nest-rs",
                        "{rel}: declares `{name}`. A consumer names the umbrella and \
                         nothing else — enable the capability's feature on `nest-rs` \
                         instead. See *The umbrella is the front door* in CLAUDE.md",
                    );
                }
            }
        }
        assert!(checked > 0, "no consumer manifest was reachable to check");
    }

    /// Every manifest that consumes the framework, derived rather than listed.
    ///
    /// Two disjoint halves, and both are needed:
    ///
    /// - **outside the framework's own workspace** — `crates/*` and the root
    ///   manifest that owns them. Everything else consumes from outside, which
    ///   is what reaches
    ///   `bench/sut/nestrs`, which carries its own empty `[workspace]` table so
    ///   `cargo clippy --workspace` never sees it; it drifted back to a
    ///   five-crate stanza unobserved, and a *name*-based rule would not have
    ///   caught it either, since it named no umbrella to be recognised by.
    /// - **names the umbrella** — a framework crate never depends on `nest-rs`
    ///   (it would be a cycle), so this half reaches exactly the consumers that
    ///   live *among* the framework crates. Today that is
    ///   `nest-rs-macro-hygiene`, the compile-time witness, and `CLAUDE.md` is
    ///   explicit about it: "If its manifest needs a second line, the rule is
    ///   broken."
    ///
    /// The manifests the CLI **generates** are on the list for the reason its
    /// twin gives: a scaffolded project inherits them verbatim, so a second
    /// `nest-rs-*` line there ships to every new project and fails nothing here.
    ///
    /// Deriving is not a tidiness argument. The list this replaced held eleven
    /// paths, and the rule it enforces is about the manifest a *reader* copies —
    /// a set that grows by the same edit that forgets to extend a list.
    fn consumer_manifests() -> Vec<(String, String)> {
        let mut found: Vec<(String, String)> = repo_manifests()
            .into_iter()
            .filter(|(rel, raw)| {
                // The framework's own workspace is `crates/*` **plus the root
                // manifest that owns them** — that manifest declares every
                // `nest-rs-*` version in one `[workspace.dependencies]` table,
                // which is the release line rather than a consumer's install.
                let outside_the_framework = rel != "Cargo.toml" && !rel.starts_with("crates/");
                // Short-circuited deliberately: the second half is a full TOML
                // parse of every manifest in the repo, and it only ever has to
                // answer for the ones living *among* the framework crates.
                outside_the_framework
                    || raw
                        .parse::<DocumentMut>()
                        .ok()
                        .is_some_and(|doc| declares_the_umbrella(&doc))
            })
            .collect();
        found.extend(generated_manifests());
        found
    }

    /// Every `Cargo.toml` the repo owns, by repo-relative path.
    ///
    /// Both manifest rules read this: one asks what every requirement is spelled
    /// like, the other which of these manifests consume the framework. Sharing
    /// the walk is what keeps them from disagreeing about the population, which
    /// is exactly what happened — the derived half found a drift the literal
    /// half was blind to.
    fn repo_manifests() -> Vec<(String, String)> {
        repo_sources(&|name, _| name == "Cargo.toml")
    }

    /// The four tables a dependency requirement can be written in, each paired
    /// with whether it is the **workspace's own policy line** — which is what
    /// decides whether the documented exact pin applies. All three manifest
    /// rules read the same four, so the list is here rather than in each.
    fn dep_tables(doc: &DocumentMut) -> [(bool, Option<&Item>); 4] {
        [
            (
                true,
                doc.get("workspace").and_then(|w| w.get("dependencies")),
            ),
            (false, doc.get("dependencies")),
            (false, doc.get("dev-dependencies")),
            (false, doc.get("build-dependencies")),
        ]
    }

    fn declares_the_umbrella(doc: &DocumentMut) -> bool {
        dep_tables(doc)
            .into_iter()
            .filter_map(|(_, table)| table?.as_table_like())
            .any(|table| table.get("nest-rs").is_some())
    }

    /// Every file under `dir` that `pick` accepts, skipping the trees no rule
    /// here has business reading.
    ///
    /// One walker for every rule that needs one. Two of them had already
    /// diverged — one pruned `dist/`, the other descended into it — so the
    /// manifest rules and the toolchain rule saw different populations of the
    /// same repo, for no reason either could state.
    fn collect_files(
        dir: &std::path::Path,
        pick: &dyn Fn(&str, &std::path::Path) -> bool,
        out: &mut Vec<std::path::PathBuf>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            if path.is_dir() {
                if matches!(name.as_str(), "target" | "node_modules" | ".git" | "dist") {
                    continue;
                }
                collect_files(&path, pick, out);
            } else if pick(&name, &path) {
                out.push(path);
            }
        }
    }

    /// Every file `pick` accepts, by repo-relative path, with its contents.
    fn repo_sources(pick: &dyn Fn(&str, &std::path::Path) -> bool) -> Vec<(String, String)> {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut paths = Vec::new();
        collect_files(&repo, pick, &mut paths);
        paths.sort();
        paths
            .into_iter()
            .filter_map(|path| {
                let rel = path
                    .strip_prefix(&repo)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                // Packaged crate: the sibling workspaces aren't there.
                std::fs::read_to_string(&path).ok().map(|raw| (rel, raw))
            })
            .collect()
    }

    // A hand-rolled manifest may pin a version literally; the feature list then
    // has nowhere to go until the entry is widened into a table.
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
