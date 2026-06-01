//! [`Bind<S, A>`] — route-model binding: turn a path id into the loaded,
//! authorized entity, so a handler's parameter is a domain object, not a scalar.
//!
//! It folds the three steps a by-id handler used to write by hand into one typed
//! parameter — parse the id, load the row, check the caller may act on it — and
//! short-circuits the request before the handler body runs:
//!
//! - the path id is not a UUID v7 → `400`;
//! - no row with that id → `404`;
//! - the row exists but the caller's [`Ability`] denies action `A` on it → `403`
//!   (the existence is intentionally not hidden, matching the gate's semantics);
//! - otherwise the handler receives the loaded [`EntityTrait::Model`].
//!
//! The load runs **through the entity's service** ([`CrudService::access`]), not the
//! ORM directly — the service is the single audited gateway, so a by-id binding
//! emits the same `nestrs::orm` access span (a denial logs at `warn`) as every other
//! data access. `Bind` is generic over the *service* `S`: a handler writes
//! `user: Bind<UsersService, Read>` and the extractor resolves `Arc<S>` from the
//! request scope. The ability is read from the request extensions, where the
//! [`AbilityGuard`](crate::AbilityGuard) placed it, and installed as the ambient
//! ability for the `access` call (its `Repo` load runs in the request transaction
//! via the ambient executor); its absence is a `500` (the guard did not run — a
//! wiring bug).

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use nestrs_authz::{with_ability, Ability, ActionMarker};
use nestrs_core::RequestScope;
use nestrs_database::{Access, CrudService};
use poem::http::StatusCode;
use poem::web::Path;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

/// The loaded, authorized entity bound from a path id, through the entity's service
/// `S`. Declare it as a handler parameter — `user: Bind<UsersService, Read>` — and
/// read the model via [`Deref`] or own it with [`into_inner`](Bind::into_inner).
pub struct Bind<S: CrudService, A>(<S::Entity as EntityTrait>::Model, PhantomData<fn() -> A>);

impl<S: CrudService, A> Bind<S, A> {
    pub fn into_inner(self) -> <S::Entity as EntityTrait>::Model {
        self.0
    }
}

impl<S: CrudService, A> Deref for Bind<S, A> {
    type Target = <S::Entity as EntityTrait>::Model;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, S, A> FromRequest<'a> for Bind<S, A>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let Path(id) = Path::<Uuid>::from_request(req, body).await?;
        if id.get_version_num() != 7 {
            return Err(Error::from_string(
                "path id must be a UUID v7",
                StatusCode::BAD_REQUEST,
            ));
        }

        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Error::from_string(
                "missing request `Ability` — is the ability guard applied to this route?",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        let scope = req.extensions().get::<Arc<RequestScope>>().ok_or_else(|| {
            Error::from_string(
                "request scope not installed — RequestScopeEndpoint must wrap the route tree",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        let service = scope.get::<S>().ok_or_else(|| {
            Error::from_string(
                "no provider registered for the bound service — add it to a module's providers",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        // Load + authorize through the service's audited gateway, with the caller's
        // ability installed as ambient for its instance check (and `Repo` scoping).
        let access = with_ability(ability.clone(), service.access(A::ACTION, id))
            .await
            .map_err(|err| {
                Error::from_string(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
            })?;
        match access {
            Access::Found(model) => Ok(Bind(model, PhantomData)),
            Access::Denied => Err(Error::from_status(StatusCode::FORBIDDEN)),
            Access::Missing => Err(Error::from_status(StatusCode::NOT_FOUND)),
        }
    }
}
