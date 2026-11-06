//! Covers `src/http/authorize.rs` — what a route's posture says when the wiring
//! is wrong: the `500` body is opaque, so the `error` event is the only place the
//! cause exists — and where the class gate it declares runs.

use nest_rs_authz::Read;
use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HttpView, Json, StatusCode, controller, routes};
use nest_rs_testing::{LogCapture, TestApp};

mod widget {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Widget")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Answers for an ability and attaches none: what the boot cannot tell from a guard
/// that does, so the gate fails closed.
#[injectable]
#[derive(Default)]
struct AttachesNothing;

impl Layer for AttachesNothing {}

#[async_trait]
impl Guard for AttachesNothing {
    fn __attaches_ability(&self) -> bool {
        true
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for AttachesNothing {}

#[controller(path = "/widgets")]
#[use_guards(AttachesNothing)]
#[derive(Default)]
struct WidgetsController;

#[routes]
impl WidgetsController {
    #[get("/")]
    #[public(Read, widget::Entity)]
    async fn list(&self) -> Json<Vec<i32>> {
        Json(Vec::new())
    }
}

#[module(providers = [AttachesNothing, WidgetsController])]
struct WidgetsModule;

#[tokio::test]
async fn a_route_whose_ability_guard_attached_nothing_says_so_at_error_with_the_remedy() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<WidgetsModule>()
        .await
        .expect("the guard answers for an ability");

    let refused = app.http().get("/widgets").send().await;
    refused.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    let event = logs.expect_one("nest_rs::authz", "authorization denied");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("transport").as_deref(), Some("http"));
    assert_eq!(event.field("reason").as_deref(), Some("no_ambient_ability"));
    assert_eq!(
        event.field("subject").as_deref(),
        Some("widgets"),
        "the event names the subject, got {:?}",
        event.fields,
    );
    assert!(
        event
            .field("remedy")
            .is_some_and(|h| h.contains("AuthzModule")),
        "the event carries the remedy, got {:?}",
        event.fields,
    );
}

/// The class gate runs with the guards, before every route-scoped layer: a
/// caller the posture refuses is answered by the gate, never by a layer that
/// would have answered for the handler.
mod gate_order {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nest_rs_authz::{AbilityBuilder, Action, Read};
    use nest_rs_core::async_trait;
    use nest_rs_core::{
        Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, UnitContext, injectable, module,
    };
    use nest_rs_guards::{Denial, Guard, GuardFor};
    use nest_rs_http::{
        HeaderValue, HttpView, Json, Response, StatusCode, controller, crud, header, routes,
    };
    use nest_rs_seaorm::CrudService;
    use nest_rs_testing::TestApp;

    mod secret {
        use nest_rs_seaorm::expose;
        use sea_orm::entity::prelude::*;

        #[expose(name = "SecretDto")]
        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "secrets")]
        pub(crate) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            #[expose]
            pub id: Uuid,
            #[expose]
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(crate) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    use secret::SecretDto;

    /// What a stored answer looks like: the handler is never called.
    const STORED: &str = r#"{"id":"0195b4a6-4c1c-7d3e-8f00-000000000001","name":"stored"}"#;

    /// Grants `Read` on secrets to a caller sending `x-read`, nothing otherwise.
    #[injectable]
    #[derive(Default)]
    struct Grants;

    impl Layer for Grants {}

    #[async_trait]
    impl Guard for Grants {
        fn __attaches_ability(&self) -> bool {
            true
        }

        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
            let mut ab = AbilityBuilder::new();
            if nest_rs_http::HttpView::of(cx)?
                .request()
                .header("x-read")
                .is_some()
            {
                ab.can(Action::Read, secret::Entity);
            }
            cx.attach(nest_rs_authz::__private::AmbientAbility::new(
                ab.build().expect("valid test ability"),
            ));
            Ok(())
        }

        async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<nest_rs_http::HttpView> for Grants {}

    /// A cache answering every request from its store, as a route-scoped
    /// caching interceptor does on a hit.
    #[injectable]
    #[derive(Default)]
    struct Stored;

    impl Layer for Stored {}

    #[async_trait]
    impl Interceptor for Stored {
        async fn intercept(&self, cx: &mut UnitContext<'_>, _next: Next<'_>) -> Outcome {
            let stored = Response::builder()
                .header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )
                .body(STORED);
            Ok(HttpView::of(cx)?.respond(stored))
        }
    }

    static SWALLOWED: AtomicUsize = AtomicUsize::new(0);

    /// Maps every error its route returns to a success, counting each.
    #[injectable]
    #[derive(Default)]
    struct Swallow;

    impl Layer for Swallow {}

    #[async_trait]
    impl ExceptionFilter for Swallow {
        type Exception = dyn std::error::Error + Send + Sync;

        async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
            SWALLOWED.fetch_add(1, Ordering::SeqCst);
            match HttpView::of(cx) {
                Ok(view) => Caught::Answered(view.respond(Response::builder().body("swallowed"))),
                Err(_) => Caught::Declined,
            }
        }
    }

    #[controller(path = "/secrets")]
    #[use_guards(Grants)]
    struct SecretsController;

    #[routes]
    impl SecretsController {
        #[get("/stored")]
        #[authorize(Read, secret::Entity)]
        #[use_interceptors(Stored)]
        async fn stored(&self) -> Json<Vec<SecretDto>> {
            Json(Vec::new())
        }

        #[get("/swallowed")]
        #[authorize(Read, secret::Entity)]
        #[use_exception_filters(Swallow)]
        async fn swallowed(&self) -> Json<Vec<SecretDto>> {
            Json(Vec::new())
        }

        #[get("/feed")]
        #[public(Read, secret::Entity)]
        #[use_interceptors(Stored)]
        async fn feed(&self) -> Json<Vec<SecretDto>> {
            Json(Vec::new())
        }
    }

    #[injectable]
    #[derive(Default)]
    struct SecretsService;

    impl CrudService for SecretsService {
        type Entity = secret::Entity;
    }

    #[controller(path = "/crud")]
    #[use_guards(Grants)]
    #[use_interceptors(Stored)]
    struct SecretsCrudController {
        #[inject]
        svc: Arc<SecretsService>,
    }

    #[crud(service = SecretsService, ops = [list, get])]
    impl SecretsCrudController {}

    #[module(
        imports = [nest_rs_seaorm::SeaOrmDatabaseModule],
        providers = [Grants, Stored, Swallow, SecretsController, SecretsService, SecretsCrudController],
    )]
    struct GateOrderModule;

    async fn app() -> TestApp {
        TestApp::builder()
            .provide(sea_orm::DatabaseConnection::default())
            .provide(nest_rs_seaorm::SeaOrmConfig::default())
            .module::<GateOrderModule>()
            .build()
            .await
            .expect("the gate-order module boots")
    }

    #[tokio::test]
    async fn a_refused_caller_is_answered_by_the_gate_not_by_a_route_scoped_interceptor() {
        let app = app().await;

        let refused = app.http().get("/secrets/stored").send().await;
        refused.assert_status(StatusCode::FORBIDDEN);

        let granted = app
            .http()
            .get("/secrets/stored")
            .header("x-read", "")
            .send()
            .await;
        granted.assert_status_is_ok();
        granted.assert_text(STORED).await;
    }

    #[tokio::test]
    async fn a_route_scoped_filter_never_sees_the_gates_denial() {
        let app = app().await;
        let before = SWALLOWED.load(Ordering::SeqCst);

        let refused = app.http().get("/secrets/swallowed").send().await;

        refused.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(SWALLOWED.load(Ordering::SeqCst), before, "the filter ran");
    }

    #[tokio::test]
    async fn a_public_read_is_gated_before_its_interceptor_too() {
        let app = app().await;

        let refused = app.http().get("/secrets/feed").send().await;
        refused.assert_status(StatusCode::FORBIDDEN);

        let granted = app
            .http()
            .get("/secrets/feed")
            .header("x-read", "")
            .send()
            .await;
        granted.assert_status_is_ok();
        granted.assert_text(STORED).await;
    }

    #[tokio::test]
    async fn a_crud_operation_is_gated_before_its_controllers_interceptor() {
        let app = app().await;

        let refused = app.http().get("/crud").send().await;
        refused.assert_status(StatusCode::FORBIDDEN);
        let refused = app
            .http()
            .get("/crud/0195b4a6-4c1c-7d3e-8f00-000000000001")
            .send()
            .await;
        refused.assert_status(StatusCode::FORBIDDEN);

        let granted = app.http().get("/crud").header("x-read", "").send().await;
        granted.assert_status_is_ok();
        granted.assert_text(STORED).await;
    }

    /// Declares the posture and binds no guard: nothing attaches the ability
    /// its class gate reads.
    #[controller(path = "/unguarded")]
    struct UnguardedController;

    #[routes]
    impl UnguardedController {
        #[get("/")]
        #[authorize(Read, secret::Entity)]
        async fn list(&self) -> Json<Vec<SecretDto>> {
            Json(Vec::new())
        }
    }

    #[module(providers = [UnguardedController])]
    struct UnguardedModule;

    #[tokio::test]
    async fn a_posture_no_guard_backs_refuses_the_boot() {
        let Err(refused) = TestApp::for_module::<UnguardedModule>().await else {
            panic!("a gate with no guard to attach its ability would answer every call 500");
        };
        let chain = format!("{refused:#}");
        assert!(chain.contains("GET /unguarded"), "{chain}");
        assert!(chain.contains("#[use_guards]"), "{chain}");
    }
}
