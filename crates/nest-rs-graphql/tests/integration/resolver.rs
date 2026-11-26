//! Module-gating: a resolver in a reachable module appears in the schema; a
//! resolver in no reachable module is silently skipped, unless a served parent
//! carries its `#[field_resolver]`, which refuses the boot. Beside it, the
//! developer's method an operation calls — by its path, keeping every
//! attribute `#[operations]` does not consume — and the name each field is
//! served under.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nest_rs_config::Surface;
use nest_rs_core::{
    Layer, Problem, Scoped, ToProblem, UnitContext, async_trait, injectable, instance, module,
};
use nest_rs_graphql::async_graphql::Context;
use nest_rs_graphql::{GraphqlConfig, GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_pipes::{Pipe, PipeError, Piped};
use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
use nest_rs_testing::TestApp;
use tracing_subscriber::registry::LookupSpan as _;

#[resolver]
struct LooseResolver;

#[operations]
impl LooseResolver {
    #[query]
    #[public]
    async fn loose(&self) -> String {
        "ok".into()
    }
}

#[module(providers = [LooseResolver])]
struct LooseFeatureModule;

#[module(imports = [GraphqlModule, LooseFeatureModule])]
struct AppWithLoose;

// Linked into this binary, but unreachable here.
#[module(imports = [GraphqlModule::for_root(GraphqlConfig {
    introspection: Surface::Open,
    ..GraphqlConfig::default()
})])]
struct AppWithoutLoose;

#[tokio::test]
async fn a_reachable_resolver_appears_in_the_schema() {
    let app = TestApp::builder()
        .module::<AppWithLoose>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ loose }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let loose = json
        .value()
        .object()
        .get("data")
        .object()
        .get("loose")
        .string();
    assert_eq!(loose, "ok");
}

#[tokio::test]
async fn an_unreachable_resolver_is_filtered_from_the_schema() {
    let app = TestApp::builder()
        .module::<AppWithoutLoose>()
        .build()
        .await
        .expect("an app composes only the resolvers in its reachable modules");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({
            "query": "{ __type(name: \"Query\") { fields { name } } }"
        }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let fields = json
        .value()
        .object()
        .get("data")
        .object()
        .get("__type")
        .object()
        .get("fields")
        .array();
    for field in fields.iter() {
        let name = field.object().get("name").string();
        assert_ne!(name, "loose", "unreachable resolver leaked into the schema",);
    }
}

// `#[field_resolver]`'s position 1 is the **parent**, so `&Context` comes second.

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Parcel {
    id: i32,
}

#[resolver]
struct ParcelResolver;

#[operations]
impl ParcelResolver {
    #[query]
    #[public]
    async fn parcel(&self, id: i32) -> Parcel {
        Parcel { id }
    }

    /// Parent first, then the context: the context is the wrapper's `__ctx`, not
    /// an injected dependency.
    #[field_resolver]
    async fn tag(
        &self,
        parent: &Parcel,
        ctx: &nest_rs_graphql::async_graphql::Context<'_>,
    ) -> nest_rs_graphql::async_graphql::Result<String> {
        let _ = ctx.data_opt::<nest_rs_core::Container>();
        Ok(format!("parcel-{}", parent.id))
    }
}

#[module(imports = [GraphqlModule], providers = [ParcelResolver])]
struct ParcelModule;

#[tokio::test]
async fn a_field_resolver_takes_the_context_after_its_parent() {
    let app = TestApp::for_module::<ParcelModule>()
        .await
        .expect("the schema boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ parcel(id: 3) { id tag } }" }))
        .send()
        .await;
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert!(
        body["errors"].is_null(),
        "the documented shape resolves rather than reporting a missing provider: {body}",
    );
    assert_eq!(body["data"]["parcel"]["tag"], "parcel-3", "{body}");
}

// Method syntax resolves on the receiver before it derefs: a root holds its
// resolver in an `Arc`, a field resolver builds it by value.

#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait ShadowsOnArc {
    fn crate_label(&self) -> String;
}

impl<T> ShadowsOnArc for std::sync::Arc<T> {
    fn crate_label(&self) -> String {
        "the trait on Arc".into()
    }
}

#[expect(
    dead_code,
    reason = "never called: the expansion calls the method by its path"
)]
trait ShadowsByValue {
    fn weight(self, parent: &Crate) -> nest_rs_graphql::async_graphql::Result<String>;
}

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Crate {
    id: i32,
}

#[resolver]
struct CrateResolver;

impl ShadowsByValue for CrateResolver {
    fn weight(self, _parent: &Crate) -> nest_rs_graphql::async_graphql::Result<String> {
        Ok("the trait by value".into())
    }
}

#[operations]
impl CrateResolver {
    #[query]
    #[public]
    fn crate_label(&self) -> String {
        "the operation".into()
    }

    #[query]
    #[public]
    async fn shipment(&self) -> Crate {
        Crate { id: 1 }
    }

    #[field_resolver]
    fn weight(&self, parent: &Crate) -> nest_rs_graphql::async_graphql::Result<String> {
        Ok(format!("the field resolver {}", parent.id))
    }
}

#[module(imports = [GraphqlModule], providers = [CrateResolver])]
struct CrateModule;

#[tokio::test]
async fn an_operation_calls_its_method_and_not_a_trait_method_of_the_same_name() {
    let app = TestApp::for_module::<CrateModule>()
        .await
        .expect("the schema boots");

    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": "{ crateLabel shipment { weight } }" }))
        .send()
        .await;
    let body = resp.json().await.value().deserialize::<serde_json::Value>();

    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["crateLabel"], "the operation", "{body}");
    assert_eq!(
        body["data"]["shipment"]["weight"], "the field resolver 1",
        "{body}"
    );
}

// An answer may borrow through its receiver's lifetime, which a root field's wrapper names
// too; a field resolver's resolver is built per call, so it borrows that one in its body.

/// The aisle a shelf stands in, which its fields answer borrowed.
struct Aisle(String);

impl Default for Aisle {
    fn default() -> Self {
        Self("aisle 4".to_owned())
    }
}

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Bin {
    number: i32,
}

#[resolver]
#[derive(Default)]
struct BorrowingResolver {
    aisle: Aisle,
}

#[operations]
impl BorrowingResolver {
    #[query]
    #[public]
    async fn aisle<'a>(&'a self) -> &'a str {
        let aisle: &'a Aisle = &self.aisle;
        &aisle.0
    }

    #[mutation]
    #[public]
    fn restock<'a>(&'a self) -> nest_rs_graphql::async_graphql::Result<&'a str> {
        let aisle: &'a Aisle = &self.aisle;
        Ok(&aisle.0)
    }

    #[query]
    #[public]
    async fn bin(&self) -> Bin {
        Bin { number: 3 }
    }

    #[field_resolver]
    async fn shelved<'a>(&'a self, parent: &'a Bin) -> String {
        let aisle: &'a str = &self.aisle.0;
        format!("bin {} in {aisle}", parent.number)
    }
}

#[module(imports = [GraphqlModule], providers = [BorrowingResolver])]
struct BorrowingModule;

#[tokio::test]
async fn an_operation_borrowing_through_the_lifetime_its_receiver_names_resolves() {
    let app = TestApp::for_module::<BorrowingModule>()
        .await
        .expect("the schema boots");

    let read = crate::graphql_answer(&app, "{ aisle bin { shelved } }").await;
    assert_eq!(read["data"]["aisle"], "aisle 4", "{read}");
    assert_eq!(read["data"]["bin"]["shelved"], "bin 3 in aisle 4", "{read}");
    let written = crate::graphql_answer(&app, "mutation { restock }").await;
    assert_eq!(written["data"]["restock"], "aisle 4", "{written}");
}

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Pallet {
    number: i32,
}

#[resolver]
#[derive(Default)]
struct FragmentResolver {
    aisle: Aisle,
}

// The methods come through `tt` fragments, keeping the caller's spans, and
// `#[operations]` the macro's: what the expansion binds resolves in the methods'.
macro_rules! declare_fragment_operations {
    ($($operations:tt)*) => {
        #[operations]
        impl FragmentResolver {
            $($operations)*
        }
    };
}

declare_fragment_operations! {
    #[query]
    #[public]
    async fn pallet(&self, number: i32) -> Pallet {
        Pallet { number }
    }

    #[query]
    #[public]
    async fn pallet_aisle<'a>(&'a self) -> &'a str {
        let aisle: &'a Aisle = &self.aisle;
        &aisle.0
    }

    #[mutation]
    #[public]
    fn restack(&self, number: i32) -> nest_rs_graphql::async_graphql::Result<Pallet> {
        Ok(Pallet { number: number + 1 })
    }

    #[field_resolver]
    async fn bay(&self, parent: &Pallet) -> String {
        format!("bay {}", parent.number)
    }

    #[field_resolver]
    async fn stacked<'a>(&'a self, parent: &'a Pallet) -> String {
        let aisle: &'a str = &self.aisle.0;
        format!("pallet {} in {aisle}", parent.number)
    }
}

#[module(imports = [GraphqlModule], providers = [FragmentResolver])]
struct FragmentModule;

#[tokio::test]
async fn a_field_a_macro_fragment_writes_resolves_as_the_one_written_out() {
    let app = TestApp::for_module::<FragmentModule>()
        .await
        .expect("the schema boots");

    let read =
        crate::graphql_answer(&app, "{ pallet(number: 7) { bay stacked } palletAisle }").await;
    assert_eq!(read["data"]["pallet"]["bay"], "bay 7", "{read}");
    assert_eq!(
        read["data"]["pallet"]["stacked"], "pallet 7 in aisle 4",
        "{read}"
    );
    assert_eq!(read["data"]["palletAisle"], "aisle 4", "{read}");
    let written = crate::graphql_answer(&app, "mutation { restack(number: 7) { number } }").await;
    assert_eq!(written["data"]["restack"]["number"], 8, "{written}");
}

// An operation's attributes stay on the developer's method, as a route's do: `#[operations]`
// removes what it consumes, and the field it generates carries what describes it to
// async-graphql alone.

/// A ledger lookup's audit, built per request: a `Scoped<Audit>` is no argument
/// of the schema, and no `Debug`, so an instrument skips it.
#[injectable(scope = request)]
#[derive(Default)]
struct Audit {
    lookups: AtomicU32,
}

impl Audit {
    fn record(&self) -> u32 {
        self.lookups.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// The spans each part of an instrumented field ran in, innermost first.
static RAN_IN: Mutex<Vec<(&'static str, Vec<&'static str>)>> = Mutex::new(Vec::new());

/// Notes the names of the spans `part` runs in, read off the registry the
/// capture installs.
fn note(part: &'static str) {
    let scope = tracing::Span::current().id().map_or_else(Vec::new, |id| {
        tracing::dispatcher::get_default(|dispatch| {
            dispatch
                .downcast_ref::<tracing_subscriber::Registry>()
                .and_then(|registry| registry.span(&id))
                .map(|span| span.scope().map(|span| span.name()).collect())
                .unwrap_or_default()
        })
    });
    RAN_IN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((part, scope));
}

/// Where `part` ran, as [`note`] read it.
fn ran_in(part: &str) -> Vec<&'static str> {
    let ran_in = RAN_IN.lock().unwrap_or_else(PoisonError::into_inner);
    ran_in
        .iter()
        .find(|(noted, _)| *noted == part)
        .map(|(_, scope)| scope.clone())
        .unwrap_or_else(|| panic!("the {part} ran: {ran_in:?}"))
}

/// Lets every caller through, noting the spans its check runs in.
#[injectable]
#[derive(Default, Layer)]
struct NotingGuard;

#[async_trait]
impl Guard for NotingGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        note("guard");
        Ok(())
    }
}

impl GuardFor<GraphqlView> for NotingGuard {}

/// Trims a label, noting the spans it runs in.
struct NotingTrim;

impl Pipe for NotingTrim {
    type In = String;
    type Out = String;
    fn transform(raw: String) -> Result<String, PipeError> {
        note("pipe");
        Ok(raw.trim().to_owned())
    }
}

/// The desk a ledger is kept at, which an instrument reads off `self`.
struct Desk(&'static str);

impl Default for Desk {
    fn default() -> Self {
        Self("desk 4")
    }
}

#[resolver]
#[derive(Default)]
struct LedgerResolver {
    desk: Desk,
}

#[operations]
impl LedgerResolver {
    /// Instrumented as a route is: the span is the method's, so it names the
    /// method's own `self` and records the label the pipe already trimmed.
    #[query]
    #[public]
    #[use_guards(NotingGuard)]
    #[tracing::instrument(skip_all, fields(desk = self.desk.0, id = id, label = %label.0))]
    async fn entry(
        &self,
        ctx: &Context<'_>,
        audit: Scoped<Audit>,
        id: i32,
        label: Piped<NotingTrim>,
    ) -> String {
        note("method");
        format!(
            "{} {id} {}, lookup {}",
            ctx.field().name(),
            label.into_inner(),
            audit.record()
        )
    }

    /// Refused by its guard: the method never runs, so its instrument opens
    /// no span and records nothing the caller sent.
    #[query]
    #[public]
    #[use_guards(crate::FieldStoreDown)]
    #[tracing::instrument(skip(self))]
    async fn sealed(&self, account: String) -> String {
        note("sealed");
        account
    }

    /// `err` records what the method itself answered, never the opaque error
    /// the field sends in its place.
    #[query]
    #[public]
    #[tracing::instrument(skip_all, err)]
    async fn overdrawn(&self) -> Result<i32, BankDown> {
        Err(BankDown)
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [LedgerResolver, NotingGuard, Audit, crate::FieldStoreDown],
)]
struct LedgerModule;

#[tokio::test]
async fn an_attribute_on_an_operation_stays_on_its_method_spanning_its_body_alone() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<LedgerModule>()
        .await
        .expect("the schema boots");

    let body = crate::graphql_answer(&app, r#"{ entry(id: 7, label: "  receipts  ") }"#).await;
    assert_eq!(
        body["data"]["entry"], "entry 7 receipts, lookup 1",
        "{body}"
    );
    let span = logs.expect_span(module_path!(), "entry");
    assert_eq!(span.field("id").as_deref(), Some("7"), "{:?}", span.fields);
    assert_eq!(
        span.field("desk").as_deref(),
        Some("desk 4"),
        "{:?}",
        span.fields
    );
    assert_eq!(
        span.field("label").as_deref(),
        Some("receipts"),
        "the method's argument, past its pipe: {:?}",
        span.fields
    );
    for (part, spanned) in [("guard", false), ("pipe", false), ("method", true)] {
        let scope = ran_in(part);
        assert_eq!(
            scope.contains(&"entry"),
            spanned,
            "the {part} ran in {scope:?}"
        );
    }
}

#[tokio::test]
async fn a_caller_a_guard_denies_opens_no_span_of_the_methods_instrument() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<LedgerModule>()
        .await
        .expect("the schema boots");

    let refused = crate::graphql_answer(&app, r#"{ sealed(account: "FR76 3000") }"#).await;
    assert_eq!(
        refused["errors"][0]["extensions"]["code"], "UNAVAILABLE",
        "{refused}"
    );
    assert!(
        RAN_IN
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty(),
        "the method ran"
    );
    let opened: Vec<_> = logs
        .spans()
        .into_iter()
        .filter(|span| span.target == module_path!())
        .collect();
    assert!(opened.is_empty(), "{opened:?}");
    assert!(
        !logs.quotes("FR76 3000"),
        "an argument of a refused caller reached a line"
    );
}

#[tokio::test]
async fn an_instruments_err_records_the_error_the_method_answered() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<LedgerModule>()
        .await
        .expect("the schema boots");

    let answered = crate::graphql_answer(&app, "{ overdrawn }").await;
    assert_eq!(
        answered["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{answered}"
    );
    let recorded: Vec<_> = logs
        .events_at(tracing::Level::ERROR)
        .into_iter()
        .filter(|event| event.target == module_path!())
        .collect();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(
        recorded[0].field("error").as_deref(),
        Some("the bank did not answer"),
        "{recorded:?}"
    );
}

/// An attribute of the app's own that shares the resilience decorator's name
/// is the app's: `#[operations]` reads no attribute by its name, so it stays
/// on the method like any other, whatever the method answers.
mod named_resilient {
    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_pipes::Piped;
    use nest_rs_testing::TestApp;
    // The app's own attribute macro, named as the decorator is.
    use tracing::instrument as resilient;

    use super::{NotingGuard, NotingTrim, note, ran_in};

    #[resolver]
    struct LookupResolver;

    #[operations]
    impl LookupResolver {
        #[query]
        #[public]
        #[use_guards(NotingGuard)]
        #[resilient(skip_all)]
        async fn lookup(&self, id: i32, label: Piped<NotingTrim>) -> String {
            note("method");
            format!("{id} {}", label.into_inner())
        }
    }

    #[module(imports = [GraphqlModule], providers = [LookupResolver, NotingGuard])]
    struct LookupModule;

    #[tokio::test]
    async fn an_attribute_an_app_names_resilient_is_the_apps_own_on_its_method() {
        let _logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<LookupModule>()
            .await
            .expect("the schema boots");

        let body = crate::graphql_answer(&app, r#"{ lookup(id: 7, label: "  receipts  ") }"#).await;
        assert_eq!(body["data"]["lookup"], "7 receipts", "{body}");
        for (part, spanned) in [("guard", false), ("pipe", false), ("method", true)] {
            let scope = ran_in(part);
            assert_eq!(
                scope.contains(&"lookup"),
                spanned,
                "the {part} ran in {scope:?}"
            );
        }
    }
}

/// The bank a balance is read from.
#[instance]
struct Bank;

/// What the bank answers when it fails: a failure no client reads.
#[derive(Debug, thiserror::Error)]
#[error("the bank did not answer")]
struct BankDown;

impl ToProblem for BankDown {
    fn to_problem(&self) -> Option<Problem> {
        None
    }
}

/// How many times a balance was asked of the bank.
#[injectable]
#[derive(Default)]
struct BalanceReads {
    sent: AtomicU32,
}

#[resolver]
struct ReconciledResolver {
    #[inject]
    bank: Arc<ResiliencePolicy<Bank>>,
    #[inject]
    reads: Arc<BalanceReads>,
}

#[operations]
impl ReconciledResolver {
    #[query]
    #[public]
    #[resilient(bank)]
    async fn balance(&self) -> Result<i32, ResilienceError<BankDown>> {
        self.reads.sent.fetch_add(1, Ordering::SeqCst);
        Ok(42)
    }

    /// Decorated under a condition that holds, beside an instrument in the same
    /// `cfg_attr`: each entry is the method's, in the order written, so the one
    /// span covers the whole run, every attempt of it.
    #[query]
    #[public]
    #[cfg_attr(
        all(),
        resilient(bank, idempotent),
        tracing::instrument(skip(self, ctx))
    )]
    async fn settled(&self, ctx: &Context<'_>) -> Result<i32, ResilienceError<BankDown>> {
        if self.reads.sent.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(BankDown);
        }
        Ok(i32::try_from(ctx.field().name().len()).unwrap_or(i32::MAX))
    }

    /// Decorated under a condition that does not hold: the method is the one
    /// written, its body answering the method's own type under no policy.
    #[query]
    #[public]
    #[cfg_attr(any(), resilient(bank))]
    async fn pending(&self) -> Result<i32, ResilienceError<BankDown>> {
        self.reads.sent.fetch_add(1, Ordering::SeqCst);
        Ok(7)
    }
}

#[module(
    imports = [GraphqlModule, ResilienceModule<Bank>],
    providers = [ReconciledResolver, BalanceReads],
)]
struct ReconciledModule;

#[tokio::test(start_paused = true)]
async fn a_resilient_operation_runs_its_developers_body_under_the_policy() {
    let app = TestApp::for_module::<ReconciledModule>()
        .await
        .expect("the schema boots");
    let bank = app
        .container()
        .get::<ResiliencePolicy<Bank>>()
        .expect("the module built the bank's policy");
    let reads = app
        .container()
        .get::<BalanceReads>()
        .expect("the module provides the count");

    bank.open_breaker(Duration::from_secs(30))
        .expect("the policy has a breaker");
    let refused = crate::graphql_answer(&app, "{ balance }").await;
    let extensions = &refused["errors"][0]["extensions"];
    assert_eq!(extensions["code"], "UNAVAILABLE", "{refused}");
    assert_eq!(extensions["retryAfterSeconds"], 30, "{refused}");
    assert_eq!(
        reads.sent.load(Ordering::SeqCst),
        0,
        "the breaker refused the body unsent"
    );

    bank.close_breaker().expect("the policy has a breaker");
    let answered = crate::graphql_answer(&app, "{ balance }").await;
    assert_eq!(answered["data"]["balance"], 42, "{answered}");
    assert_eq!(reads.sent.load(Ordering::SeqCst), 1, "sent once closed");
}

/// A `#[cfg_attr]` stays on the developer's method whole: the decorator under
/// its condition runs the operation under the policy while it holds, and the
/// instrument beside it spans that method.
#[tokio::test(start_paused = true)]
async fn a_resilient_inside_a_cfg_attr_runs_its_operation_under_the_policy_while_it_holds() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<ReconciledModule>()
        .await
        .expect("the schema boots");
    let bank = app
        .container()
        .get::<ResiliencePolicy<Bank>>()
        .expect("the module built the bank's policy");
    let reads = app
        .container()
        .get::<BalanceReads>()
        .expect("the module provides the count");

    bank.open_breaker(Duration::from_secs(30))
        .expect("the policy has a breaker");
    let refused = crate::graphql_answer(&app, "{ settled }").await;
    assert_eq!(
        refused["errors"][0]["extensions"]["code"], "UNAVAILABLE",
        "{refused}"
    );
    assert_eq!(
        reads.sent.load(Ordering::SeqCst),
        0,
        "the breaker refused the body unsent"
    );
    let undecorated = crate::graphql_answer(&app, "{ pending }").await;
    assert_eq!(undecorated["data"]["pending"], 7, "{undecorated}");
    assert_eq!(
        reads.sent.swap(0, Ordering::SeqCst),
        1,
        "a condition that does not hold leaves the method under no policy"
    );

    bank.close_breaker().expect("the policy has a breaker");
    let answered = crate::graphql_answer(&app, "{ settled }").await;
    assert_eq!(answered["data"]["settled"], 7, "{answered}");
    assert_eq!(
        reads.sent.load(Ordering::SeqCst),
        2,
        "sent again after its failure, as `idempotent` declares"
    );
    let spanned = logs
        .spans()
        .into_iter()
        .filter(|span| span.target == module_path!() && span.name == "settled")
        .count();
    assert_eq!(
        spanned, 2,
        "the instrument below the decorator spans the whole run: once a query"
    );
}

/// One operation of each role, as a route would be written: an instrument
/// reading the resolver's own `self`, and the decorator imported under another
/// name inside a nested `#[cfg_attr]`, each staying on the developer's method.
mod every_role {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use nest_rs_core::module;
    use nest_rs_graphql::async_graphql::SimpleObject;
    use nest_rs_graphql::async_graphql::futures_util::StreamExt as _;
    use nest_rs_graphql::async_graphql::futures_util::stream::{self, BoxStream};
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
    use nest_rs_resilience::resilient as shielded;
    use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy};
    use nest_rs_testing::TestApp;

    use super::{BalanceReads, Bank, BankDown};

    #[derive(SimpleObject)]
    #[graphql(complex)]
    struct Account {
        id: i32,
    }

    #[resolver]
    struct AccountsResolver {
        #[inject]
        bank: Arc<ResiliencePolicy<Bank>>,
        #[inject]
        reads: Arc<BalanceReads>,
    }

    impl AccountsResolver {
        /// The bank read once more, failing the first time it is asked.
        fn read(&self) -> Result<u32, BankDown> {
            match self.reads.sent.fetch_add(1, Ordering::SeqCst) {
                0 => Err(BankDown),
                sent => Ok(sent),
            }
        }
    }

    #[operations]
    impl AccountsResolver {
        #[query]
        #[public]
        #[cfg_attr(all(), cfg_attr(all(), shielded(bank, idempotent)))]
        #[tracing::instrument(skip_all, fields(role = "query", sent = self.reads.sent.load(Ordering::SeqCst)))]
        async fn account(&self) -> Result<Account, ResilienceError<BankDown>> {
            self.read()?;
            Ok(Account { id: 7 })
        }

        #[mutation]
        #[public]
        #[cfg_attr(all(), cfg_attr(all(), shielded(bank, idempotent)))]
        #[tracing::instrument(skip_all, fields(role = "mutation", sent = self.reads.sent.load(Ordering::SeqCst)))]
        async fn close_account(&self) -> Result<i32, ResilienceError<BankDown>> {
            self.read()?;
            Ok(7)
        }

        #[subscription]
        #[public]
        #[cfg_attr(all(), cfg_attr(all(), shielded(bank, idempotent)))]
        #[tracing::instrument(skip_all, fields(role = "subscription", sent = self.reads.sent.load(Ordering::SeqCst)))]
        async fn account_changes(
            &self,
        ) -> Result<BoxStream<'static, i32>, ResilienceError<BankDown>> {
            self.read()?;
            Ok(stream::iter([7, 8]).boxed())
        }

        #[entity]
        #[public]
        #[cfg_attr(all(), cfg_attr(all(), shielded(bank, idempotent)))]
        #[tracing::instrument(skip_all, fields(role = "entity", sent = self.reads.sent.load(Ordering::SeqCst)))]
        async fn find_account_by_id(&self, id: i32) -> Result<Account, ResilienceError<BankDown>> {
            self.read()?;
            Ok(Account { id })
        }

        /// The parent the field resolver below reads, built without the bank.
        #[query]
        #[public]
        async fn opened_account(&self) -> Account {
            Account { id: 7 }
        }

        #[field_resolver]
        #[cfg_attr(all(), cfg_attr(all(), shielded(bank, idempotent)))]
        #[tracing::instrument(skip_all, fields(role = "field", sent = self.reads.sent.load(Ordering::SeqCst)))]
        async fn balance(&self, parent: &Account) -> Result<i32, ResilienceError<BankDown>> {
            self.read()?;
            Ok(parent.id * 6)
        }
    }

    #[module(
        imports = [
            GraphqlModule::for_root(GraphqlConfig {
                federation: true,
                federation_sdl: nest_rs_config::Surface::Open,
                ..GraphqlConfig::default()
            }),
            ResilienceModule<Bank>,
        ],
        providers = [AccountsResolver, BalanceReads],
    )]
    struct AccountsModule;

    /// Each role's first read of the bank fails and its second answers: the
    /// decorator ran the developer's body again, under the policy its `self`
    /// holds, and the instrument's one span, reading that `self`, covers the run.
    #[tokio::test(start_paused = true)]
    async fn an_operation_of_each_role_keeps_its_instrument_and_its_renamed_decorator() {
        let logs = nest_rs_testing::LogCapture::install();
        let app = TestApp::for_module::<AccountsModule>()
            .await
            .expect("the schema boots");
        let reads = app
            .container()
            .get::<BalanceReads>()
            .expect("the module provides the count");
        let retried = |role: &str| {
            assert_eq!(
                reads.sent.swap(0, Ordering::SeqCst),
                2,
                "the {role} was sent again after its failure"
            );
        };

        let read = crate::graphql_answer(&app, "{ account { id } }").await;
        assert_eq!(read["data"]["account"]["id"], 7, "{read}");
        retried("query");

        let written = crate::graphql_answer(&app, "mutation { closeAccount }").await;
        assert_eq!(written["data"]["closeAccount"], 7, "{written}");
        retried("mutation");

        let mut socket = app.graphql_socket().open();
        socket.connect().await;
        socket.subscribe("changes", "subscription { accountChanges }");
        for change in [7, 8] {
            let item = socket.next_item("changes").await.expect("an item");
            assert_eq!(item["data"]["accountChanges"], change, "{item}");
        }
        retried("subscription");

        let resolved = crate::graphql_answer(
            &app,
            r#"{ _entities(representations: [{ __typename: "Account", id: 4 }]) { ... on Account { id } } }"#,
        )
        .await;
        assert_eq!(resolved["data"]["_entities"][0]["id"], 4, "{resolved}");
        retried("entity");

        let nested = crate::graphql_answer(&app, "{ openedAccount { balance } }").await;
        assert_eq!(nested["data"]["openedAccount"]["balance"], 42, "{nested}");
        retried("field resolver");

        for (method, role) in [
            ("account", "query"),
            ("close_account", "mutation"),
            ("account_changes", "subscription"),
            ("find_account_by_id", "entity"),
            ("balance", "field"),
        ] {
            let span = logs.expect_span(module_path!(), method);
            assert_eq!(
                span.field("role").as_deref(),
                Some(role),
                "{:?}",
                span.fields
            );
            assert!(span.field("sent").is_some(), "{:?}", span.fields);
        }
    }
}

/// A field is deprecated for its clients by `#[graphql(deprecation = "…")]`,
/// which the field `#[operations]` generates carries, where Rust's
/// `#[deprecated]` on the method is refused: the schema's introspection says it.
mod deprecated {
    use nest_rs_core::module;
    use nest_rs_graphql::async_graphql::SimpleObject;
    use nest_rs_graphql::{GraphqlConfig, GraphqlModule, operations, resolver};
    use nest_rs_testing::TestApp;

    #[derive(SimpleObject)]
    #[graphql(complex)]
    struct Invoice {
        number: i32,
    }

    #[resolver]
    struct InvoicesResolver;

    #[operations]
    impl InvoicesResolver {
        #[query]
        #[public]
        #[graphql(deprecation = "ask `invoices`")]
        async fn invoice(&self) -> Invoice {
            Invoice { number: 7 }
        }

        #[field_resolver]
        #[graphql(deprecation = "read `number`")]
        async fn total(&self, parent: &Invoice) -> i32 {
            parent.number * 6
        }
    }

    #[module(
        imports = [GraphqlModule::for_root(GraphqlConfig {
            introspection: nest_rs_config::Surface::Open,
            ..GraphqlConfig::default()
        })],
        providers = [InvoicesResolver],
    )]
    struct InvoicesModule;

    #[tokio::test]
    async fn a_field_deprecated_by_its_graphql_attribute_says_so_to_its_clients() {
        let app = TestApp::for_module::<InvoicesModule>()
            .await
            .expect("the schema boots");

        let read = crate::graphql_answer(&app, "{ invoice { number total } }").await;
        assert_eq!(read["data"]["invoice"]["total"], 42, "{read}");
        let described = crate::graphql_answer(
            &app,
            r#"{
                query: __type(name: "Query") {
                    fields(includeDeprecated: true) { name isDeprecated deprecationReason }
                }
                invoice: __type(name: "Invoice") {
                    fields(includeDeprecated: true) { name isDeprecated deprecationReason }
                }
            }"#,
        )
        .await;
        for (owner, field, reason) in [
            ("query", "invoice", "ask `invoices`"),
            ("invoice", "total", "read `number`"),
        ] {
            let fields = described["data"][owner]["fields"]
                .as_array()
                .unwrap_or_else(|| panic!("`{owner}` lists its fields: {described}"));
            let deprecated = fields
                .iter()
                .find(|described| described["name"] == field)
                .unwrap_or_else(|| panic!("`{owner}` lists `{field}`: {described}"));
            assert_eq!(deprecated["isDeprecated"], true, "{deprecated}");
            assert_eq!(deprecated["deprecationReason"], reason, "{deprecated}");
        }
    }
}

/// A lint level written on an operation describes the developer's method, where
/// an expectation is checked, plain or under a `#[cfg_attr]`: the field wrapper
/// beside it trips neither lint below, so an expectation carried there would go
/// unfulfilled — rustc's under this level, clippy's under the workspace's
/// `-D warnings`.
#[deny(unfulfilled_lint_expectations)]
mod expected {
    use nest_rs_core::module;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_testing::TestApp;

    /// The dock the crates are unloaded at.
    struct Dock(String);

    impl Default for Dock {
        fn default() -> Self {
            Self("dock 4".to_owned())
        }
    }

    #[derive(nest_rs_graphql::async_graphql::SimpleObject)]
    #[graphql(complex)]
    struct Crate {
        number: i32,
    }

    #[resolver]
    #[derive(Default)]
    struct ExpectedResolver {
        dock: Dock,
    }

    #[operations]
    impl ExpectedResolver {
        #[query]
        #[public]
        #[expect(unused_variables, reason = "the depot narrows the count next release")]
        async fn crate_count(&self, depot: String) -> i32 {
            3
        }

        #[query]
        #[public]
        #[expect(
            clippy::needless_lifetimes,
            reason = "the dock is named as it is borrowed"
        )]
        async fn dock<'a>(&'a self) -> &'a str {
            &self.dock.0
        }

        #[query]
        #[public]
        async fn unloaded(&self) -> Crate {
            Crate { number: 7 }
        }

        #[field_resolver]
        #[expect(unused_variables, reason = "the unit converts the weight next release")]
        async fn weight(&self, parent: &Crate, unit: String) -> i32 {
            parent.number * 2
        }

        /// Under a condition, where the expectation is checked on the method
        /// too: `#[operations]` leaves a `#[cfg_attr]` on it as written.
        #[query]
        #[public]
        #[cfg_attr(
            all(),
            expect(unused_variables, reason = "the bay narrows the count next release")
        )]
        async fn pallet_count(&self, bay: String) -> i32 {
            5
        }
    }

    #[module(imports = [GraphqlModule], providers = [ExpectedResolver])]
    struct ExpectedModule;

    #[tokio::test]
    async fn an_expectation_on_an_operation_is_checked_on_the_developers_method() {
        let app = TestApp::for_module::<ExpectedModule>()
            .await
            .expect("the schema boots");

        let body = crate::graphql_answer(
            &app,
            r#"{ crateCount(depot: "Lyon") palletCount(bay: "B2") dock unloaded { weight(unit: "kg") } }"#,
        )
        .await;
        assert_eq!(body["data"]["crateCount"], 3, "{body}");
        assert_eq!(body["data"]["palletCount"], 5, "{body}");
        assert_eq!(body["data"]["dock"], "dock 4", "{body}");
        assert_eq!(body["data"]["unloaded"]["weight"], 14, "{body}");
    }
}

// async-graphql's camel case reads `a1_b` and `a_1b` as one field; the compile-time
// refusal is `nest-rs-macro-hygiene`'s `operations_two_methods_one_served_name`.

#[derive(nest_rs_graphql::async_graphql::SimpleObject)]
#[graphql(complex)]
struct Ledger {
    id: i32,
}

#[resolver]
struct NamingResolver;

#[operations]
impl NamingResolver {
    #[query]
    #[public]
    async fn get_2fa(&self) -> String {
        "get_2fa".into()
    }

    #[query]
    #[public]
    async fn user_id(&self, snake: i32) -> String {
        format!("user_id:{snake}")
    }

    #[query]
    #[public]
    #[expect(
        non_snake_case,
        reason = "the field's spelled case is the shape under test"
    )]
    async fn userID(&self, upper: i32) -> String {
        format!("userID:{upper}")
    }

    #[query]
    #[public]
    async fn ledger(&self, id: i32) -> Ledger {
        Ledger { id }
    }

    #[field_resolver]
    async fn a_1b_total(&self, parent: &Ledger, right: i32) -> String {
        format!("a_1b_total:{}:{right}", parent.id)
    }
}

#[module(imports = [GraphqlModule], providers = [NamingResolver])]
struct NamingModule;

async fn naming_query(query: &str) -> serde_json::Value {
    let app = TestApp::for_module::<NamingModule>()
        .await
        .expect("the schema boots");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": query }))
        .send()
        .await;
    resp.json().await.value().deserialize::<serde_json::Value>()
}

#[tokio::test]
async fn a_digit_after_an_underscore_starts_a_word_as_async_graphql_reads_it() {
    let body = naming_query("{ get2Fa }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["get2Fa"], "get_2fa", "{body}");
}

#[tokio::test]
async fn a_capitalised_method_name_is_its_own_field() {
    let body = naming_query("{ userId(snake: 1) userID(upper: 2) }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(body["data"]["userId"], "user_id:1", "{body}");
    assert_eq!(body["data"]["userID"], "userID:2", "{body}");
}

#[tokio::test]
async fn a_field_resolver_is_named_by_the_same_rule() {
    let body = naming_query("{ ledger(id: 4) { a1BTotal(right: 2) } }").await;
    assert!(body["errors"].is_null(), "{body}");
    assert_eq!(
        body["data"]["ledger"]["a1BTotal"], "a_1b_total:4:2",
        "{body}"
    );
}

// The name `#[operations]` states must be the one async-graphql's own derive
// serves; the oracle is that derive, never a copy of its rule.

use nest_rs_graphql::async_graphql;

/// The method names both halves carry: a digit after `_`, a digit run, a digit
/// before a letter, a capital run, and the plain case.
struct Oracle;

#[async_graphql::Object]
impl Oracle {
    async fn get_2fa(&self) -> i32 {
        1
    }
    async fn a_1b(&self) -> i32 {
        1
    }
    async fn page_2_items(&self) -> i32 {
        1
    }
    async fn v2_api(&self) -> i32 {
        1
    }
    #[expect(
        non_snake_case,
        reason = "the field's spelled case is the shape under test"
    )]
    async fn userID(&self) -> i32 {
        1
    }
    async fn user_count(&self) -> i32 {
        1
    }
}

#[resolver]
struct OracleResolver;

#[operations]
impl OracleResolver {
    #[query]
    #[public]
    async fn get_2fa(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn a_1b(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn page_2_items(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn v2_api(&self) -> i32 {
        1
    }
    #[expect(
        non_snake_case,
        reason = "the field's spelled case is the shape under test"
    )]
    #[query]
    #[public]
    async fn userID(&self) -> i32 {
        1
    }
    #[query]
    #[public]
    async fn user_count(&self) -> i32 {
        1
    }
}

#[module(providers = [OracleResolver])]
struct OracleFeatureModule;

#[module(imports = [
    GraphqlModule::for_root(GraphqlConfig {
        introspection: Surface::Open,
        ..GraphqlConfig::default()
    }),
    OracleFeatureModule,
])]
struct AppWithOracleNames;

const QUERY_FIELDS: &str = "{ __schema { queryType { fields { name } } } }";

fn field_names(data: &serde_json::Value) -> std::collections::BTreeSet<String> {
    data["__schema"]["queryType"]["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|field| field["name"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_field_is_served_under_the_name_async_graphql_gives_the_method() {
    let oracle = async_graphql::Schema::build(
        Oracle,
        async_graphql::EmptyMutation,
        async_graphql::EmptySubscription,
    )
    .finish()
    .execute(QUERY_FIELDS)
    .await;
    assert!(oracle.errors.is_empty(), "{:?}", oracle.errors);
    let expected = field_names(&oracle.data.into_json().expect("introspection answers JSON"));
    assert!(
        expected.contains("get2Fa") && expected.contains("a1B"),
        "the oracle splits at a digit: {expected:?}",
    );

    let app = TestApp::builder()
        .module::<AppWithOracleNames>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql");
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!({ "query": QUERY_FIELDS }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let text = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a GraphQL response body");
    let body: serde_json::Value = serde_json::from_str(&text).expect("a GraphQL response is JSON");
    let served = field_names(&body["data"]);

    let missing: Vec<&String> = expected.difference(&served).collect();
    assert!(
        missing.is_empty(),
        "fields async-graphql would serve that #[operations] does not: {missing:?} — served \
         {served:?}",
    );
}

/// `#[ComplexObject]` grafts a `#[field_resolver]` onto its parent at compile
/// time, so a served parent serves the field whether or not the app reaches its
/// resolver: the schema build refuses that, as the module gate requires.
mod field_host {
    use std::sync::Arc;

    use nest_rs_core::{injectable, module};
    use nest_rs_graphql::async_graphql::SimpleObject;
    use nest_rs_graphql::{GraphqlModule, operations, resolver};
    use nest_rs_testing::TestApp;

    #[derive(Clone, SimpleObject)]
    #[graphql(complex)]
    struct Leaf {
        id: i32,
    }

    #[injectable]
    #[derive(Default)]
    struct Sap;

    #[resolver]
    struct LeavesResolver;

    #[operations]
    impl LeavesResolver {
        #[query]
        #[public]
        async fn leaves(&self) -> Vec<Leaf> {
            vec![Leaf { id: 1 }]
        }
    }

    #[resolver]
    struct LeafFieldsResolver {
        #[inject]
        sap: Arc<Sap>,
    }

    #[operations]
    impl LeafFieldsResolver {
        #[field_resolver]
        async fn veins(&self, leaf: &Leaf) -> i32 {
            let Self { sap: _ } = self;
            leaf.id * 2
        }
    }

    #[module(providers = [Sap, LeafFieldsResolver])]
    struct LeafFieldsModule;

    #[module(imports = [GraphqlModule], providers = [LeavesResolver])]
    struct LeavesGraphqlModule;

    #[tokio::test]
    async fn a_served_field_whose_resolver_is_not_reached_is_refused_naming_the_import() {
        let Err(refused) = TestApp::for_module::<LeavesGraphqlModule>().await else {
            panic!("`veins` would be served outside every module the app imports");
        };
        let refused = nest_rs_core::error_message(refused.as_ref());

        for named in [
            "`Leaf.veins`",
            "LeafFieldsResolver",
            "import `LeafFieldsModule`",
        ] {
            assert!(refused.contains(named), "names {named}: {refused}");
        }
    }

    #[module(imports = [LeavesGraphqlModule, LeafFieldsModule])]
    struct LeafApp;

    #[tokio::test]
    async fn a_served_field_whose_resolver_is_imported_boots_and_answers() {
        let app = TestApp::for_module::<LeafApp>()
            .await
            .expect("the resolver of `veins` is reached");

        let body = crate::graphql_answer(&app, "{ leaves { veins } }").await;

        assert_eq!(body["data"]["leaves"][0]["veins"], 2, "{body}");
    }

    #[derive(Clone, SimpleObject)]
    #[graphql(complex)]
    struct Stem {
        id: i32,
    }

    #[derive(Clone, SimpleObject)]
    struct Stalk {
        #[graphql(flatten)]
        stem: Stem,
    }

    #[resolver]
    struct StalksResolver;

    #[operations]
    impl StalksResolver {
        #[query]
        #[public]
        async fn stalk(&self) -> Stalk {
            Stalk {
                stem: Stem { id: 1 },
            }
        }
    }

    #[resolver]
    struct StemFieldsResolver;

    #[operations]
    impl StemFieldsResolver {
        #[field_resolver]
        async fn rings(&self, stem: &Stem) -> i32 {
            stem.id + 1
        }
    }

    #[module(providers = [StemFieldsResolver])]
    struct StemFieldsModule;

    #[module(imports = [GraphqlModule], providers = [StalksResolver])]
    struct StalksGraphqlModule;

    #[tokio::test]
    async fn a_flattened_parents_field_whose_resolver_is_not_reached_is_refused() {
        let Err(refused) = TestApp::for_module::<StalksGraphqlModule>().await else {
            panic!("a flattened object's field is served like any other");
        };
        let refused = nest_rs_core::error_message(refused.as_ref());

        for named in [
            "`Stem.rings`",
            "StemFieldsResolver",
            "import `StemFieldsModule`",
        ] {
            assert!(refused.contains(named), "names {named}: {refused}");
        }
    }
}
