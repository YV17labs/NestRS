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
pub(super) const RESOURCE: Dep = nest_rs(&["resource"]);
pub(super) const GRAPHQL: Dep = nest_rs(&["graphql"]);
pub(super) const WS: Dep = nest_rs(&["ws"]);
pub(super) const SCHEDULE: Dep = nest_rs(&["schedule"]);
// `redis` implies `queue`: the abstractions and the Redis-bound
// `QueueConnection` / `QueueModule` arrive together.
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
        Transport::Graphql => vec![&GRAPHQL, &ASYNC_GRAPHQL],
        Transport::Ws => vec![&WS, &TRACING],
        Transport::Queue => vec![&REDIS, &ANYHOW, &TRACING],
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
        const EXACT: [&str; 2] = ["async-graphql", "async-graphql-poem"];
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut sources: Vec<(String, String)> = Vec::new();
        for rel in [
            "Cargo.toml",
            "demo/Cargo.toml",
            "bench/sut/nestrs/Cargo.toml",
        ] {
            // Packaged crate: the sibling workspaces aren't there.
            if let Ok(raw) = std::fs::read_to_string(repo.join(rel)) {
                sources.push((rel.to_owned(), raw));
            }
        }
        // The manifests the CLI **generates**, which the rule names and this
        // test did not reach: a drift there ships to every scaffolded project
        // and fails no suite in this repo. Discovered rather than listed, so a
        // template added later is covered the day it is written.
        sources.extend(generated_manifests());
        let mut checked = 0usize;
        for (rel, raw) in &sources {
            let doc = raw.parse::<DocumentMut>().expect("valid TOML");
            let tables = [
                doc.get("workspace").and_then(|w| w.get("dependencies")),
                doc.get("dependencies"),
                doc.get("dev-dependencies"),
                doc.get("build-dependencies"),
            ];
            for table in tables.into_iter().flatten() {
                let Some(table) = table.as_table_like() else {
                    continue;
                };
                for (name, entry) in table.iter() {
                    // `nest-rs-*` tracks the release line, not a third party.
                    if name.starts_with("nest-rs") || EXACT.contains(&name) {
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
                    assert_eq!(
                        req.trim_start_matches('=').split('.').count(),
                        2,
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
        let mut found = Vec::new();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/templates");
        let entries = std::fs::read_dir(&dir).expect("the templates directory");
        let mut files: Vec<std::path::PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            let source = std::fs::read_to_string(&path).expect("a template source");
            // `r#"…"#` and `r##"…"##`: Rust forces the second the moment a body
            // contains `"#`, and a scan for the first alone goes quiet rather
            // than failing when that happens.
            for (index, body) in raw_string_bodies(&source).into_iter().enumerate() {
                let rendered = fill_placeholders(&body);
                let Ok(doc) = rendered.parse::<DocumentMut>() else {
                    continue; // not TOML — a Rust or Markdown template
                };
                let declares_deps = doc.get("dependencies").is_some()
                    || doc
                        .get("workspace")
                        .and_then(|w| w.get("dependencies"))
                        .is_some();
                if declares_deps {
                    found.push((format!("templates:{name}#{index}"), rendered));
                }
            }
        }
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
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifests = [
            // The product, workspace table and every member.
            "demo/Cargo.toml",
            "demo/apps/api/Cargo.toml",
            "demo/apps/assistant/Cargo.toml",
            "demo/apps/auth/Cargo.toml",
            "demo/apps/live/Cargo.toml",
            "demo/apps/worker/Cargo.toml",
            "demo/crates/features/Cargo.toml",
            "demo/crates/migrations/Cargo.toml",
            "demo/crates/seed/Cargo.toml",
            // The benchmark SUT: it measures what we ship, so it installs the
            // way we tell people to install.
            "bench/sut/nestrs/Cargo.toml",
            // The compile-time witness. CLAUDE.md: "If its manifest needs a
            // second line, the rule is broken."
            "crates/nest-rs-macro-hygiene/Cargo.toml",
        ];
        let mut checked = 0usize;
        for rel in manifests {
            let path = repo.join(rel);
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue; // packaged crate: the sibling workspaces aren't there
            };
            let doc = raw.parse::<DocumentMut>().expect("valid TOML");
            let tables = [
                doc.get("workspace").and_then(|w| w.get("dependencies")),
                doc.get("dependencies"),
                doc.get("dev-dependencies"),
                doc.get("build-dependencies"),
            ];
            for table in tables.into_iter().flatten() {
                let Some(table) = table.as_table_like() else {
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
