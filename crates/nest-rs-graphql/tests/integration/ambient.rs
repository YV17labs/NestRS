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

mod needed_principal {
    //! A `#[public]` field, or a `#[field_resolver]` under one, reading as an
    //! `Ambient<P>` the principal a guard produces: a caller that guard admitted
    //! without one answers `UNAUTHENTICATED`, filed as that guard's denial, never
    //! the wiring error an unattached value is, whether the guard runs at the
    //! field or only at the `/graphql` request.

    use nest_rs_core::{
        Ambient, AmbientValue, Layer, UnitContext, async_trait, code, injectable, module,
    };
    use nest_rs_graphql::async_graphql::SimpleObject;
    use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
    use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, GuardPhase, PrincipalClaim};
    use nest_rs_http::{EndpointGuards, HttpView};
    use nest_rs_testing::{LogCapture, TestApp};
    use serde_json::{Value, json};

    /// Who the request's credential names.
    #[derive(Clone)]
    struct Named(String);

    impl AmbientValue for Named {}

    /// Identifies a caller naming itself and admits one that does not, as an
    /// authentication guard admits an anonymous `/graphql` request in band.
    #[injectable]
    #[derive(Default, Layer)]
    struct Identifies;

    #[async_trait]
    impl Guard for Identifies {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }

        fn produced_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<Named>())
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            if let Some(name) = HttpView::of(cx)?.request().header("x-name") {
                let named = Named(name.to_owned());
                cx.attach(named);
            }
            Ok(())
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<HttpView> for Identifies {}

    #[derive(SimpleObject)]
    #[graphql(complex)]
    struct Badge {
        number: i32,
    }

    #[resolver]
    struct NamedResolver;

    #[operations]
    impl NamedResolver {
        #[query]
        #[public]
        async fn named(&self, named: Ambient<Named>) -> String {
            named.into_inner().0
        }

        #[query]
        #[public]
        async fn badge(&self) -> Badge {
            Badge { number: 7 }
        }

        #[field_resolver]
        async fn holder(&self, parent: &Badge, named: Ambient<Named>) -> String {
            format!("{} {}", parent.number, named.into_inner().0)
        }
    }

    #[module(imports = [GraphqlModule], providers = [Identifies, NamedResolver])]
    struct NamedModule;

    /// The guard in the app's pool: it runs at the request and at each root field.
    #[module(imports = [NamedModule], providers = [GlobalGuards<(Identifies,)>])]
    struct PooledApp;

    /// The guard at the `/graphql` request alone: no field's chain holds it.
    #[module(
        imports = [NamedModule],
        providers = [EndpointGuards<GraphqlView, (Identifies,)>],
    )]
    struct EndpointApp;

    async fn ask(app: &TestApp, query: &str, name: Option<&str>) -> Value {
        let mut request = app.http().post("/graphql");
        if let Some(name) = name {
            request = request.header("x-name", name);
        }
        let resp = request.body_json(&json!({ "query": query })).send().await;
        resp.json().await.value().deserialize()
    }

    /// `query` asked by `ada` and anonymously on `app`: the first reads her
    /// name off `path`, the second is refused there as an unidentified caller.
    async fn refused_unnamed(app: TestApp, query: &str, path: &[&str], named: &str) {
        let answered = ask(&app, query, Some("ada")).await;
        let read = path
            .iter()
            .fold(&answered["data"], |read, segment| &read[segment]);
        assert_eq!(read, named, "{query}: {answered}");

        let logs = LogCapture::install();
        let anonymous = ask(&app, query, None).await;
        assert_eq!(
            anonymous["errors"][0]["extensions"]["code"],
            code::UNAUTHENTICATED.as_str(),
            "{query}: {anonymous}",
        );
        assert_eq!(anonymous["errors"][0]["path"], json!(path), "{anonymous}");
        let denial = logs.expect_one("nest_rs::layers", "guard denied the operation");
        assert_eq!(denial.level, "warn");
        assert_eq!(denial.field("guard").as_deref(), Some("Identifies"));
        assert!(
            logs.find("nest_rs::graphql", "graphql operation failed")
                .is_empty(),
            "an unidentified caller is no wiring error",
        );
    }

    #[tokio::test]
    async fn a_public_field_reading_its_principal_refuses_an_unidentified_caller() {
        for app in [
            TestApp::for_module::<PooledApp>().await,
            TestApp::for_module::<EndpointApp>().await,
        ] {
            refused_unnamed(app.expect("boots"), "{ named }", &["named"], "ada").await;
        }
    }

    #[tokio::test]
    async fn a_field_resolver_under_a_public_field_refuses_an_unidentified_caller() {
        for app in [
            TestApp::for_module::<PooledApp>().await,
            TestApp::for_module::<EndpointApp>().await,
        ] {
            refused_unnamed(
                app.expect("boots"),
                "{ badge { holder } }",
                &["badge", "holder"],
                "7 ada",
            )
            .await;
        }
    }
}
