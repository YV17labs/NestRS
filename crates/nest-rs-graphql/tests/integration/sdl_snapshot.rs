//! Behavioural guard for the pinned async-graphql registry API (see the
//! `nest-rs-graphql` crate `//!` doc). The compile-time canary in
//! `src/resolver.rs` catches shape changes to `MetaType::Object`; this test
//! catches drift the compiler cannot see — e.g. a change to
//! `remove_unused_types` that leaks member object types into the SDL, or a
//! change in how sorted SDL export renders.
//!
//! It composes a two-resolver schema (a query-only resolver and one carrying
//! both a query and a mutation) and asserts the emitted SDL byte-for-byte
//! against the committed snapshot below. A diff here after an async-graphql
//! bump is the review signal: intended ⇒ update the snapshot; unexpected ⇒
//! regression.

use std::path::PathBuf;

use async_graphql::SimpleObject;
use nest_rs_core::module;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_testing::TestApp;

#[derive(SimpleObject)]
struct Widget {
    id: i32,
    label: String,
}

#[resolver]
struct AlphaResolver;

#[operations]
impl AlphaResolver {
    #[query]
    #[public]
    async fn widget(&self, id: i32) -> Widget {
        Widget {
            id,
            label: "alpha".into(),
        }
    }
}

#[resolver]
struct BetaResolver;

#[operations]
impl BetaResolver {
    #[query]
    #[public]
    async fn ping(&self) -> String {
        "pong".into()
    }

    #[mutation]
    #[public]
    async fn bump(&self, by: i32) -> i32 {
        by + 1
    }
}

#[module(providers = [AlphaResolver])]
struct AlphaModule;

#[module(providers = [BetaResolver])]
struct BetaModule;

/// Per-process temp file the boot-time `emit_sdl` writes to. `render_sdl` is
/// `pub(crate)`, so the SDL is captured through the same production path an app
/// uses (`NESTRS_GRAPHQL__EMIT_SDL`) rather than by reaching into crate
/// internals.
fn snapshot_path(stem: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nest_rs_graphql_{stem}_{}.graphql",
        std::process::id()
    ))
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        emit_sdl: true,
        schema_path: snapshot_path("merged"),
        ..GraphqlConfig::default()
    }),
    AlphaModule,
    BetaModule,
])]
struct SnapshotApp;

/// The committed SDL (tab-indented, as `render_sdl` emits it). Regenerate
/// deliberately — never blindly — via the bump procedure in the crate `//!`
/// doc. Fields, arguments, and enum items are sorted by `render_sdl`, so the
/// only churn a legitimate change produces is the change itself.
const EXPECTED_SDL: &str = "\
type Mutation {\n\
\tbump(by: Int!): Int!\n\
}\n\
\n\
type Query {\n\
\tping: String!\n\
\twidget(id: Int!): Widget!\n\
}\n\
\n\
type Widget {\n\
\tid: Int!\n\
\tlabel: String!\n\
}\n\
\n\
\"\"\"\n\
Directs the executor to include this field or fragment only when the `if` argument is true.\n\
\"\"\"\n\
directive @include(if: Boolean!) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT\n\
\"\"\"\n\
Directs the executor to skip this field or fragment when the `if` argument is true.\n\
\"\"\"\n\
directive @skip(if: Boolean!) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT\n\
schema {\n\
\tquery: Query\n\
\tmutation: Mutation\n\
}\n";

#[tokio::test]
async fn merged_schema_sdl_matches_committed_snapshot() {
    let path = snapshot_path("merged");
    let _ = std::fs::remove_file(&path);

    let _app = TestApp::builder()
        .module::<SnapshotApp>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the two-resolver schema boots and emits SDL");

    let sdl = std::fs::read_to_string(&path).expect("emit_sdl wrote the schema file at boot");
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        sdl, EXPECTED_SDL,
        "composed SDL drifted from the committed snapshot — see the async-graphql bump \
         procedure in the nest-rs-graphql crate doc before updating this constant",
    );
}

// ---------------------------------------------------------------------------
// What the emit says when it cannot write. `emit_sdl` is a *dev* convenience —
// the committed `schema.graphql` refreshed as a side effect of a run — so a
// failed write must never stop a boot that otherwise serves fine. Which leaves
// the event as the only signal, and a developer whose schema silently stopped
// refreshing has nothing else to go on: the app answers queries at the new
// shape while the committed SDL, and every client generated from it, stay at
// the old one.

/// A path under a directory that does not exist, so `std::fs::write` fails with
/// `NotFound` rather than on a permission the test would have to arrange.
fn unwritable_path() -> PathBuf {
    std::env::temp_dir()
        .join(format!("nest_rs_graphql_absent_{}", std::process::id()))
        .join("schema.graphql")
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        emit_sdl: true,
        schema_path: unwritable_path(),
        ..GraphqlConfig::default()
    }),
    AlphaModule,
])]
struct UnwritableSdlApp;

#[tokio::test]
async fn an_sdl_emit_that_cannot_write_warns_and_still_serves() {
    let logs = nest_rs_testing::LogCapture::install();

    let app = TestApp::builder()
        .module::<UnwritableSdlApp>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("a failed SDL write is a dev-loop annoyance, never a boot failure");

    let response = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping: widget(id: 1) { id } }" }))
        .send()
        .await;
    response.assert_status_is_ok();

    let event = logs.expect_one("nest_rs::graphql", "failed to write GraphQL SDL");
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("path")
            .is_some_and(|p| p.ends_with("schema.graphql")),
        "the event names the file that did not get written, got {:?}",
        event.fields,
    );
    assert!(
        event.field("error").is_some(),
        "and why, got {:?}",
        event.fields,
    );
    assert!(
        logs.find("nest_rs::graphql", "wrote GraphQL SDL")
            .is_empty(),
        "the success line and the failure line are exclusive: {:#?}",
        logs.events(),
    );
}

// ---------------------------------------------------------------------------
// The federated branch of `render_sdl`. `SDLExportOptions::federation()` is
// taken only when `GraphqlConfig::federation` is on, and the app above is not a
// subgraph — so the *committed* SDL of a federated app, which is the artefact a
// router's composition reads, had no test at all.

#[derive(SimpleObject)]
struct Gadget {
    id: i32,
    label: String,
}

#[resolver]
struct GadgetResolver;

#[operations]
impl GadgetResolver {
    #[query]
    #[public]
    async fn gadget(&self, id: i32) -> Gadget {
        Gadget {
            id,
            label: "gadget".into(),
        }
    }

    #[entity]
    #[public]
    async fn find_gadget_by_id(&self, id: i32) -> async_graphql::Result<Gadget> {
        Ok(Gadget {
            id,
            label: "by reference".into(),
        })
    }
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        emit_sdl: true,
        schema_path: snapshot_path("subgraph"),
        federation: true,
        ..GraphqlConfig::default()
    }),
])]
struct SubgraphSnapshotApp;

/// The committed **subgraph** form: `@key` on the federated type, and the two
/// fields a router calls (`_service` / `_entities`) stripped, as the Apollo spec
/// asks of an exported subgraph schema.
const EXPECTED_SUBGRAPH_SDL: &str = "\
type Gadget @key(fields: \"id\") {\n\
\tid: Int!\n\
\tlabel: String!\n\
}\n\
\n\
type Query {\n\
\tgadget(id: Int!): Gadget!\n\
}\n\
\n\
\"\"\"\n\
Directs the executor to include this field or fragment only when the `if` argument is true.\n\
\"\"\"\n\
directive @include(if: Boolean!) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT\n\
\"\"\"\n\
Directs the executor to skip this field or fragment when the `if` argument is true.\n\
\"\"\"\n\
directive @skip(if: Boolean!) on FIELD | FRAGMENT_SPREAD | INLINE_FRAGMENT\n\
extend schema @link(\n\
\turl: \"https://specs.apollo.dev/federation/v2.5\",\n\
\timport: [\"@key\", \"@tag\", \"@shareable\", \"@inaccessible\", \"@override\", \"@external\", \
\"@provides\", \"@requires\", \"@composeDirective\", \"@interfaceObject\", \"@requiresScopes\"]\n\
)\n";

#[tokio::test]
async fn subgraph_sdl_matches_committed_snapshot() {
    let path = snapshot_path("subgraph");
    let _ = std::fs::remove_file(&path);

    let app = TestApp::builder()
        .module::<SubgraphSnapshotApp>()
        .module::<GadgetModule>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("a subgraph boots and emits its SDL");

    let sdl = std::fs::read_to_string(&path).expect("emit_sdl wrote the schema file at boot");
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        sdl, EXPECTED_SUBGRAPH_SDL,
        "the committed subgraph SDL drifted — this is the artefact a router's \
         composition reads, so review the diff before updating the constant",
    );

    // And it is the same schema `_service` publishes, bar the newline the file
    // ends with — the two are rendered from one registry through two option
    // sets, and a reader has to be able to trust that.
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ _service { sdl } }" }))
        .send()
        .await;
    let body = resp.json().await.value().deserialize::<serde_json::Value>();
    let served = body["data"]["_service"]["sdl"]
        .as_str()
        .unwrap_or_else(|| panic!("a subgraph publishes its own SDL: {body}"));
    assert_eq!(
        served.trim_end(),
        sdl.trim_end(),
        "the committed subgraph SDL and the one `_service` serves are one schema",
    );
}

#[module(providers = [GadgetResolver])]
struct GadgetModule;
