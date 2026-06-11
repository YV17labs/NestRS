use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[expose(name = "User", service = super::service::UsersService, graphql)]
#[sea_orm::model]
#[derive(Clone, Debug, DeriveEntityModel)]
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
    #[expose(skip)]
    pub role: String,
    #[expose(skip)]
    pub password_hash: Option<String>,
    #[sea_orm(belongs_to, from = "org_id", to = "id")]
    pub org: HasOne<crate::orgs::Entity>,
    #[sea_orm(has_many)]
    pub posts: HasMany<crate::posts::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use nest_rs_resource::WireModelDefaults;
    use serde_json::Map;

    use super::*;

    #[test]
    fn wire_defaults_fill_in_role_and_password_hash_when_absent() {
        let mut body: Map<String, serde_json::Value> = Map::new();
        Entity::fill_wire_defaults(&mut body);

        assert_eq!(
            body.get("role"),
            Some(&serde_json::Value::String(String::new()))
        );
        assert_eq!(body.get("password_hash"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn wire_defaults_do_not_overwrite_already_present_keys() {
        let mut body: Map<String, serde_json::Value> = Map::new();
        body.insert("role".into(), serde_json::Value::String("admin".into()));
        Entity::fill_wire_defaults(&mut body);

        assert_eq!(body["role"], serde_json::Value::String("admin".into()));
    }
}
