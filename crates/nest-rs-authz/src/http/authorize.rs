//! [`Authorize<A, S>`] — route-level access gate as a poem extractor.

use std::any::TypeId;
use std::marker::PhantomData;
use std::sync::Arc;

use nest_rs_guards::{Denial, denial_to_http_error};
use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

use crate::gate::{Refusal, transport};
use crate::{Ability, ActionMarker, Subject};

/// Enforcement plumbing for action `A` on subject `S`: 403 unless the
/// request-scoped [`Ability`] grants it; 500 when the ability is missing
/// (wiring bug, not a client error). Class-level only — the per-row filter and
/// response mask enforce conditions. Its presence in a handler signature is
/// also what makes `#[routes]` install the response shaper (automatic masking
/// + ambient ability).
///
/// # Don't write this — write `#[authorize(Action, Entity)]`
///
/// The posture of an HTTP route is declared by the decorator, exactly as on a
/// `#[query]`/`#[mutation]`:
///
/// ```
/// # use std::sync::Arc;
/// # use nest_rs_authz::{AbilityBuilder, Action, Create};
/// # use nest_rs_core::{Layer, injectable, input, module};
/// # use nest_rs_guards::{Denial, Guard, HttpGuard};
/// # use nest_rs_http::poem::http::StatusCode;
/// # use nest_rs_http::poem::web::Json;
/// # use nest_rs_http::poem::{Request, Result};
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
/// # impl nest_rs_resource::WireModelDefaults for users::Entity {}
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
/// #     async fn check_http(&self, req: &mut Request) -> std::result::Result<(), Denial> {
/// #         let mut ab = AbilityBuilder::new();
/// #         if req.headers().contains_key("x-admin") {
/// #             ab.can(Action::Manage, users::Entity);
/// #         }
/// #         let ability = ab.build().map_err(|_| Denial::internal("malformed rules"))?;
/// #         req.extensions_mut().insert(Arc::new(ability));
/// #         Ok(())
/// #     }
/// # }
/// # impl HttpGuard for Grants {}
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
/// `#[routes]` desugars that to this extractor as the handler's first
/// parameter; written by hand it works, but is not a posture declaration. An
/// extractor reached indirectly (nested, or run by a hand-rolled `FromRequest`)
/// is backstopped by `nest_rs_http::MaskProbe`, which fails the route closed.
pub struct Authorize<A, S>(PhantomData<fn() -> (A, S)>);

impl<'a, A, S> FromRequest<'a> for Authorize<A, S>
where
    A: ActionMarker,
    S: Subject,
{
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        nest_rs_http::MaskProbe::mark();
        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            // A wiring bug, and the response body is an opaque problem+json: log
            // it or the developer sees a 500 with nothing to grep for.
            tracing::error!(
                target: crate::TARGET,
                action = ?A::ACTION,
                subject = std::any::type_name::<S>(),
                path = %req.original_uri().path(),
                hint = "bind the ability guard (#[use_guards(AuthnGuard, AuthzGuard)]) \
                        and import AuthzModule in this feature's module.rs",
                "missing request Ability — route is authorized but no ability guard ran",
            );
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        if ability.can_class(A::ACTION, TypeId::of::<S>()) {
            return Ok(Authorize(PhantomData));
        }
        let missing = ability.missing_scopes(A::ACTION, TypeId::of::<S>());
        if missing.is_empty() {
            crate::gate::warn_denied(Refusal {
                reason: Some(crate::gate::reason::NO_CLASS_GRANT),
                ..Refusal::of::<A, S>(transport::HTTP)
            });
            return Err(denial_to_http_error(Denial::forbidden("forbidden")));
        }
        // A token that verified but is too narrow: the discovery interceptor turns
        // the scopes into the RFC 6750 `insufficient_scope` challenge.
        crate::gate::warn_denied(Refusal {
            reason: Some(crate::gate::reason::INSUFFICIENT_SCOPE),
            ..Refusal::of::<A, S>(transport::HTTP)
        });
        Err(denial_to_http_error(Denial::insufficient_scope(
            missing,
            "forbidden",
        )))
    }
}
