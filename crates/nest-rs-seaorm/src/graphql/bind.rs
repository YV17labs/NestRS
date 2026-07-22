//! [`bind`] — route-model binding for resolvers, the GraphQL analog of
//! [`crate::Bind`].

use nest_rs_authz::ActionMarker;
use nest_rs_core::Container;
use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::{Access, Authorized, CrudService};

/// Matches the `nest_rs_authz::graphql` gate's denial shape (code `FORBIDDEN`).
fn forbidden() -> Error {
    Error::new("forbidden").extend_with(|_, e| e.set("code", "FORBIDDEN"))
}

/// A failed by-id load is logged in full (shared with the HTTP `Bind`) and
/// answered with a generic error, so SQL/driver detail never reaches the
/// client.
fn internal(service: &'static str, err: &sea_orm::DbErr) -> Error {
    crate::error::log_by_id_load_failure(service, err);
    Error::new("internal error").extend_with(|_, e| e.set("code", "INTERNAL_SERVER_ERROR"))
}

/// Turn a by-id argument into the loaded, authorized entity (the resolver
/// analog of [`crate::Bind`]). Outcomes: no row → `Ok(None)`; denied →
/// `FORBIDDEN` (existence not hidden, matching the HTTP `Bind`); else
/// `Ok(Some(model))`. Requires the ambient ability; without one this returns an
/// error so a missing auth bridge cannot silently behave as anonymous.
///
/// The service is resolved from the container by type `S`.
pub async fn bind<A, S>(
    ctx: &Context<'_>,
    id: &str,
) -> Result<Option<<S::Entity as EntityTrait>::Model>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    let service = ctx
        .data_unchecked::<Container>()
        .get::<S>()
        .ok_or_else(|| Error::new("no provider registered for the bound service"))?;
    bind_with::<A, S>(&service, ctx, id).await
}

/// Core of [`bind`] against a service instance: ambient-ability check, id parse,
/// and the authorized load. [`bind`] resolves the service from the container,
/// then delegates here.
async fn bind_with<A, S>(
    service: &S,
    ctx: &Context<'_>,
    id: &str,
) -> Result<Option<<S::Entity as EntityTrait>::Model>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    // No ambient ability ⇒ fail closed before any load.
    if ctx
        .data_opt::<std::sync::Arc<nest_rs_authz::Ability>>()
        .is_none()
    {
        return Err(Error::new(
            "missing request `Ability` — is the GraphQL auth bridge installed on /graphql?",
        ));
    }
    let id = Uuid::parse_str(id).map_err(|err| Error::new(err.to_string()))?;
    if id.get_version_num() != 7 {
        return Err(Error::new("id must be a UUID v7"));
    }
    match service
        .access(A::ACTION, id)
        .await
        .map_err(|err| internal(std::any::type_name::<S>(), &err))?
    {
        Access::Found(model) => Ok(Some(model)),
        Access::Denied => Err(forbidden()),
        Access::Missing => Ok(None),
    }
}

/// Bind a **mutation subject**: turn a by-id argument into the loaded, authorized
/// entity as an [`Authorized`] proof. Unlike [`bind`] — which returns `None` for
/// a missing row so a *query* resolves to null — a missing row is an error here
/// (code `NOT_FOUND`): a mutation has no row to act on. Denied → `FORBIDDEN`;
/// found → `Authorized`. Hand the result straight to the service method whose
/// `Authorized<A, E>` parameter then carries the authorization — and the action
/// `A` — at the type level, so the method body cannot reach a row the caller was
/// not allowed to load, nor act under an action it was not granted.
pub async fn bind_required<A, S>(ctx: &Context<'_>, id: &str) -> Result<Authorized<A, S::Entity>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    not_found_to_err(bind::<A, S>(ctx, id).await?)
}

/// A bound mutation subject must exist (a mutation has no row to act on
/// otherwise); a missing row becomes a `NOT_FOUND` GraphQL error.
fn not_found_to_err<A: ActionMarker, E: EntityTrait>(
    model: Option<E::Model>,
) -> Result<Authorized<A, E>> {
    match model {
        Some(model) => Ok(Authorized::new(model)),
        None => Err(Error::new("not found").extend_with(|_, e| e.set("code", "NOT_FOUND"))),
    }
}
