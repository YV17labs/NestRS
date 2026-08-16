//! Covers `src/http/authorize.rs` — what the shaper says when the wiring is
//! wrong.
//!
//! `Authorize<A, S>` answers a missing ambient ability with a `500` whose body
//! is an opaque problem+json, deliberately: a client learns nothing about the
//! app's authorization state from a wiring bug. That makes the event the *only*
//! place the cause exists, which is why it is `error` and why it carries the
//! remedy — and why nothing reading it was a gap rather than a nicety.

use nest_rs_authz::{Read, http::Authorize};
use nest_rs_testing::LogCapture;
use poem::{Body, FromRequest, Request, RequestBody};

mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    #[sea_orm(table_name = "widgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[tokio::test]
async fn a_route_whose_ability_guard_never_ran_says_so_at_error_with_the_remedy() {
    let logs = LogCapture::install();
    // No `Arc<Ability>` in extensions: the shape a route gets when `#[authorize]`
    // is written but `AuthzHttpModule` is not imported.
    let req = Request::builder()
        .uri("/widgets/1".parse().unwrap())
        .finish();
    let mut body = RequestBody::new(Body::empty());

    let err = Authorize::<Read, widget::Entity>::from_request(&req, &mut body)
        .await
        .err()
        .expect("a missing ambient ability is a wiring bug, not a pass");
    assert_eq!(err.status(), poem::http::StatusCode::INTERNAL_SERVER_ERROR);

    let event = logs.expect_one(
        "nest_rs::authz",
        "missing request Ability — route is authorized but no ability guard ran",
    );
    assert_eq!(event.level, "error");
    // Three fields, and each answers a question the opaque body cannot: which
    // route, which posture, and what to do about it. The path comes from
    // `original_uri`, which a hand-built request leaves at `/` — what matters
    // here is that the field is carried at all, since a bare `warn`+ is itself
    // the defect `CLAUDE.md` names.
    assert!(
        event.field("path").is_some(),
        "the event names the route, got {:?}",
        event.fields,
    );
    assert!(
        event.field("subject").is_some_and(|s| s.contains("widget")),
        "the event names the subject, got {:?}",
        event.fields,
    );
    assert!(
        event
            .field("hint")
            .is_some_and(|h| h.contains("AuthzHttpModule")),
        "the event carries the remedy, got {:?}",
        event.fields,
    );
}
