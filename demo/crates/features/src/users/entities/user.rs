use nest_rs::seaorm::{expose, wire_enum};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[wire_enum]
#[derive(Default, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    #[default]
    #[sea_orm(string_value = "user")]
    User,
    #[sea_orm(string_value = "admin")]
    Admin,
}

#[expose(
    name = "User",
    service = super::super::service::UsersService,
    graphql,
    soft_delete,
    timestamps
)]
#[sea_orm::model]
#[derive(Clone, DeriveEntityModel)]
#[sea_orm(
    table_name = "user",
    model_attrs(derive(PartialEq, Serialize, Deserialize))
)]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
    #[sea_orm(unique)]
    #[expose(input(create, update), validate(email))]
    pub email: String,
    #[wire_default]
    pub role: UserRole,
    pub password_hash: Option<String>,
    #[expose]
    pub created_at: DateTimeWithTimeZone,
    #[expose]
    pub updated_at: DateTimeWithTimeZone,
    pub deleted_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(belongs_to, from = "org_id", to = "id")]
    #[expose]
    pub org: HasOne<crate::orgs::Entity>,
    #[sea_orm(has_many)]
    #[expose]
    pub posts: HasMany<crate::posts::Entity>,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            id,
            org_id,
            name,
            email,
            role,
            password_hash: _,
            created_at,
            updated_at,
            deleted_at,
        } = self;
        f.debug_struct("Model")
            .field("id", id)
            .field("org_id", org_id)
            .field("name", name)
            .field("email", email)
            .field("role", role)
            .field("password_hash", &"<redacted>")
            .field("created_at", created_at)
            .field("updated_at", updated_at)
            .field("deleted_at", deleted_at)
            .finish()
    }
}

impl std::fmt::Debug for ModelEx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            id,
            org_id,
            name,
            email,
            role,
            password_hash: _,
            created_at,
            updated_at,
            deleted_at,
            org,
            posts,
        } = self;
        f.debug_struct("ModelEx")
            .field("id", id)
            .field("org_id", org_id)
            .field("name", name)
            .field("email", email)
            .field("role", role)
            .field("password_hash", &"<redacted>")
            .field("created_at", created_at)
            .field("updated_at", updated_at)
            .field("deleted_at", deleted_at)
            .field("org", org)
            .field("posts", posts)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use nest_rs::authz::WireModelDefaults;
    use serde_json::Map;

    use super::*;

    #[test]
    fn wire_defaults_fill_in_role_and_password_hash_when_absent() {
        let mut body: Map<String, serde_json::Value> = Map::new();
        Entity::fill_wire_defaults(&mut body);

        assert_eq!(
            body.get("role"),
            Some(&serde_json::Value::String("user".into()))
        );
        assert_eq!(body.get("password_hash"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn debug_redacts_the_password_hash() {
        let model = Model {
            id: Uuid::nil(),
            org_id: Uuid::nil(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            role: UserRole::User,
            password_hash: Some("$argon2id$v=19$secret".into()),
            created_at: DateTimeWithTimeZone::default(),
            updated_at: DateTimeWithTimeZone::default(),
            deleted_at: None,
        };

        for printed in [format!("{model:?}"), format!("{:?}", ModelEx::from(model))] {
            assert!(!printed.contains("argon2id"), "{printed}");
            assert!(
                printed.contains("password_hash: \"<redacted>\""),
                "{printed}"
            );
            assert!(printed.contains("ada@example.com"), "{printed}");
        }
    }

    #[test]
    fn wire_defaults_do_not_overwrite_already_present_keys() {
        let mut body: Map<String, serde_json::Value> = Map::new();
        body.insert("role".into(), serde_json::Value::String("admin".into()));
        Entity::fill_wire_defaults(&mut body);

        assert_eq!(body["role"], serde_json::Value::String("admin".into()));
    }
}
