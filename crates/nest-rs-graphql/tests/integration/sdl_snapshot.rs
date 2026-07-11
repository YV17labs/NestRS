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
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, resolver};
use nest_rs_http::HttpTransport;
use nest_rs_testing::TestApp;

#[derive(SimpleObject)]
struct Widget {
    id: i32,
    label: String,
}

#[resolver]
struct AlphaResolver;

#[resolver]
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

#[resolver]
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
fn snapshot_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "nest_rs_graphql_sdl_snapshot_{}.graphql",
        std::process::id()
    ))
}

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        emit_sdl: true,
        schema_path: snapshot_path(),
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
    let path = snapshot_path();
    let _ = std::fs::remove_file(&path);

    let _app = TestApp::builder()
        .module::<SnapshotApp>()
        .http(HttpTransport::new())
        .build()
        .await
        .expect("the two-resolver schema boots and emits SDL");

    let sdl = std::fs::read_to_string(&path).expect("emit_sdl wrote the schema file at boot");
    let _ = std::fs::remove_file(&path);

    if EXPECTED_SDL == "SNAPSHOT_PLACEHOLDER" {
        eprintln!("=====SDL-CAPTURE-BEGIN=====\n{sdl}\n=====SDL-CAPTURE-END=====");
    }

    assert_eq!(
        sdl, EXPECTED_SDL,
        "composed SDL drifted from the committed snapshot — see the async-graphql bump \
         procedure in the nest-rs-graphql crate doc before updating this constant",
    );
}
