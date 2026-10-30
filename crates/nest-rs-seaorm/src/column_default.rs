use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::OnceLock;

use sea_orm::EntityTrait;

/// One `#[expose]` entity's column defaults — a `Uuid` key's v7 UUID and each
/// `#[expose(default = …)]` — submitted at link time so that an insert through
/// [`Repo`](crate::Repo) fills what the row left unset, while the entity's
/// `ActiveModelBehavior` stays its own; emitted by the decorator, never by
/// hand.
///
/// Fn pointers, since each value is a call and the entry must be a `static`.
pub struct ColumnDefaultRegistration {
    /// `TypeId::of::<ActiveModel>()`, the type [`fill`](Self::fill) takes.
    pub active_model: fn() -> TypeId,
    /// Sets each defaulted column the `ActiveModel` leaves unset.
    pub fill: Fill,
}

/// A registration's fill, over the `ActiveModel` it downcasts to.
type Fill = fn(&mut dyn Any);

inventory::collect!(ColumnDefaultRegistration);

/// Every registered `ActiveModel`'s fill, read once from the inventory.
fn registry() -> &'static HashMap<TypeId, Fill> {
    static REGISTRY: OnceLock<HashMap<TypeId, Fill>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        inventory::iter::<ColumnDefaultRegistration>
            .into_iter()
            .map(|entry| ((entry.active_model)(), entry.fill))
            .collect()
    })
}

/// Fill each column default `E` registered that `active` leaves unset; an
/// entity `#[expose]` does not declare registers none, and passes untouched.
pub(crate) fn fill_column_defaults<E: EntityTrait>(active: &mut E::ActiveModel) {
    if let Some(fill) = registry().get(&TypeId::of::<E::ActiveModel>()) {
        fill(active);
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{ActiveValue, Set};

    use super::*;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
            pub stage: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod gadget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "gadgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: Uuid,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    // The shape `#[expose]` submits for a `Uuid` key and one `default`.
    inventory::submit! {
        ColumnDefaultRegistration {
            active_model: || TypeId::of::<widget::ActiveModel>(),
            fill: |active| {
                if let Some(active) = active.downcast_mut::<widget::ActiveModel>() {
                    if active.id.is_not_set() {
                        active.id = Set(sea_orm::prelude::Uuid::now_v7());
                    }
                    if active.stage.is_not_set() {
                        active.stage = Set("draft".to_owned());
                    }
                }
            },
        }
    }

    #[test]
    fn a_registered_entity_takes_its_defaults_where_the_row_left_them_unset() {
        let mut active = widget::ActiveModel::default();
        fill_column_defaults::<widget::Entity>(&mut active);

        let ActiveValue::Set(id) = active.id else {
            panic!("the key is filled: {active:?}");
        };
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(active.stage, Set("draft".to_owned()));
    }

    #[test]
    fn what_the_row_carries_is_kept() {
        let id = sea_orm::prelude::Uuid::now_v7();
        let mut active = widget::ActiveModel {
            id: Set(id),
            stage: Set("sent".to_owned()),
        };
        fill_column_defaults::<widget::Entity>(&mut active);

        assert_eq!(active.id, Set(id));
        assert_eq!(active.stage, Set("sent".to_owned()));
    }

    #[test]
    fn an_entity_with_no_registration_passes_untouched() {
        let mut active = gadget::ActiveModel::default();
        fill_column_defaults::<gadget::Entity>(&mut active);

        assert_eq!(active.id, ActiveValue::NotSet);
    }
}
