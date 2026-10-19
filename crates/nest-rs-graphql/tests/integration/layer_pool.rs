//! The layer-pool contract on the `/graphql` self-mount: a global guard runs
//! **exactly once** per request, the guards a module declares for the
//! endpoint **add to** the global pool, a guard in both running once, and a
//! unit-stage interceptor runs around the request.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{EndpointGuards, async_trait};
use nest_rs_testing::TestApp;
use tokio::sync::Mutex;

static GUARD_RUNS: AtomicUsize = AtomicUsize::new(0);
static INTERCEPTOR_RUNS: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counters() {
    GUARD_RUNS.store(0, Ordering::SeqCst);
    INTERCEPTOR_RUNS.store(0, Ordering::SeqCst);
}

/// Counts every `check_http` execution, then admits the request.
#[injectable]
#[derive(Default)]
struct CountingPassGuard;

impl Layer for CountingPassGuard {}

#[async_trait]
impl Guard for CountingPassGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        GUARD_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for CountingPassGuard {}

/// Counts every unit it runs around, then forwards.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct CountingInterceptor;

impl Layer for CountingInterceptor {}

#[async_trait]
impl Interceptor for CountingInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        INTERCEPTOR_RUNS.fetch_add(1, Ordering::SeqCst);
        next.run(cx).await
    }
}

#[resolver]
struct PingResolver;

#[operations]
impl PingResolver {
    #[query]
    #[public]
    async fn ping(&self) -> GqlResult<String> {
        Ok("pong".into())
    }
}

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [CountingPassGuard, CountingInterceptor, PingResolver],
)]
struct FallbackModule;

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        CountingPassGuard,
        PingResolver,
        EndpointGuards<GraphqlView, (CountingPassGuard,)>,
    ],
)]
struct DeclaredModule;

async fn post_ping(app: &TestApp) {
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ ping }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    assert_eq!(
        json.value()
            .object()
            .get("data")
            .object()
            .get("ping")
            .string(),
        "pong",
    );
}

#[tokio::test]
async fn global_guard_runs_exactly_once_per_graphql_request() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<FallbackModule>()
        .use_guards_global([guard::<CountingPassGuard>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        GUARD_RUNS.load(Ordering::SeqCst),
        1,
        "the global guard pool runs at the request and nowhere else — the double-run \
         must never come back",
    );
}

#[tokio::test]
async fn a_guard_both_pooled_and_declared_for_the_endpoint_runs_once() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::builder()
        .module::<DeclaredModule>()
        .use_guards_global([guard::<CountingPassGuard>()])
        .build()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        GUARD_RUNS.load(Ordering::SeqCst),
        1,
        "the endpoint's guards join the global pool — a guard in both runs once",
    );
}

#[tokio::test]
async fn a_guard_declared_for_the_endpoint_runs_without_a_pool() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::for_module::<DeclaredModule>()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(GUARD_RUNS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_unit_stage_interceptor_runs_around_the_graphql_post() {
    let _gate = GATE.lock().await;
    reset_counters();

    let app = TestApp::for_module::<FallbackModule>()
        .await
        .expect("boots");

    post_ping(&app).await;
    assert_eq!(
        INTERCEPTOR_RUNS.load(Ordering::SeqCst),
        1,
        "a self-mount's request is a unit of the HTTP edge: the unit stage runs there",
    );
}

/// The interceptors and exception filters a resolver binds, and the app's own,
/// around its root fields and its field resolvers.
mod pipeline {
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nest_rs_core::problem::code;
    use nest_rs_core::{
        Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, Problem, UnitContext,
        exception_filter, injectable, interceptor, module,
    };
    use nest_rs_graphql::async_graphql::futures_util::stream::{self, Stream};
    use nest_rs_graphql::async_graphql::{Result as GqlResult, SimpleObject};
    use nest_rs_graphql::{FieldRole, GraphqlModule, GraphqlView, operations, resolver};
    use nest_rs_http::async_trait;
    use nest_rs_testing::TestApp;
    use tokio::sync::broadcast;

    static AROUND_ROOT_FIELDS: AtomicUsize = AtomicUsize::new(0);
    static AROUND_LABELS: AtomicUsize = AtomicUsize::new(0);

    /// What a global interceptor saw of each unit it ran around: its unit,
    /// the field and the role the view names.
    static SEEN: StdMutex<Vec<(&'static str, &'static str, FieldRole)>> = StdMutex::new(Vec::new());

    /// Counts the root fields its resolver binds it on.
    #[injectable]
    #[derive(Default)]
    struct AroundRootFields;

    impl Layer for AroundRootFields {}

    #[async_trait]
    impl Interceptor for AroundRootFields {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            AROUND_ROOT_FIELDS.fetch_add(1, Ordering::SeqCst);
            next.run(cx).await
        }
    }

    /// Counts each call of the field resolver it is bound on.
    #[injectable]
    #[derive(Default)]
    struct AroundLabels;

    impl Layer for AroundLabels {}

    #[async_trait]
    impl Interceptor for AroundLabels {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            AROUND_LABELS.fetch_add(1, Ordering::SeqCst);
            next.run(cx).await
        }
    }

    /// Records what the app's own interceptor reads of each unit.
    #[injectable]
    #[derive(Default)]
    struct Recorder;

    impl Layer for Recorder {}

    #[async_trait]
    impl Interceptor for Recorder {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            let seen = cx
                .switch_to::<GraphqlView>()
                .map(|view| (cx.unit().name(), view.field(), view.role()));
            if let Some(seen) = seen {
                SEEN.lock().unwrap().push(seen);
            }
            next.run(cx).await
        }
    }

    /// The ledger refusing a write: a domain error the client is owed no
    /// explanation for, unless a filter says otherwise.
    #[derive(Debug)]
    struct LedgerRefused;

    impl std::fmt::Display for LedgerRefused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the ledger at 10.0.0.9 refused the write")
        }
    }

    impl std::error::Error for LedgerRefused {}

    /// Answers a refused write as the conflict it is.
    #[injectable]
    #[derive(Default)]
    struct LedgerConflict;

    impl Layer for LedgerConflict {}

    #[async_trait]
    impl ExceptionFilter for LedgerConflict {
        type Exception = LedgerRefused;

        async fn catch(&self, _: &LedgerRefused, _: &mut UnitContext<'_>) -> Caught {
            Caught::Problem(Problem::new(409, code::CONFLICT).with_detail("the ledger is busy"))
        }
    }

    #[derive(SimpleObject)]
    #[graphql(complex)]
    struct Shelf {
        id: i32,
    }

    #[resolver]
    #[use_interceptors(AroundRootFields)]
    struct ShelfResolver;

    #[operations]
    impl ShelfResolver {
        #[query]
        #[public]
        async fn shelves(&self) -> Vec<Shelf> {
            (1..=3).map(|id| Shelf { id }).collect()
        }

        #[query]
        #[public]
        async fn count(&self) -> i32 {
            3
        }

        #[mutation]
        #[public]
        #[use_exception_filters(LedgerConflict)]
        async fn write(&self) -> Result<bool, LedgerRefused> {
            Err(LedgerRefused)
        }

        #[mutation]
        #[public]
        async fn touch(&self) -> GqlResult<bool> {
            Ok(true)
        }

        #[field_resolver]
        #[use_interceptors(AroundLabels)]
        async fn label(&self, parent: &Shelf) -> String {
            format!("shelf {}", parent.id)
        }

        #[field_resolver]
        #[use_exception_filters(LedgerConflict)]
        async fn balance(&self, parent: &Shelf) -> Result<i32, LedgerRefused> {
            match parent.id {
                2 => Err(LedgerRefused),
                id => Ok(id),
            }
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [AroundRootFields, AroundLabels, LedgerConflict, Recorder, ShelfResolver],
    )]
    struct ShelfModule;

    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn post(app: &TestApp, query: &str) -> serde_json::Value {
        let resp = app
            .http()
            .post("/graphql")
            .body_json(&serde_json::json!({ "query": query }))
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.json().await.value().deserialize()
    }

    #[tokio::test]
    async fn a_resolvers_interceptor_wraps_each_root_field_once_and_a_field_resolver_only_where_declared_on_it()
     {
        let _serial = SERIAL.lock().await;
        AROUND_ROOT_FIELDS.store(0, Ordering::SeqCst);
        AROUND_LABELS.store(0, Ordering::SeqCst);
        let app = TestApp::for_module::<ShelfModule>().await.expect("boots");

        let body = post(&app, "{ count shelves { label } }").await;

        assert_eq!(body["data"]["shelves"][2]["label"], "shelf 3", "{body}");
        assert_eq!(
            AROUND_ROOT_FIELDS.load(Ordering::SeqCst),
            2,
            "once per root field, never per parent row: {body}",
        );
        assert_eq!(
            AROUND_LABELS.load(Ordering::SeqCst),
            3,
            "the field resolver's own interceptor, once per parent: {body}",
        );
    }

    #[tokio::test]
    async fn the_apps_interceptor_runs_around_each_root_field_as_a_graphql_field_unit() {
        let _serial = SERIAL.lock().await;
        SEEN.lock().unwrap().clear();
        let app = TestApp::builder()
            .module::<ShelfModule>()
            .use_interceptors_global([interceptor::<Recorder>()])
            .build()
            .await
            .expect("boots");

        let body = post(&app, "{ count shelves { label } }").await;

        assert_eq!(body["data"]["count"], 3, "{body}");
        let mut seen = SEEN.lock().unwrap().clone();
        seen.sort_by_key(|(_, field, _)| *field);
        assert_eq!(
            seen,
            [
                ("graphql.field", "count", FieldRole::RootField),
                ("graphql.field", "shelves", FieldRole::RootField),
            ],
            "one run per root field, under its unit and its role — none at the request, \
             none per parent row",
        );
        assert_eq!(
            nest_rs_graphql::unit::FIELD.name(),
            "graphql.field",
            "the unit a global interceptor reads",
        );
    }

    #[tokio::test]
    async fn an_exception_filter_answers_a_domain_error_as_a_field_error_while_its_siblings_resolve()
     {
        let _serial = SERIAL.lock().await;
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<ShelfModule>().await.expect("boots");

        let body = post(&app, "mutation { write touch }").await;

        assert_eq!(body["data"]["touch"], true, "the sibling resolved: {body}");
        assert!(body["data"]["write"].is_null(), "{body}");
        let error = &body["errors"][0];
        assert_eq!(error["message"], "the ledger is busy", "{body}");
        assert_eq!(error["extensions"]["code"], "CONFLICT", "{body}");
        assert_eq!(error["path"], serde_json::json!(["write"]), "{body}");
        assert!(!body.to_string().contains("10.0.0.9"), "{body}");
        assert!(
            logs.find("nest_rs::graphql", "graphql operation failed")
                .is_empty(),
            "a failure a filter answered 4xx files nothing",
        );
    }

    #[tokio::test]
    async fn a_field_resolvers_own_exception_filter_answers_its_parent_row() {
        let _serial = SERIAL.lock().await;
        let app = TestApp::for_module::<ShelfModule>().await.expect("boots");

        let body = post(&app, "{ shelves { id balance } }").await;

        assert_eq!(body["data"]["shelves"][0]["balance"], 1, "{body}");
        assert!(body["data"]["shelves"][1]["balance"].is_null(), "{body}");
        let error = &body["errors"][0];
        assert_eq!(error["extensions"]["code"], "CONFLICT", "{body}");
        assert_eq!(
            error["path"],
            serde_json::json!(["shelves", 1, "balance"]),
            "{body}"
        );
        assert!(!body.to_string().contains("10.0.0.9"), "{body}");
    }

    static MODULE_WIDE: AtomicUsize = AtomicUsize::new(0);

    /// A module's own `#[interceptor]`: app-wide, at the handler stage.
    #[interceptor]
    #[derive(Default)]
    struct ModuleWide;

    impl Layer for ModuleWide {}

    #[async_trait]
    impl Interceptor for ModuleWide {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            MODULE_WIDE.fetch_add(1, Ordering::SeqCst);
            next.run(cx).await
        }
    }

    #[module(
        imports = [ShelfModule],
        providers = [ModuleWide],
    )]
    struct ModuleWideRoot;

    #[tokio::test]
    async fn a_modules_interceptor_runs_around_each_root_field_once() {
        let _serial = SERIAL.lock().await;
        MODULE_WIDE.store(0, Ordering::SeqCst);
        let app = TestApp::for_module::<ModuleWideRoot>()
            .await
            .expect("boots");

        let body = post(&app, "{ count shelves { label } }").await;

        assert_eq!(body["data"]["count"], 3, "{body}");
        assert_eq!(
            MODULE_WIDE.load(Ordering::SeqCst),
            2,
            "once per root field — none at the request, none per parent row: {body}",
        );
    }

    static OPENINGS: AtomicUsize = AtomicUsize::new(0);

    /// Counts each subscription opening it runs around.
    #[injectable]
    #[derive(Default)]
    struct CountOpenings;

    impl Layer for CountOpenings {}

    #[async_trait]
    impl Interceptor for CountOpenings {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            OPENINGS.fetch_add(1, Ordering::SeqCst);
            next.run(cx).await
        }
    }

    #[derive(Clone, Debug, SimpleObject)]
    struct Entry {
        n: i32,
    }

    #[injectable]
    struct Entries {
        tx: broadcast::Sender<Entry>,
    }

    impl Default for Entries {
        fn default() -> Self {
            Self {
                tx: broadcast::channel(8).0,
            }
        }
    }

    /// Every field it declares answers through the filter it binds.
    #[resolver]
    #[use_exception_filters(LedgerConflict)]
    struct VaultResolver {
        #[inject]
        entries: Arc<Entries>,
    }

    #[operations]
    impl VaultResolver {
        #[mutation]
        #[public]
        async fn seal(&self) -> Result<bool, LedgerRefused> {
            Err(LedgerRefused)
        }

        #[subscription]
        #[public]
        #[use_interceptors(CountOpenings)]
        async fn entries(&self) -> impl Stream<Item = Entry> {
            stream::unfold(self.entries.tx.subscribe(), |mut rx| async move {
                rx.recv().await.ok().map(|entry| (entry, rx))
            })
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [LedgerConflict, CountOpenings, Entries, VaultResolver],
    )]
    struct VaultModule;

    #[tokio::test]
    async fn a_resolvers_exception_filter_answers_its_root_field() {
        let _serial = SERIAL.lock().await;
        let app = TestApp::for_module::<VaultModule>().await.expect("boots");

        let body = post(&app, "mutation { seal }").await;

        assert_eq!(
            body["errors"][0]["extensions"]["code"], "CONFLICT",
            "{body}"
        );
        assert!(!body.to_string().contains("10.0.0.9"), "{body}");
    }

    #[resolver]
    struct BareLedgerResolver;

    #[operations]
    impl BareLedgerResolver {
        #[mutation]
        #[public]
        async fn settle(&self) -> Result<bool, LedgerRefused> {
            Err(LedgerRefused)
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(None)],
        providers = [LedgerConflict, BareLedgerResolver],
    )]
    struct BareLedgerModule;

    #[tokio::test]
    async fn the_apps_exception_filter_answers_a_root_field() {
        let _serial = SERIAL.lock().await;
        let app = TestApp::builder()
            .module::<BareLedgerModule>()
            .use_exception_filters_global([exception_filter::<LedgerConflict>()])
            .build()
            .await
            .expect("boots");

        let body = post(&app, "mutation { settle }").await;

        assert_eq!(
            body["errors"][0]["extensions"]["code"], "CONFLICT",
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_subscriptions_layers_run_at_its_opening_never_per_item() {
        let _serial = SERIAL.lock().await;
        OPENINGS.store(0, Ordering::SeqCst);
        let app = TestApp::for_module::<VaultModule>().await.expect("boots");
        let entries = app.container().get::<Entries>().expect("provided");
        let mut socket = app.graphql_socket().open();
        socket.connect().await;
        socket.subscribe("1", "subscription { entries { n } }");
        let emit = async {
            nest_rs_testing::wait_until(std::time::Duration::from_secs(5), || {
                entries.tx.receiver_count() >= 1
            })
            .await;
            for n in 1..=3 {
                entries
                    .tx
                    .send(Entry { n })
                    .expect("the subscriber is live");
            }
        };

        let (first, ()) = tokio::join!(socket.next_item("1"), emit);
        let mut received = vec![first.expect("an item")];
        for _ in 1..3 {
            received.push(socket.next_item("1").await.expect("an item"));
        }

        assert_eq!(received[2]["data"]["entries"]["n"], 3, "{received:?}");
        assert_eq!(
            OPENINGS.load(Ordering::SeqCst),
            1,
            "once at the opening, none per item",
        );
    }
}
