//! The load `#[authorize(A, bind = S)]` runs on an edge that names a subject's
//! id in its address: the row, through service `S` under the unit's ability.
//! Each edge says what a refusal answers there.

use std::any::type_name;
use std::error::Error;

use nest_rs_authz::{ActionMarker, current_ability};
use sea_orm::{DbErr, EntityTrait, PrimaryKeyTrait};
use uuid::Uuid;

use crate::error::{UnwiredBind, UnwiredResource};
use crate::{Access, Authorized, CrudService};

/// Why no subject is handed over.
pub(crate) enum Unbound {
    /// The id is not a UUID v7.
    NotV7,
    /// What the unit needs to load it is missing, which its edge files at
    /// `error`, the service beside it.
    Unwired {
        why: UnwiredBind,
        provider: &'static str,
    },
    /// The service failed reading the row.
    Failed(DbErr),
    /// The row exists and the caller may not have it.
    Denied,
    /// No such row.
    Missing,
}

/// The row `id` names, loaded through `S` under the unit's ability.
pub(crate) async fn load<A, S>(id: Uuid) -> Result<Authorized<A, S::Entity>, Unbound>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    if id.get_version_num() != 7 {
        return Err(Unbound::NotV7);
    }
    let unwired = |why| Unbound::Unwired {
        why,
        provider: type_name::<S>(),
    };
    if current_ability().is_none() {
        return Err(unwired(UnwiredBind::Ability));
    }
    let Some(scope) = nest_rs_core::current_request_scope() else {
        return Err(unwired(UnwiredBind::RequestScope));
    };
    let Some(service) = scope.get::<S>() else {
        return Err(unwired(UnwiredBind::Provider));
    };
    match service.access(A::ACTION, id).await {
        Ok(Access::Found(model)) => Ok(Authorized::new(model)),
        Ok(Access::Denied) => Err(Unbound::Denied),
        Ok(Access::Missing) => Err(Unbound::Missing),
        Err(err) => Err(Unbound::Failed(err)),
    }
}

/// The subject `#[authorize(A, bind = S)]` loads for a `#[resource]`, the row
/// its template's one variable names, handed to its `Authorized<A, E>`
/// parameter. `None` for an id that is no v7, a row the caller may not have and
/// one that does not exist: one answer, so a reader cannot tell them apart. An
/// `Err` is what the edge answers opaquely and files.
pub async fn resource_bind<A, S>(
    id: Uuid,
) -> Result<Option<Authorized<A, S::Entity>>, Box<dyn Error + Send + Sync>>
where
    S: CrudService + 'static,
    <S::Entity as EntityTrait>::PrimaryKey: PrimaryKeyTrait<ValueType = Uuid>,
    A: ActionMarker,
{
    match load::<A, S>(id).await {
        Ok(subject) => Ok(Some(subject)),
        Err(Unbound::NotV7 | Unbound::Denied | Unbound::Missing) => Ok(None),
        Err(Unbound::Unwired { why, provider }) => Err(Box::new(UnwiredResource { provider, why })),
        Err(Unbound::Failed(err)) => Err(Box::new(err)),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_authz::Read;

    use super::*;

    mod note {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "notes")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    struct NotesService;

    impl CrudService for NotesService {
        type Entity = note::Entity;
    }

    #[tokio::test]
    async fn a_resource_that_cannot_load_says_which_service_and_what_its_unit_lacks() {
        let Err(unwired) = resource_bind::<Read, NotesService>(Uuid::now_v7()).await else {
            panic!("no ability is ambient");
        };
        let said = nest_rs_core::error_message(unwired.as_ref());
        assert!(said.contains("NotesService"), "{said}");
        assert!(said.contains("ability guard"), "{said}");
    }
}
