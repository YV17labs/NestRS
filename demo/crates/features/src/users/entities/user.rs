use nest_rs_resource::expose;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// The user's authorization role, backed 1:1 by the existing `role` varchar
/// (`"user"`/`"admin"`). Being a typed enum is what makes an unknown DB string
/// **fail to load** (a `DbErr`) rather than silently demote to `User` — the
/// fail-closed posture the plain `String` column could not give. Distinct from
/// the JWT-scope [`crate::Role`]; `oauth::role_from_db` maps this into that.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, EnumIter, DeriveActiveEnum,
)]
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
#[derive(Clone, Debug, DeriveEntityModel)]
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

#[cfg(test)]
mod tests {
    use nest_rs_resource::WireModelDefaults;
    use serde_json::Map;

    use super::*;

    #[test]
    fn wire_defaults_fill_in_role_and_password_hash_when_absent() {
        let mut body: Map<String, serde_json::Value> = Map::new();
        Entity::fill_wire_defaults(&mut body);

        // `role` is a custom enum the built-in emitter can't default — the
        // `#[wire_default]` opt-in supplies `UserRole::default()` ⇒ `"user"`.
        assert_eq!(
            body.get("role"),
            Some(&serde_json::Value::String("user".into()))
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
