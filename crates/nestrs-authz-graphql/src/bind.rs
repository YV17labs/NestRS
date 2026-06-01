//! [`bind`] — route-model binding for resolvers, the GraphQL analog of
//! `nestrs_authz_http::Bind<S, A>`.

use nestrs_authz::ActionMarker;
use nestrs_core::Container;
use nestrs_database::{Access, CrudService};
use nestrs_graphql::async_graphql::{Context, Error, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::context::{ability, forbidden};

/// Turn a by-id argument into the loaded, authorized entity, so a by-id resolver
/// is a single call instead of a manual parse + load + ability check — the
/// resolver analog of the controller's `Bind<S, A>` parameter. Parses the id as a
/// UUID v7 (a bad id errors), then loads + authorizes **through the entity's
/// service** ([`CrudService::access`]) — the single audited gateway, so the load
/// joins the request transaction and the denial is logged like any other access —
/// resolving `Arc<S>` from the container in the GraphQL context:
///
/// - no such row → `Ok(None)` (a nullable `user(id)` field resolves to `null`);
/// - the row exists but the ability denies it → a `FORBIDDEN` error (existence is
///   not hidden, matching the HTTP `Bind`);
/// - otherwise → `Ok(Some(model))`.
///
/// Requires the ambient ability (so it doubles as the auth gate — no ability means
/// `FORBIDDEN`); the route needs the GraphQL auth bridge that installs it.
pub async fn bind<S, A>(
    ctx: &Context<'_>,
    id: &str,
) -> Result<Option<<S::Entity as EntityTrait>::Model>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    // Gate: no ambient ability (anonymous, or the auth bridge is not installed) →
    // FORBIDDEN, before any load. The bridge installs it ambient for `access` too.
    ability(ctx)?;
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
