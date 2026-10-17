//! [`Authorize<A, S>`] — the route-level class gate for action `A` on
//! subject `S`, and the type that arms the route's response mask.

use std::any::TypeId;
use std::marker::PhantomData;

use nest_rs_core::{Edge, Posture};
use nest_rs_guards::Denial;
use nest_rs_http::__private::denial_to_http_error;
use poem::{FromRequest, Request, RequestBody, Result};

use crate::gate::{Refusal, decide, warn_denied};
use crate::{Ability, ActionMarker, Subject};

/// Enforcement plumbing for action `A` on subject `S`. A route declaring it —
/// through `#[authorize(A, S)]` or as a handler parameter — runs
/// [`class_gate`](crate::class_gate) with its guards, before any interceptor or
/// filter the route declares, and installs the response shaper (automatic
/// masking + ambient ability). Class-level only — the per-row filter and
/// response mask enforce conditions.
///
/// # Don't write this — write `#[authorize(Action, Entity)]`
///
/// The posture of an HTTP route is declared by the decorator, exactly as on a
/// `#[query]`/`#[mutation]`:
///
/// ```
/// # use nest_rs_authz::{AbilityBuilder, Action, Create};
/// # use nest_rs_core::{Layer, injectable, input, module};
/// # use nest_rs_guards::{Denial, Guard, GuardFor};
/// # use nest_rs_http::poem::http::StatusCode;
/// # use nest_rs_http::poem::web::Json;
/// # use nest_rs_http::poem::Result;
/// # use nest_rs_http::{Valid, async_trait, controller, routes};
/// # use nest_rs_testing::TestApp;
/// # mod users {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize, serde::Deserialize)]
/// #     #[sea_orm(table_name = "users")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #         pub name: String,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # impl nest_rs_authz::WireModelDefaults for users::Entity {}
/// # #[input]
/// # struct CreateUser {
/// #     name: String,
/// # }
/// # #[input]
/// # struct User {
/// #     id: i32,
/// #     name: String,
/// # }
/// # impl From<CreateUser> for User {
/// #     fn from(input: CreateUser) -> Self {
/// #         User { id: 1, name: input.name }
/// #     }
/// # }
/// # #[injectable]
/// # #[derive(Default)]
/// # struct Grants;
/// # impl Layer for Grants {}
/// # #[async_trait]
/// # impl Guard for Grants {
/// #     async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> std::result::Result<(), Denial> {
/// #         let mut ab = AbilityBuilder::new();
/// #         if nest_rs_http::HttpView::of(cx)?.request().header("x-admin").is_some() {
/// #             ab.can(Action::Manage, users::Entity);
/// #         }
/// #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
/// #         cx.attach(ability);
/// #         Ok(())
/// #     }
/// # }
/// # impl GuardFor<nest_rs_http::HttpView> for Grants {}
/// # #[controller(path = "/users")]
/// # #[use_guards(Grants)]
/// # #[derive(Default)]
/// # struct UsersController;
/// # #[routes]
/// # impl UsersController {
/// #[post("/")]
/// #[authorize(Create, users::Entity)]
/// async fn create(&self, body: Valid<Json<CreateUser>>) -> Result<Json<User>> {
///     Ok(Json(User::from(body.into_inner())))
/// }
/// # }
/// # #[module(providers = [Grants, UsersController])]
/// # struct UsersModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
/// # let app = TestApp::for_module::<UsersModule>().await?;
/// # let ada = serde_json::json!({ "name": "ada" });
/// # let admin = app.http().post("/users").header("x-admin", "").body_json(&ada).send().await;
/// # let stranger = app.http().post("/users").body_json(&ada).send().await;
///
/// admin.assert_json(serde_json::json!({ "id": 1, "name": "ada" })).await;
/// stranger.assert_status(StatusCode::FORBIDDEN);
/// # Ok(())
/// # }
/// ```
///
/// `#[routes]` reads that as this type without a parameter. Written by hand as a
/// handler parameter — on a `#[public]` route, whose posture is the opposite
/// one — it gates and arms the same way, and checks again as it is extracted.
/// An extractor reached indirectly (nested, or run by a hand-rolled
/// `FromRequest`) is backstopped by the HTTP edge's masking probe, which fails
/// the route closed.
pub struct Authorize<A, S>(PhantomData<fn() -> (A, S)>);

impl<'a, A, S> FromRequest<'a> for Authorize<A, S>
where
    A: ActionMarker,
    S: Subject,
{
    async fn from_request(_req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        nest_rs_http::__private::MaskProbe::mark();
        checked::<A, S>().map_err(denial_to_http_error)?;
        Ok(Authorize(PhantomData))
    }
}

/// The class gate again, as the parameter is extracted: the route's shaper ran
/// [`class_gate`](crate::class_gate) with its guards, and an extractor reached
/// past the shaper — nested, or run by a hand-rolled `FromRequest` — holds the
/// same line. No site reaches an extractor, so it decides as on the
/// `#[public]` route a hand-written parameter is written for.
fn checked<A: ActionMarker, S: Subject>() -> std::result::Result<(), Denial> {
    let refusal = Refusal::on(Edge::Http.as_str()).declaring::<A, S>();
    let Some(ability) = nest_rs_core::ambient::<Ability>() else {
        crate::gate::error_unreached(Refusal {
            remedy: Some(EXTRACTED_UNREACHED),
            ..refusal
        });
        return Err(Denial::internal(crate::gate::NO_AMBIENT_ABILITY));
    };
    decide(&ability, A::ACTION, TypeId::of::<S>(), Posture::Public).map_err(|(why, denial)| {
        warn_denied(Refusal {
            reason: Some(why),
            ..refusal
        });
        denial
    })
}

/// What a route extracting `Authorize` with no ability is told to change.
const EXTRACTED_UNREACHED: &str = "bind the ability guard on this route — \
                                   #[use_guards(AuthnGuard, AuthzGuard)] or the app's global \
                                   pool — and import AuthzModule in this feature's module.rs";
