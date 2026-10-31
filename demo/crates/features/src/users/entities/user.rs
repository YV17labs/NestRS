use nest_rs::core::Secret;
use nest_rs::seaorm::expose;
use sea_orm::entity::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum UserRole {
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
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[expose]
    pub id: Uuid,
    #[expose]
    pub org_id: Uuid,
    #[expose(input(create, update), validate(length(min = 1)))]
    pub name: String,
    #[expose(input(create, update), validate(email))]
    pub email: String,
    #[wire_default(UserRole::User)]
    pub role: UserRole,
    pub password_hash: Option<Secret<String>>,
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
    use super::*;

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
                printed.contains("password_hash: Some(<redacted>)"),
                "{printed}"
            );
            assert!(printed.contains("ada@example.com"), "{printed}");
        }
    }

    #[test]
    fn active_models_debug_redacts_the_password_hash() {
        let active = ActiveModel {
            password_hash: sea_orm::Set(Some("$argon2id$v=19$secret".into())),
            ..Default::default()
        };

        for printed in [
            format!("{active:?}"),
            format!("{:?}", ActiveModelEx::from(active.clone())),
        ] {
            assert!(!printed.contains("argon2id"), "{printed}");
            assert!(printed.contains("password_hash"), "{printed}");
        }
    }
}
