use nest_rs_authz::ActionMarker;
use nest_rs_core::Container;
use nest_rs_graphql::async_graphql::{Context, Error, ErrorExtensions, Result};
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use nest_rs_authz::graphql::forbidden;

use crate::{Access, Authorized, CrudService};

/// Parse a by-id argument as a UUID v7, or refuse with the stable code
/// `INVALID_ARGUMENT`; `#[crud]` emits calls to it.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal is the client's 400: it names the expected shape, never a parser's internals"
)]
pub fn parse_v7(id: &str) -> Result<Uuid> {
    let invalid = || {
        Error::new(nest_rs_core::UUID_V7_REQUIRED)
            .extend_with(|_, e| e.set("code", "INVALID_ARGUMENT"))
    };
    let parsed = Uuid::parse_str(id).map_err(|_| invalid())?;
    if parsed.get_version_num() != 7 {
        return Err(invalid());
    }
    Ok(parsed)
}

/// Logged here: GraphQL's error path has no `ResponseError` that would log it.
fn internal(service: &'static str, err: &sea_orm::DbErr) -> Error {
    tracing::error!(
        target: crate::TARGET,
        service,
        error = %nest_rs_core::error_message(err),
        "by-id access load failed",
    );
    Error::new(nest_rs_core::OPAQUE_CLIENT_MESSAGE)
        .extend_with(|_, e| e.set("code", "INTERNAL_SERVER_ERROR"))
}

/// Turn a by-id argument into the loaded, authorized entity (the resolver
/// analog of [`crate::Bind`]). Outcomes: no row → `Ok(None)`; denied →
/// `FORBIDDEN` (existence not hidden); else `Ok(Some(model))`. Without an
/// ambient ability it errors rather than behave as anonymous.
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
    if ctx
        .data_opt::<std::sync::Arc<nest_rs_authz::Ability>>()
        .is_none()
    {
        return Err(Error::new(
            "missing request `Ability` — is the GraphQL auth bridge installed on /graphql?",
        ));
    }
    let id = parse_v7(id)?;
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

/// Bind a **mutation subject** as an [`Authorized`] proof: like [`bind`], but a
/// missing row is a `NOT_FOUND` error.
pub async fn bind_required<A, S>(ctx: &Context<'_>, id: &str) -> Result<Authorized<A, S::Entity>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    not_found_to_err(bind::<A, S>(ctx, id).await?)
}

fn not_found_to_err<A: ActionMarker, E: EntityTrait>(
    model: Option<E::Model>,
) -> Result<Authorized<A, E>> {
    match model {
        Some(model) => Ok(Authorized::new(model)),
        None => Err(Error::new("not found").extend_with(|_, e| e.set("code", "NOT_FOUND"))),
    }
}
