//! Covers `src/http/authorize.rs` — what the shaper says when the wiring is
//! wrong. The `500` body is opaque, so the `error` event is the only place the
//! cause exists.

use nest_rs_authz::{Read, http::Authorize};
use nest_rs_http::{Body, FromRequest, Request, RequestBody, StatusCode, Uri};
use nest_rs_testing::LogCapture;

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[tokio::test]
async fn a_route_whose_ability_guard_never_ran_says_so_at_error_with_the_remedy() {
    let logs = LogCapture::install();
    // No ambient `Ability`: the shape a route gets when `#[authorize]` is
    // written but no ability guard runs on it.
    let req = Request::builder()
        .uri(Uri::from_static("/widgets/1"))
        .finish();
    let mut body = RequestBody::new(Body::empty());

    let err = Authorize::<Read, widget::Entity>::from_request(&req, &mut body)
        .await
        .err()
        .expect("a missing ambient ability is a wiring bug, not a pass");
    assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let event = logs.expect_one("nest_rs::authz", "authorization denied");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("transport").as_deref(), Some("http"));
    assert_eq!(event.field("reason").as_deref(), Some("no_ambient_ability"));
    assert!(
        event.field("subject").is_some_and(|s| s.contains("widget")),
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

    use nest_rs_authz::http::Authorize;
    use nest_rs_authz::{AbilityBuilder, Action, Read, WireModelDefaults};
    use nest_rs_core::{Layer, injectable, module};
    use nest_rs_filters::{Filter, RequestSnapshot};
    use nest_rs_guards::{Denial, Guard, GuardFor};
    use nest_rs_http::StatusCode;
    use nest_rs_http::{Json, async_trait, controller, crud, routes};
    use nest_rs_interceptors::{Interceptor, Next};
    use nest_rs_seaorm::CrudService;
    use nest_rs_testing::TestApp;
    use poem::{Request, Response};

    mod secret {
        use sea_orm::entity::prelude::*;
        use serde::{Deserialize, Serialize};

        #[derive(
            Clone, Debug, PartialEq, DeriveEntityModel, Deserialize, Serialize, schemars::JsonSchema,
        )]
        #[sea_orm(table_name = "secrets")]
        pub(crate) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(crate) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    impl WireModelDefaults for secret::Entity {}

    #[derive(serde::Serialize, schemars::JsonSchema)]
    pub(crate) struct SecretDto {
        name: String,
    }

    impl From<&secret::Model> for SecretDto {
        fn from(model: &secret::Model) -> Self {
            Self {
                name: model.name.clone(),
            }
        }
    }

    /// What a stored answer looks like: the handler is never called.
    const STORED: &str = r#"{"id":"0195b4a6-4c1c-7d3e-8f00-000000000001","name":"stored"}"#;

    /// Grants `Read` on secrets to a caller sending `x-read`, nothing otherwise.
    #[injectable]
    #[derive(Default)]
    struct Grants;

    impl Layer for Grants {}

    #[async_trait]
    impl Guard for Grants {
        async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
            let mut ab = AbilityBuilder::new();
            if nest_rs_http::HttpView::of(cx)?
                .request()
                .header("x-read")
                .is_some()
            {
                ab.can(Action::Read, secret::Entity);
            }
            cx.attach(ab.build().expect("valid test ability"));
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
        async fn intercept(&self, _req: Request, _next: Next<'_>) -> poem::Result<Response> {
            Ok(Response::builder()
                .content_type("application/json")
                .body(STORED))
        }
    }

    static SWALLOWED: AtomicUsize = AtomicUsize::new(0);

    /// Maps every error its route returns to a success, counting each.
    #[injectable]
    #[derive(Default)]
    struct Swallow;

    impl Layer for Swallow {}

    #[async_trait]
    impl Filter for Swallow {
        async fn filter(&self, _req: &RequestSnapshot, _error: poem::Error) -> Response {
            SWALLOWED.fetch_add(1, Ordering::SeqCst);
            Response::builder().body("swallowed")
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
        #[use_filters(Swallow)]
        async fn swallowed(&self) -> Json<Vec<SecretDto>> {
            Json(Vec::new())
        }

        // The public-reads spelling: the posture is `#[public]`, and the
        // parameter is what gates the class and arms the mask.
        #[get("/feed")]
        #[public]
        #[use_interceptors(Stored)]
        async fn feed(&self, _authz: Authorize<Read, secret::Entity>) -> Json<Vec<SecretDto>> {
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

    #[crud(service = svc, entity = secret::Entity, output = SecretDto, ops = [list, get])]
    impl SecretsCrudController {}

    #[module(providers = [Grants, Stored, Swallow, SecretsController, SecretsService, SecretsCrudController])]
    struct GateOrderModule;

    async fn app() -> TestApp {
        TestApp::for_module::<GateOrderModule>()
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
    async fn the_public_reads_spelling_is_gated_before_its_interceptor_too() {
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
}
