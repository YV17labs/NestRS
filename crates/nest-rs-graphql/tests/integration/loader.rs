//! `#[dataloader]` — the batch a generated loader runs is the owner's method.

use std::collections::HashMap;
use std::sync::Arc;

use nest_rs_core::{AmbientValue, Layer, UnitContext, ambient, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::dataloader::{DataLoader, Loader};
use nest_rs_graphql::{GraphqlModule, GraphqlView, dataloader, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{EndpointGuards, HttpConfig, HttpModule, HttpView};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A trait whose method shares the batch's name, implemented for the `Arc` the
/// loader holds its owner in: method syntax finds it before it derefs.
#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait LabelsOnArc {
    fn labels(&self, keys: &[i32]) -> HashMap<i32, String>;
}

impl<T> LabelsOnArc for Arc<T> {
    fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        keys.iter()
            .map(|key| (*key, "the trait on Arc".into()))
            .collect()
    }
}

#[injectable]
#[derive(Default)]
struct Shelf;

#[dataloader]
impl Shelf {
    async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        keys.iter()
            .map(|key| (*key, format!("shelf {key}")))
            .collect()
    }
}

#[tokio::test]
async fn a_loader_runs_its_owners_method_and_not_a_trait_method_of_the_same_name() {
    let loaded = ShelfLabels(Arc::new(Shelf)).load(&[7]).await;
    let Ok(loaded) = loaded;
    assert_eq!(loaded.get(&7).map(String::as_str), Some("shelf 7"));
}

/// Who the request is for, as an ability guard says what the caller may do.
#[derive(Debug)]
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// Attaches the caller at the request, as the authorization guard attaches the
/// caller's ability.
#[injectable]
#[derive(Default)]
struct CallerGuard;

impl Layer for CallerGuard {}

#[async_trait]
impl Guard for CallerGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(Caller("alice"));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for CallerGuard {}

/// Answers each key with what its batch ran under.
#[injectable]
#[derive(Default)]
struct TraceShelf;

#[dataloader]
impl TraceShelf {
    async fn traces(&self, keys: &[i32]) -> HashMap<i32, String> {
        let seen = nest_rs_core::current_trace_id()
            .map(|trace| trace.to_hex())
            .unwrap_or_default();
        keys.iter().map(|key| (*key, seen.clone())).collect()
    }

    async fn callers(&self, keys: &[i32]) -> HashMap<i32, String> {
        let seen = ambient::<Caller>().map_or("nobody", |caller| caller.0);
        keys.iter().map(|key| (*key, seen.to_owned())).collect()
    }

    async fn spans(&self, keys: &[i32]) -> HashMap<i32, String> {
        let seen = tracing::Span::current()
            .metadata()
            .map_or("none", |metadata| metadata.name());
        keys.iter().map(|key| (*key, seen.to_owned())).collect()
    }
}

#[resolver]
struct TraceShelfResolver;

#[operations]
impl TraceShelfResolver {
    #[query]
    #[public]
    async fn batch_trace(&self, id: i32, loader: &DataLoader<TraceShelfTraces>) -> String {
        let Ok(found) = loader.load_one(id).await;
        found.unwrap_or_default()
    }

    #[query]
    #[public]
    async fn batch_caller(&self, id: i32, loader: &DataLoader<TraceShelfCallers>) -> String {
        let Ok(found) = loader.load_one(id).await;
        found.unwrap_or_default()
    }

    #[query]
    #[public]
    async fn batch_span(&self, id: i32, loader: &DataLoader<TraceShelfSpans>) -> String {
        let Ok(found) = loader.load_one(id).await;
        found.unwrap_or_default()
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [
        TraceShelf,
        TraceShelfResolver,
        CallerGuard,
        EndpointGuards<GraphqlView, (CallerGuard,)>,
    ],
)]
struct TraceShelfApp;

#[tokio::test]
async fn a_batch_runs_under_the_trace_of_the_request_that_triggered_it() {
    let app = nest_rs_testing::TestApp::for_module::<TraceShelfApp>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ batchTrace(id: 7) }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let request_trace = resp
        .0
        .headers()
        .get(nest_rs_http::TRACERESPONSE_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split('-').nth(1))
        .map(str::to_owned)
        .expect("the edge answers the trace it served the request under");
    let body: serde_json::Value = resp.json().await.value().deserialize();

    assert_eq!(
        body["data"]["batchTrace"].as_str(),
        Some(request_trace.as_str()),
        "the batch reads the request's trace id: {body}",
    );
}

#[tokio::test]
async fn a_batch_runs_in_its_requests_span_as_it_carries_its_requests_ids() {
    let _logs = nest_rs_testing::LogCapture::install();
    let app = nest_rs_testing::TestApp::for_module::<TraceShelfApp>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ batchSpan(id: 7) }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body: serde_json::Value = resp.json().await.value().deserialize();

    assert_eq!(
        body["data"]["batchSpan"].as_str(),
        Some(nest_rs_http::unit::REQUEST.name()),
        "the export nests the batch where its lines say it belongs, never under the \
         field that asked first: {body}",
    );
}

#[tokio::test]
async fn a_batch_reads_what_the_request_carries_with_no_batch_binding() {
    let app = nest_rs_testing::TestApp::for_module::<TraceShelfApp>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(
            &serde_json::json!({ "query": "{ a: batchCaller(id: 1) b: batchCaller(id: 2) }" }),
        )
        .send()
        .await;
    resp.assert_status_is_ok();
    let body: serde_json::Value = resp.json().await.value().deserialize();

    assert_eq!(body["data"]["a"].as_str(), Some("alice"), "{body}");
    assert_eq!(
        body["data"]["b"].as_str(),
        Some("alice"),
        "one batch serves both fields: {body}",
    );
}

/// Set when the slow batch is dropped, finished and started, in turn.
static SLOW_BATCH_DROPPED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SLOW_BATCH_FINISHED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SLOW_BATCH_STARTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct SetOnDrop(&'static std::sync::atomic::AtomicBool);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[injectable]
#[derive(Default)]
struct SlowShelf;

#[dataloader]
impl SlowShelf {
    async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
        let _dropped = SetOnDrop(&SLOW_BATCH_DROPPED);
        SLOW_BATCH_STARTED.notify_one();
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        SLOW_BATCH_FINISHED.store(true, std::sync::atomic::Ordering::SeqCst);
        keys.iter().map(|key| (*key, "late".into())).collect()
    }
}

#[resolver]
struct SlowShelfResolver;

#[operations]
impl SlowShelfResolver {
    #[query]
    #[public]
    async fn label(&self, id: i32, loader: &DataLoader<SlowShelfLabels>) -> String {
        let Ok(found) = loader.load_one(id).await;
        found.unwrap_or_default()
    }
}

#[module(
    imports = [
        GraphqlModule,
        HttpModule::for_root(HttpConfig {
            shutdown_timeout: std::time::Duration::from_secs(1),
            ..Default::default()
        }),
    ],
    providers = [SlowShelf, SlowShelfResolver],
)]
struct SlowShelfApp;

#[tokio::test]
async fn a_batch_still_running_when_the_transport_stops_is_dropped_with_it() {
    let app = nest_rs_testing::TestApp::builder()
        .module::<SlowShelfApp>()
        .build_ws()
        .await
        .expect("the schema boots on a real port");
    let mut client = connect(app.addr()).await;
    let body = r#"{"query":"{ label(id: 7) }"}"#;
    client
        .write_all(
            format!(
                "POST /graphql HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\n\r\n{body}",
                body.len(),
            )
            .as_bytes(),
        )
        .await
        .expect("the request is sent");
    SLOW_BATCH_STARTED.notified().await;

    tokio::time::pause();
    app.shutdown().await.expect("the transport stops cleanly");
    tokio::time::resume();

    assert!(
        SLOW_BATCH_DROPPED.load(std::sync::atomic::Ordering::SeqCst)
            && !SLOW_BATCH_FINISHED.load(std::sync::atomic::Ordering::SeqCst),
        "the batch was dropped where it waited before the transport returned",
    );
    let mut rest = Vec::new();
    let _ = client.read_to_end(&mut rest).await;
    assert!(rest.is_empty(), "the cut request was not answered");
}

/// A connection to the served address, retried while the listener comes up.
async fn connect(addr: std::net::SocketAddr) -> tokio::net::TcpStream {
    for _ in 0..100 {
        if let Ok(stream) = tokio::net::TcpStream::connect(addr).await {
            return stream;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the transport never came up on {addr}");
}

/// A batch runs off its field's task: a panic in it is contained there and
/// said once on this crate's target, as a field's would be.
mod panicking_batch {
    use std::collections::HashMap;

    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::dataloader::DataLoader;
    use nest_rs_graphql::{GraphqlModule, dataloader, operations, resolver};
    use nest_rs_testing::{LogCapture, TestApp};

    #[injectable]
    #[derive(Default)]
    struct BrokenShelf;

    #[dataloader]
    impl BrokenShelf {
        async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
            tokio::task::yield_now().await;
            panic!("the shelf at {} broke", keys.len())
        }
    }

    #[resolver]
    struct BrokenShelfResolver;

    #[operations]
    impl BrokenShelfResolver {
        #[query]
        #[public]
        async fn broken_label(&self, id: i32, loader: &DataLoader<BrokenShelfLabels>) -> String {
            let Ok(found) = loader.load_one(id).await;
            found.unwrap_or_default()
        }
    }

    #[module(
        imports = [GraphqlModule],
        providers = [BrokenShelf, BrokenShelfResolver],
    )]
    struct BrokenShelfApp;

    #[tokio::test]
    async fn a_batch_that_panics_is_contained_and_said_once_on_the_graphql_target() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<BrokenShelfApp>()
            .await
            .expect("the schema boots");

        let _ = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": "{ brokenLabel(id: 7) }" }))
            .send()
            .await;

        let said = logs.expect_one(
            nest_rs_graphql::TARGET,
            "a DataLoader batch panicked; the loads it served answer no value",
        );
        assert_eq!(said.level, "error");
        assert!(
            said.field(nest_rs_core::panic::FIELD)
                .is_some_and(|panic| panic.contains("the shelf at 1 broke")),
            "{:?}",
            said.fields,
        );
    }
}

/// A `#[field_resolver]` reads a loader through a `&DataLoader<L>` parameter,
/// which declares the need: the schema build checks it as it checks an exposed
/// relation.
mod field_parameter {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::SimpleObject;
    use nest_rs_graphql::async_graphql::dataloader::DataLoader;
    use nest_rs_graphql::{GraphqlModule, dataloader, operations, resolver};
    use nest_rs_testing::TestApp;

    #[derive(Clone, SimpleObject)]
    #[graphql(complex)]
    struct Book {
        id: i32,
    }

    #[injectable]
    #[derive(Default)]
    struct Catalogue {
        batches: AtomicUsize,
    }

    #[dataloader]
    impl Catalogue {
        async fn labels(&self, keys: &[i32]) -> HashMap<i32, String> {
            self.batches.fetch_add(1, Ordering::SeqCst);
            keys.iter()
                .map(|key| (*key, format!("book {key}")))
                .collect()
        }
    }

    #[resolver]
    struct BooksResolver;

    #[operations]
    impl BooksResolver {
        #[query]
        #[public]
        async fn books(&self) -> Vec<Book> {
            vec![Book { id: 1 }, Book { id: 2 }]
        }

        #[field_resolver]
        async fn label(&self, book: &Book, labels: &DataLoader<CatalogueLabels>) -> String {
            let Ok(found) = labels.load_one(book.id).await;
            found.unwrap_or_default()
        }
    }

    #[module(imports = [GraphqlModule], providers = [Catalogue, BooksResolver])]
    struct BooksApp;

    #[tokio::test]
    async fn a_loader_parameter_batches_every_parent_of_one_response() {
        let app = TestApp::for_module::<BooksApp>()
            .await
            .expect("the schema boots");

        let body = crate::graphql_answer(&app, "{ books { label } }").await;

        assert_eq!(
            body["data"]["books"],
            serde_json::json!([{ "label": "book 1" }, { "label": "book 2" }]),
            "{body}",
        );
        let catalogue = app.container().get::<Catalogue>().expect("provided");
        assert_eq!(catalogue.batches.load(Ordering::SeqCst), 1, "one batch");
    }

    #[derive(Clone, SimpleObject)]
    #[graphql(complex)]
    struct Volume {
        id: i32,
    }

    #[injectable]
    #[derive(Default)]
    struct Archive;

    #[dataloader]
    impl Archive {
        async fn shelves(&self, keys: &[i32]) -> HashMap<i32, String> {
            keys.iter()
                .map(|key| (*key, format!("shelf {key}")))
                .collect()
        }
    }

    #[module(providers = [Archive])]
    struct ArchiveModule;

    #[resolver]
    struct VolumesResolver;

    #[operations]
    impl VolumesResolver {
        #[query]
        #[public]
        async fn volumes(&self) -> Vec<Volume> {
            vec![Volume { id: 1 }]
        }

        #[field_resolver]
        #[graphql(name = "shelfLabel")]
        async fn shelf(&self, volume: &Volume, shelves: &DataLoader<ArchiveShelves>) -> String {
            let Ok(found) = shelves.load_one(volume.id).await;
            found.unwrap_or_default()
        }
    }

    #[module(imports = [GraphqlModule], providers = [VolumesResolver])]
    struct VolumesGraphqlModule;

    #[tokio::test]
    async fn a_served_field_whose_loader_owner_is_not_reached_is_refused_naming_the_import() {
        let Err(refused) = TestApp::for_module::<VolumesGraphqlModule>().await else {
            panic!("every query selecting `shelfLabel` would fail");
        };
        let refused = nest_rs_core::error_message(refused.as_ref());

        for named in ["`Volume.shelfLabel`", "Archive", "import `ArchiveModule`"] {
            assert!(refused.contains(named), "names {named}: {refused}");
        }
    }

    #[module(imports = [VolumesGraphqlModule, ArchiveModule])]
    struct ShelvedApp;

    #[derive(Clone, SimpleObject)]
    #[graphql(complex)]
    struct Binding {
        id: i32,
    }

    #[derive(Clone, SimpleObject)]
    struct Folio {
        #[graphql(flatten)]
        binding: Binding,
    }

    #[resolver]
    struct FoliosResolver;

    #[operations]
    impl FoliosResolver {
        #[query]
        #[public]
        async fn folio(&self) -> Folio {
            Folio {
                binding: Binding { id: 1 },
            }
        }

        #[field_resolver]
        async fn shelf(&self, binding: &Binding, shelves: &DataLoader<ArchiveShelves>) -> String {
            let Ok(found) = shelves.load_one(binding.id).await;
            found.unwrap_or_default()
        }
    }

    #[module(imports = [GraphqlModule], providers = [FoliosResolver])]
    struct FoliosApp;

    #[tokio::test]
    async fn a_flattened_parents_field_whose_loader_owner_is_not_reached_is_refused() {
        let Err(refused) = TestApp::for_module::<FoliosApp>().await else {
            panic!("a flattened object's field is served like any other");
        };
        let refused = nest_rs_core::error_message(refused.as_ref());

        for named in ["`Binding.shelf`", "import `ArchiveModule`"] {
            assert!(refused.contains(named), "names {named}: {refused}");
        }
    }

    #[tokio::test]
    async fn a_served_field_whose_loader_owner_is_imported_boots_and_reads_it() {
        let app = TestApp::for_module::<ShelvedApp>()
            .await
            .expect("the loader `shelfLabel` reads is reached");

        let body = crate::graphql_answer(&app, "{ volumes { shelfLabel } }").await;

        assert_eq!(
            body["data"]["volumes"][0]["shelfLabel"], "shelf 1",
            "{body}"
        );
    }
}

/// A root field reads a loader through a `&DataLoader<L>` parameter as a
/// `#[field_resolver]` does: no argument of the schema, one batch across its
/// selections, and a need the schema build checks.
mod root_parameter {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::dataloader::DataLoader;
    use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
    use nest_rs_graphql::async_graphql::{self, SimpleObject};
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, dataloader, operations, resolver};
    use nest_rs_testing::TestApp;

    #[injectable]
    #[derive(Default)]
    struct Almanac {
        batches: AtomicUsize,
    }

    #[dataloader]
    impl Almanac {
        async fn entries(&self, keys: &[i32]) -> HashMap<i32, String> {
            self.batches.fetch_add(1, Ordering::SeqCst);
            keys.iter()
                .map(|key| (*key, format!("entry {key}")))
                .collect()
        }
    }

    #[module(providers = [Almanac])]
    struct AlmanacModule;

    #[resolver]
    struct EntryResolver;

    #[operations]
    impl EntryResolver {
        #[query]
        #[public]
        async fn entry(&self, id: i32, entries: &DataLoader<AlmanacEntries>) -> String {
            let Ok(found) = entries.load_one(id).await;
            found.unwrap_or_default()
        }

        #[mutation]
        #[public]
        async fn touch_entry(
            &self,
            id: i32,
            entries: &DataLoader<AlmanacEntries>,
        ) -> async_graphql::Result<String> {
            let Ok(found) = entries.load_one(id).await;
            Ok(found.unwrap_or_default())
        }
    }

    #[resolver]
    struct EntryFeedResolver;

    #[operations]
    impl EntryFeedResolver {
        #[query]
        #[public]
        async fn entry_feed_up(&self) -> bool {
            true
        }

        #[subscription]
        #[public]
        async fn entry_feed(
            &self,
            id: i32,
            entries: &DataLoader<AlmanacEntries>,
        ) -> impl Stream<Item = String> {
            let Ok(found) = entries.load_one(id).await;
            stream::iter(found)
        }
    }

    #[module(
        imports = [GraphqlModule, AlmanacModule],
        providers = [EntryResolver, EntryFeedResolver],
    )]
    struct EntryApp;

    #[tokio::test]
    async fn a_root_fields_loader_parameter_is_no_argument_and_batches_its_selections() {
        let app = TestApp::for_module::<EntryApp>()
            .await
            .expect("the owner is reached");

        let body = crate::graphql_answer(&app, "{ a: entry(id: 1) b: entry(id: 2) }").await;

        assert_eq!(
            body["data"],
            serde_json::json!({ "a": "entry 1", "b": "entry 2" }),
            "{body}",
        );
        let almanac = app.container().get::<Almanac>().expect("provided");
        assert_eq!(almanac.batches.load(Ordering::SeqCst), 1, "one batch");

        let arguments = crate::graphql_answer(&app, r#"{ entry(id: 1, entries: "sent") }"#).await;
        assert_eq!(
            arguments["errors"][0]["message"],
            r#"Unknown argument "entries" on field "entry" of type "Query"."#,
            "{arguments}",
        );
    }

    #[tokio::test]
    async fn a_mutations_loader_parameter_reads_it() {
        let app = TestApp::for_module::<EntryApp>()
            .await
            .expect("the owner is reached");

        let body = crate::graphql_answer(&app, "mutation { touchEntry(id: 3) }").await;

        assert_eq!(body["data"]["touchEntry"], "entry 3", "{body}");
    }

    #[tokio::test]
    async fn a_subscriptions_loader_parameter_reads_it_at_its_opening() {
        let app = TestApp::for_module::<EntryApp>()
            .await
            .expect("the owner is reached");
        let mut socket = app.graphql_socket().open();
        socket.connect().await;

        socket.subscribe("feed", "subscription { entryFeed(id: 4) }");
        let item = socket.next_item("feed").await.expect("the item");

        assert_eq!(item["data"]["entryFeed"], "entry 4", "{item}");
    }

    #[derive(Clone, SimpleObject)]
    struct Chapter {
        id: i32,
        title: String,
    }

    #[resolver]
    struct ChapterResolver;

    #[operations]
    impl ChapterResolver {
        #[query]
        #[public]
        async fn chapter_of_the_day(&self) -> Chapter {
            Chapter {
                id: 1,
                title: "the first".to_owned(),
            }
        }

        #[entity]
        #[public]
        async fn find_chapter_by_id(
            &self,
            id: i32,
            entries: &DataLoader<AlmanacEntries>,
        ) -> async_graphql::Result<Chapter> {
            let Ok(found) = entries.load_one(id).await;
            Ok(Chapter {
                id,
                title: found.unwrap_or_default(),
            })
        }
    }

    #[module(
        imports = [
            GraphqlModule::for_root(GraphqlConfig {
                federation: true,
                ..GraphqlConfig::default()
            }),
            AlmanacModule,
        ],
        providers = [ChapterResolver],
    )]
    struct ChapterApp;

    #[tokio::test]
    async fn an_entitys_loader_parameter_joins_no_key_and_reads_it() {
        let app = TestApp::for_module::<ChapterApp>()
            .await
            .expect("the owner is reached");

        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({
                "query": "query($reps: [_Any!]!) { \
                          _entities(representations: $reps) { ... on Chapter { title } } }",
                "variables": { "reps": [{ "__typename": "Chapter", "id": 5 }] },
            }))
            .send()
            .await;
        let body: serde_json::Value = resp.json().await.value().deserialize();

        assert_eq!(body["data"]["_entities"][0]["title"], "entry 5", "{body}");
    }

    /// Each role's resolver, its loader's owner in no module its app imports.
    mod unreached {
        use std::collections::HashMap;

        use nest_rs_core::{injectable, module};
        use nest_rs_graphql::async_graphql::dataloader::DataLoader;
        use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
        use nest_rs_graphql::async_graphql::{self, SimpleObject};
        use nest_rs_graphql::{GraphqlConfig, GraphqlModule, dataloader, operations, resolver};
        use nest_rs_testing::TestApp;

        #[injectable]
        #[derive(Default)]
        struct Ledger;

        #[dataloader]
        impl Ledger {
            async fn lines(&self, keys: &[i32]) -> HashMap<i32, String> {
                keys.iter().map(|key| (*key, String::new())).collect()
            }
        }

        #[module(providers = [Ledger])]
        struct LedgerModule;

        #[resolver]
        struct QueryReader;

        #[operations]
        impl QueryReader {
            #[query]
            #[public]
            async fn line(&self, id: i32, lines: &DataLoader<LedgerLines>) -> String {
                let Ok(found) = lines.load_one(id).await;
                found.unwrap_or_default()
            }
        }

        #[module(imports = [GraphqlModule], providers = [QueryReader])]
        struct QueryReaderApp;

        #[resolver]
        struct MutationReader;

        #[operations]
        impl MutationReader {
            #[query]
            #[public]
            async fn mutation_reader_up(&self) -> bool {
                true
            }

            #[mutation]
            #[public]
            async fn post_line(&self, id: i32, lines: &DataLoader<LedgerLines>) -> String {
                let Ok(found) = lines.load_one(id).await;
                found.unwrap_or_default()
            }
        }

        #[module(imports = [GraphqlModule], providers = [MutationReader])]
        struct MutationReaderApp;

        #[resolver]
        struct SubscriptionReader;

        #[operations]
        impl SubscriptionReader {
            #[query]
            #[public]
            async fn subscription_reader_up(&self) -> bool {
                true
            }

            #[subscription]
            #[public]
            async fn line_feed(
                &self,
                id: i32,
                lines: &DataLoader<LedgerLines>,
            ) -> impl Stream<Item = String> {
                let Ok(found) = lines.load_one(id).await;
                stream::iter(found)
            }
        }

        #[module(imports = [GraphqlModule], providers = [SubscriptionReader])]
        struct SubscriptionReaderApp;

        #[derive(Clone, SimpleObject)]
        struct Line {
            id: i32,
        }

        #[resolver]
        struct EntityReader;

        #[operations]
        impl EntityReader {
            #[query]
            #[public]
            async fn entity_reader_up(&self) -> Line {
                Line { id: 1 }
            }

            #[entity]
            #[public]
            async fn find_line_by_id(
                &self,
                id: i32,
                lines: &DataLoader<LedgerLines>,
            ) -> async_graphql::Result<Line> {
                let Ok(_) = lines.load_one(id).await;
                Ok(Line { id })
            }
        }

        #[module(
            imports = [GraphqlModule::for_root(GraphqlConfig {
                federation: true,
                ..GraphqlConfig::default()
            })],
            providers = [EntityReader],
        )]
        struct EntityReaderApp;

        async fn refusal_of<M: nest_rs_core::Module + 'static>() -> String {
            match TestApp::for_module::<M>().await {
                Err(refused) => nest_rs_core::error_message(refused.as_ref()),
                Ok(_) => panic!("every operation reading the loader would fail"),
            }
        }

        #[tokio::test]
        async fn each_root_role_whose_loader_owner_is_not_reached_is_refused_naming_the_import() {
            for (refused, field, resolver) in [
                (
                    refusal_of::<QueryReaderApp>().await,
                    "`query line`",
                    "QueryReader",
                ),
                (
                    refusal_of::<MutationReaderApp>().await,
                    "`mutation post_line`",
                    "MutationReader",
                ),
                (
                    refusal_of::<SubscriptionReaderApp>().await,
                    "`subscription line_feed`",
                    "SubscriptionReader",
                ),
                (
                    refusal_of::<EntityReaderApp>().await,
                    "`entity find_line_by_id`",
                    "EntityReader",
                ),
            ] {
                let declared = format!("(declared on `{resolver}`)");
                let import = format!("import `LedgerModule` into the module serving `{resolver}`");
                for named in [field, &declared, "Ledger", &import] {
                    assert!(refused.contains(named), "names {named}: {refused}");
                }
            }
        }

        #[module(imports = [QueryReaderApp, LedgerModule])]
        struct LedgerApp;

        #[tokio::test]
        async fn a_root_field_whose_loader_owner_is_imported_boots() {
            let app = TestApp::for_module::<LedgerApp>()
                .await
                .expect("the loader `line` reads is reached");

            let body = crate::graphql_answer(&app, "{ line(id: 1) }").await;

            assert_eq!(body["data"]["line"], "", "{body}");
        }

        /// An app that does not reach the resolver does not serve the field, so
        /// the need is not its own.
        #[tokio::test]
        async fn a_root_field_whose_resolver_is_not_reached_refuses_nothing() {
            #[resolver]
            struct Elsewhere;

            #[operations]
            impl Elsewhere {
                #[query]
                #[public]
                async fn elsewhere_up(&self) -> bool {
                    true
                }
            }

            #[module(imports = [GraphqlModule], providers = [Elsewhere])]
            struct ElsewhereApp;

            TestApp::for_module::<ElsewhereApp>()
                .await
                .expect("it serves no field reading a loader");
        }
    }
}

/// A body reading its loader off the context by hand is unreadable by the boot:
/// nothing refuses it, so the request does. The client reads no Rust type.
mod hand_read {
    use std::collections::HashMap;

    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::dataloader::DataLoader;
    use nest_rs_graphql::async_graphql::{self, Context};
    use nest_rs_graphql::{GraphqlModule, dataloader, operations, resolver};
    use nest_rs_testing::{LogCapture, TestApp};

    #[injectable]
    #[derive(Default)]
    struct Cellar;

    #[dataloader]
    impl Cellar {
        async fn bottles(&self, keys: &[i32]) -> HashMap<i32, String> {
            keys.iter().map(|key| (*key, String::new())).collect()
        }
    }

    #[resolver]
    struct BottleResolver;

    #[operations]
    impl BottleResolver {
        #[query]
        #[public]
        async fn bottle(&self, ctx: &Context<'_>, id: i32) -> async_graphql::Result<String> {
            let loader = ctx.data::<DataLoader<CellarBottles>>()?;
            let Ok(found) = loader.load_one(id).await;
            Ok(found.unwrap_or_default())
        }
    }

    #[module(imports = [GraphqlModule], providers = [BottleResolver])]
    struct BottleApp;

    #[tokio::test]
    async fn a_type_a_context_does_not_hold_answers_opaquely_and_tells_the_operator() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<BottleApp>()
            .await
            .expect("the boot cannot read a body");

        let body = crate::graphql_answer(&app, "{ aliasTheClientWrote: bottle(id: 1) }").await;

        assert_eq!(
            body["errors"][0]["message"],
            nest_rs_core::OPAQUE_CLIENT_MESSAGE,
            "{body}",
        );
        assert_eq!(
            body["errors"][0]["extensions"]["code"], "INTERNAL",
            "{body}"
        );
        assert!(!body.to_string().contains("DataLoader"), "{body}");
        let filed = logs.expect_one("nest_rs::graphql", "graphql operation failed");
        let said = filed.field("error").unwrap_or_default();
        assert!(said.contains("CellarBottles"), "names the type: {said}");
        assert!(
            !said.contains("aliasTheClientWrote"),
            "no name the client wrote reaches a line: {said}",
        );
    }

    /// A deliberate error that only looks like it opens the same way is left
    /// as built.
    #[tokio::test]
    async fn a_deliberate_error_is_not_read_as_one() {
        #[resolver]
        struct DeliberateResolver;

        #[operations]
        impl DeliberateResolver {
            #[query]
            #[public]
            async fn deliberate(&self) -> async_graphql::Result<i32> {
                Err("Data `the archive` is on the way.".into())
            }
        }

        #[module(imports = [GraphqlModule], providers = [DeliberateResolver])]
        struct DeliberateApp;

        let app = TestApp::for_module::<DeliberateApp>()
            .await
            .expect("the schema boots");

        let body = crate::graphql_answer(&app, "{ deliberate }").await;

        assert_eq!(
            body["errors"][0]["message"], "Data `the archive` is on the way.",
            "{body}",
        );
    }
}
