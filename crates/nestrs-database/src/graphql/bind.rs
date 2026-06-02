//! [`bind`] — route-model binding for resolvers, the GraphQL analog of
//! [`crate::Bind`].

use nestrs_authz::ActionMarker;
use nestrs_core::Container;
use nestrs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::{Access, CrudService};

/// A GraphQL `forbidden` error (code `FORBIDDEN`), matching the one used by the
/// `nestrs_authz::graphql` gate so callers see the same shape on a denial.
fn forbidden() -> Error {
    Error::new("forbidden").extend_with(|_, e| e.set("code", "FORBIDDEN"))
}

/// Turn a by-id argument into the loaded, authorized entity, so a by-id
/// resolver is a single call instead of a manual parse + load + ability check —
/// the resolver analog of the controller's `Bind<S, A>` parameter. Parses the
/// id as a UUID v7 (a bad id errors), then loads + authorizes **through the
/// entity's service** ([`CrudService::access`]) — the single audited gateway,
/// so the load joins the request transaction and the denial is logged like any
/// other access — resolving `Arc<S>` from the container in the GraphQL context:
///
/// - no such row → `Ok(None)` (a nullable `user(id)` field resolves to `null`);
/// - the row exists but the ability denies it → a `FORBIDDEN` error (existence
///   is not hidden, matching the HTTP `Bind`);
/// - otherwise → `Ok(Some(model))`.
///
/// Requires the ambient ability (so it doubles as the auth gate — no ability
/// means `FORBIDDEN`); the route needs the GraphQL auth bridge that installs it.
pub async fn bind<S, A>(
    ctx: &Context<'_>,
    id: &str,
) -> Result<Option<<S::Entity as EntityTrait>::Model>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    // Gate: no ambient ability (anonymous, or the auth bridge is not installed)
    // → FORBIDDEN, before any load. The bridge installs it ambient for `access`.
    if ctx.data_opt::<std::sync::Arc<nestrs_authz::Ability>>().is_none() {
        return Err(Error::new(
            "missing request `Ability` — is the GraphQL auth bridge installed on /graphql?",
        ));
    }
    let id = Uuid::parse_str(id).map_err(|err| Error::new(err.to_string()))?;
    if id.get_version_num() != 7 {
        return Err(Error::new("id must be a UUID v7"));
    }
    let service = ctx
        .data_unchecked::<Container>()
        .get::<S>()
        .ok_or_else(|| Error::new("no provider registered for the bound service"))?;
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
