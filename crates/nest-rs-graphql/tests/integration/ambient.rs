//! `src/ambient.rs`: an `Ambient<T>` parameter reads what a guard attached —
//! the request's guards for every field, a field's own for its body — and is
//! no argument of the schema; absent, the field answers opaquely and files
//! the type.

use nest_rs_core::{Ambient, AmbientValue, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::SimpleObject;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::HttpView;
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

/// Who the request's guard says is calling.
#[derive(Clone)]
struct Caller(String);

impl AmbientValue for Caller {}

/// What a field's own guard derives from the caller.
#[derive(Clone)]
struct Author(String);

impl AmbientValue for Author {}

/// What no guard attaches.
#[derive(Clone)]
struct Unattached;

impl AmbientValue for Unattached {}

/// Attaches the caller the request names, for every field it carries.
#[injectable]
#[derive(Default, Layer)]
struct CallerGuard;

#[async_trait]
impl Guard for CallerGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let name = HttpView::of(cx)?
            .request()
            .header("x-name")
            .unwrap_or("anonymous");
        let caller = Caller(name.to_owned());
        cx.attach(caller);
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

/// Derives the author from the caller, at the field it is bound on.
#[injectable]
#[derive(Default, Layer)]
struct AuthorGuard;

#[async_trait]
impl Guard for AuthorGuard {
    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let caller = cx.require::<Caller>()?;
        let author = Author(format!("{} (author)", caller.0));
        cx.attach(author);
        Ok(())
    }
}

impl GuardFor<GraphqlView> for AuthorGuard {}

#[derive(SimpleObject)]
#[graphql(complex)]
struct Card {
    title: &'static str,
}

#[resolver]
struct CtxResolver;

#[operations]
impl CtxResolver {
    #[query]
    #[public]
    async fn me(&self, caller: Ambient<Caller>) -> String {
        caller.into_inner().0
    }

    #[mutation]
    #[public]
    #[use_guards(AuthorGuard)]
    async fn sign(&self, author: Ambient<Author>, text: String) -> String {
        format!("{text}, {}", author.into_inner().0)
    }

    #[query]
    #[public]
    async fn card(&self) -> Card {
        Card { title: "hello" }
    }

    #[field_resolver]
    async fn greeting(&self, parent: &Card, caller: Ambient<Caller>) -> String {
        format!("{} {}", parent.title, caller.into_inner().0)
    }

    #[query]
    #[public]
    async fn unwired(&self, missing: Ambient<Unattached>) -> String {
        let Unattached = missing.into_inner();
        "attached".to_owned()
    }
}

#[module(
    imports = [GraphqlModule],
    providers = [CallerGuard, AuthorGuard, CtxResolver],
)]
struct CtxModule;

#[module(imports = [CtxModule], providers = [GlobalGuards<(CallerGuard,)>])]
struct BootApp;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<BootApp>()
        .build()
        .await
        .expect("boots")
}

async fn ask(app: &TestApp, query: &str) -> Value {
    let resp = app
        .http()
        .post("/graphql")
        .header("x-name", "ada")
        .body_json(&json!({ "query": query }))
        .send()
        .await;
    resp.json().await.value().deserialize()
}

#[tokio::test]
async fn a_root_field_reads_what_the_requests_guard_attached() {
    let app = boot().await;
    assert_eq!(
        ask(&app, "{ me }").await,
        json!({ "data": { "me": "ada" } })
    );
}

#[tokio::test]
async fn a_root_field_reads_what_its_own_guard_attached() {
    let app = boot().await;
    assert_eq!(
        ask(&app, r#"mutation { sign(text: "hi") }"#).await,
        json!({ "data": { "sign": "hi, ada (author)" } }),
    );
}

#[tokio::test]
async fn a_field_resolver_reads_what_the_requests_guard_attached() {
    let app = boot().await;
    assert_eq!(
        ask(&app, "{ card { greeting } }").await,
        json!({ "data": { "card": { "greeting": "hello ada" } } }),
    );
}

#[tokio::test]
async fn a_ctx_is_no_argument_of_the_schema() {
    let app = boot().await;
    let answered = ask(&app, r#"{ me(caller: "mallory") }"#).await;
    let said = answered["errors"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(said.contains("Unknown argument"), "{answered}");
}

#[tokio::test]
async fn an_unattached_ctx_answers_opaquely_and_files_its_type() {
    let logs = LogCapture::install();
    let app = boot().await;

    let answered = ask(&app, "{ unwired }").await;

    assert_eq!(
        answered["errors"][0]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{answered}"
    );
    assert_eq!(
        answered["errors"][0]["extensions"]["code"], "INTERNAL",
        "{answered}"
    );
    assert!(!answered.to_string().contains("Unattached"), "{answered}");
    let line = logs.expect_one("nest_rs::graphql", "graphql operation failed");
    assert!(
        line.field("error")
            .is_some_and(|error| error.contains("Unattached")),
        "{:?}",
        line.fields,
    );
}
