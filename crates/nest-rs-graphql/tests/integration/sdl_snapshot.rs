//! Behavioural guard for the pinned async-graphql registry API: the drift the
//! compile-time canary in `src/resolver.rs` cannot see (`remove_unused_types`,
//! sorted SDL export), asserted byte-for-byte against the committed snapshot.

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

/// Per-process temp file the boot-time `emit_sdl` writes to: `render_sdl` is
/// `pub(crate)`, so the SDL is captured through the production path.
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
/// deliberately, via the bump procedure in the crate `//!` doc.
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

// A failed `emit_sdl` write never stops the boot, so its event is the only signal.

/// A path under a directory that does not exist, so `std::fs::write` fails.
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

// The federated branch of `render_sdl`, taken only when `GraphqlConfig::federation` is on.

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

/// The committed **subgraph** form: `@key` on the federated type, `_service` /
/// `_entities` stripped.
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

    // The same schema `_service` publishes, bar the file's trailing newline.
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
