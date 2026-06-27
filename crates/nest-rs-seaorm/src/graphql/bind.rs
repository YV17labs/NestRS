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

/// Turn a by-id argument into the loaded, authorized entity (the resolver
/// analog of [`crate::Bind`]). Outcomes: no row → `Ok(None)`; denied →
/// `FORBIDDEN` (existence not hidden, matching the HTTP `Bind`); else
/// `Ok(Some(model))`. Requires the ambient ability; without one this returns an
/// error so a missing auth bridge cannot silently behave as anonymous.
///
/// The service is resolved from the container by type `S`; when the caller
/// already holds the service instance (a resolver's `#[inject]` field), prefer
/// [`bind_required_with`] — it needs no `S` token, so the binding can be driven
/// entirely from the entity carried by an `Authorized<E, A>` parameter type.
pub async fn bind<S, A>(
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
    bind_with::<S, A>(&service, ctx, id).await
}

/// [`bind`], but against a service instance the caller already holds (a
/// resolver's injected `&Service`) instead of resolving it from the container by
/// type. This is what lets a `#[query]`/`#[mutation]` declare its subject purely
/// as an `Authorized<E, A>` parameter: `#[crud]` reads the entity + action off
/// the parameter type and the service off the resolver field, so neither is
/// retyped in an attribute.
///
/// Crate-internal: the public instance-form entry point is
/// [`bind_required_with`] (the only one the macro layer calls).
async fn bind_with<S, A>(
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
        .map_err(|err| Error::new(err.to_string()))?
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
/// `Authorized<E, A>` parameter then carries the authorization — and the action
/// `A` — at the type level, so the method body cannot reach a row the caller was
/// not allowed to load, nor act under an action it was not granted.
pub async fn bind_required<S, A>(ctx: &Context<'_>, id: &str) -> Result<Authorized<S::Entity, A>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    not_found_to_err(bind::<S, A>(ctx, id).await?)
}

/// [`bind_required`], but against a service instance the caller already holds —
/// the mutation-subject form that skips the container lookup. The signature-only operation
/// `#[crud]` synthesises calls this: `bind_required_with(&*self.svc, ctx, &id)`,
/// with the entity + action inferred from the `Authorized<E, A>` parameter type.
pub async fn bind_required_with<S, A>(
    service: &S,
    ctx: &Context<'_>,
    id: &str,
) -> Result<Authorized<S::Entity, A>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    not_found_to_err(bind_with::<S, A>(service, ctx, id).await?)
}

/// A bound mutation subject must exist (a mutation has no row to act on
/// otherwise); a missing row becomes a `NOT_FOUND` GraphQL error.
fn not_found_to_err<E: EntityTrait, A: ActionMarker>(
    model: Option<E::Model>,
) -> Result<Authorized<E, A>> {
    match model {
        Some(model) => Ok(Authorized::new(model)),
        None => Err(Error::new("not found").extend_with(|_, e| e.set("code", "NOT_FOUND"))),
    }
}
