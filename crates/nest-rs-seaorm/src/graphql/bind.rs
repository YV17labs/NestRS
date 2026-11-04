use nest_rs_authz::ActionMarker;
use nest_rs_core::problem::code;
use nest_rs_core::{Container, Problem};
use nest_rs_graphql::async_graphql::{Context, Error, Result};
use nest_rs_graphql::problem_error;
use sea_orm::{EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use nest_rs_authz::graphql::forbidden;

use crate::error::DbOutcome;
use crate::{Access, Authorized, CrudService, ServiceError};

/// Parse a by-id argument as a UUID v7, or refuse with the stable code
/// `INVALID_ARGUMENT`; `#[crud]` emits calls to it.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal is the client's 400: it names the expected shape, never a parser's internals"
)]
pub fn parse_v7(id: &str) -> Result<Uuid> {
    let invalid = || {
        problem_error(
            &Problem::new(400, code::INVALID_ARGUMENT).with_detail(nest_rs_core::UUID_V7_REQUIRED),
        )
    };
    let parsed = Uuid::parse_str(id).map_err(|_| invalid())?;
    if parsed.get_version_num() != 7 {
        return Err(invalid());
    }
    Ok(parsed)
}

/// What a load that failed answers: a statement cancelled at its bound the class a
/// route's answers, and anything else the opaque problem. Logged here: that bare
/// problem carries no chain for the edge to file.
fn load_failed(service: &'static str, err: sea_orm::DbErr) -> Error {
    if DbOutcome::of(&err) == DbOutcome::Unavailable {
        return problem_error(&ServiceError::Db(err));
    }
    tracing::error!(
        target: crate::target::ORM,
        service,
        error = %nest_rs_core::error_message(&err),
        "by-id access load failed",
    );
    problem_error(&Problem::new(500, code::INTERNAL))
}

/// The subject `#[authorize(A, bind = S)]` loads for an operation, handed to
/// its `Authorized<A, E>` parameter: the row a by-id argument names, loaded
/// and authorized. No row → `NOT_FOUND`; denied → `FORBIDDEN` (existence not
/// hidden).
///
/// The service resolves through the request's scope — the container's over a
/// graphql-ws socket, which carries none. A wiring defect — no provider for
/// it, no ability on the field's unit — answers the opaque internal error and
/// is filed at `error`, never as anonymous.
pub async fn graphql_bind<A, S>(ctx: &Context<'_>, id: &str) -> Result<Authorized<A, S::Entity>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    let service = match nest_rs_core::current_request_scope() {
        Some(scope) => scope.get::<S>(),
        None => ctx.data_unchecked::<Container>().get::<S>(),
    };
    let Some(service) = service else {
        tracing::error!(
            target: crate::target::ORM,
            provider = std::any::type_name::<S>(),
            "no provider for the bound service — add it to a module's providers",
        );
        return Err(problem_error(&Problem::new(500, code::INTERNAL)));
    };
    nest_rs_authz::graphql::ability()?;
    let id = parse_v7(id)?;
    match service
        .access(A::ACTION, id)
        .await
        .map_err(|err| load_failed(std::any::type_name::<S>(), err))?
    {
        Access::Found(model) => Ok(Authorized::new(model)),
        Access::Denied => Err(forbidden()),
        Access::Missing => Err(problem_error(&Problem::new(404, code::NOT_FOUND))),
    }
}
