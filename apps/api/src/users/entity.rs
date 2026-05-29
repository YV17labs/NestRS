use nestrs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "User", complex)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
// `Serialize`/`Deserialize`/`PartialEq` go on the plain `Model` only: the
// generated `ModelEx` holds the `HasOne`/`HasMany` relation fields, whose serde
// impls are mutually recursive across the two entities and cannot be derived.
#[sea_orm(
    table_name = "user",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
    #[sea_orm(unique)]
    #[expose(input(create, update), validate(email))]
    pub email: String,
    #[sea_orm(belongs_to, from = "org_id", to = "id")]
    #[expose(skip)]
    pub org: HasOne<crate::orgs::entity::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
