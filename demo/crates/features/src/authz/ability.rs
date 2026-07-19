use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action};
use nest_rs_core::injectable;

use crate::Claims;
use crate::notifications as notification;
use crate::orgs as org;
use crate::posts as post;
use crate::users as user;

#[injectable]
#[derive(Default)]
pub struct AppAbility;

impl AbilityFactory for AppAbility {
    type Actor = Claims;

    fn define(&self, actor: &Claims, ab: &mut AbilityBuilder) {
        if actor.is_admin() {
            ab.can(Action::Read, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, org::Entity);
            ab.can(Action::Read, post::Entity)
                .when(|p| p.eq(post::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, post::Entity)
                .when(|p| p.eq(post::Column::OrgId, actor.org_id));
            ab.can(Action::Read, notification::Entity)
                .when(|p| p.eq(notification::Column::OrgId, actor.org_id));
        } else {
            ab.can(Action::Read, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id))
                .fields([user::Column::Id, user::Column::Name]);
            ab.can(Action::Create, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Read, org::Entity)
                .when(|p| p.eq(org::Column::Id, actor.org_id));
            ab.can(Action::Read, post::Entity)
                .when(|p| p.eq(post::Column::OrgId, actor.org_id));
            ab.can(Action::Create, post::Entity)
                .when(|p| p.eq(post::Column::OrgId, actor.org_id));
            ab.can(Action::Read, notification::Entity)
                .when(|p| p.eq(notification::Column::OrgId, actor.org_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_authz::{Ability, AbilityBuilder, FieldSet};
    use sea_orm::{DatabaseBackend, EntityTrait, QueryFilter, QueryTrait};
    use uuid::Uuid;

    use super::*;
    use crate::identity::Role;

    fn ability_for(roles: Vec<Role>, org_id: Uuid) -> Ability {
        let claims = Claims {
            sub: Some(Uuid::nil()),
            org_id,
            roles,
            exp: 0,
        };
        let mut b = AbilityBuilder::new();
        AppAbility.define(&claims, &mut b);
        b.build().expect("valid test ability")
    }

    fn admin(org_id: Uuid) -> Ability {
        ability_for(vec![Role::Admin], org_id)
    }

    fn member(org_id: Uuid) -> Ability {
        ability_for(vec![Role::User], org_id)
    }

    fn user_model(id: Uuid, org_id: Uuid) -> user::Model {
        let now = chrono::Utc::now().fixed_offset();
        user::Model {
            id,
            org_id,
            name: "Bob".into(),
            email: "bob@example.com".into(),
            role: "user".into(),
            password_hash: Some("argon2id$...".into()),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn org_model(id: Uuid) -> org::Model {
        let now = chrono::Utc::now().fixed_offset();
        org::Model {
            id,
            name: "Acme".into(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    fn notification_model(id: Uuid, org_id: Uuid) -> notification::Model {
        notification::Model {
            id,
            org_id,
            message: "Post \"Hello\" was published".into(),
            created_at: chrono::Utc::now().fixed_offset(),
        }
    }

    fn post_model(id: Uuid, org_id: Uuid, author_id: Uuid) -> post::Model {
        let now = chrono::Utc::now().fixed_offset();
        post::Model {
            id,
            org_id,
            author_id,
            title: "Hello".into(),
            body: "World".into(),
            status: post::PostStatus::Published,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn admin_read_users_scopes_to_caller_org() {
        let org = Uuid::now_v7();
        let sql = user::Entity::find()
            .filter(admin(org).condition_for::<user::Entity>(Action::Read))
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            sql.contains("org_id"),
            "admin reads must still scope by org_id: {sql}",
        );
        assert!(
            !sql.contains("1 = 0"),
            "admin grant must not match nothing: {sql}",
        );
    }

    #[test]
    fn admin_can_manage_users_in_own_org() {
        let org = Uuid::now_v7();
        let ab = admin(org);
        assert!(ab.can::<user::Entity>(Action::Delete, &user_model(Uuid::now_v7(), org)));
        assert!(
            !ab.can::<user::Entity>(Action::Delete, &user_model(Uuid::now_v7(), Uuid::now_v7()))
        );
    }

    #[test]
    fn admin_can_manage_every_org_unconditionally() {
        let ab = admin(Uuid::now_v7());
        assert!(ab.can_class(Action::Read, TypeId::of::<org::Entity>()));
        assert!(ab.can_class(Action::Update, TypeId::of::<org::Entity>()));
        assert!(ab.can::<org::Entity>(Action::Read, &org_model(Uuid::now_v7())));
    }

    #[test]
    fn member_read_users_scopes_to_own_org_and_strips_email() {
        let org = Uuid::now_v7();
        let sql = user::Entity::find()
            .filter(member(org).condition_for::<user::Entity>(Action::Read))
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(sql.contains("org_id"), "member must scope by org_id: {sql}");

        let fields = member(org)
            .permitted_fields::<user::Entity>(Action::Read, &user_model(Uuid::now_v7(), org));
        match fields {
            FieldSet::Only(cols) => {
                assert!(cols.contains("id"));
                assert!(cols.contains("name"));
                assert!(!cols.contains("email"), "email is admin-only");
            }
            FieldSet::All => panic!("members must have a restricted field set"),
        }
    }

    #[test]
    fn member_cannot_delete_users() {
        let org = Uuid::now_v7();
        assert!(!member(org).can::<user::Entity>(Action::Delete, &user_model(Uuid::now_v7(), org)));
        let sql = user::Entity::find()
            .filter(member(org).condition_for::<user::Entity>(Action::Delete))
            .build(DatabaseBackend::Postgres)
            .to_string();
        assert!(
            sql.contains("1 = 0"),
            "no Delete grant for members ⇒ pre-filter matches nothing: {sql}",
        );
    }

    #[test]
    fn member_can_only_read_their_own_org() {
        let org = Uuid::now_v7();
        let other = Uuid::now_v7();
        let ab = member(org);
        assert!(ab.can::<org::Entity>(Action::Read, &org_model(org)));
        assert!(!ab.can::<org::Entity>(Action::Read, &org_model(other)));
    }

    #[test]
    fn member_mask_strips_admin_only_fields() {
        let org = Uuid::now_v7();
        let json = member(org).mask::<user::Entity>(Action::Read, &user_model(Uuid::now_v7(), org));
        let obj = json.as_object().expect("masked model is a JSON object");
        assert!(obj.contains_key("id"));
        assert!(obj.contains_key("name"));
        assert!(!obj.contains_key("email"), "members must not see email");
    }

    #[test]
    fn member_can_create_posts_in_own_org() {
        let org = Uuid::now_v7();
        let ab = member(org);
        assert!(ab.can_class(Action::Create, TypeId::of::<post::Entity>()));
        assert!(ab.can::<post::Entity>(
            Action::Create,
            &post_model(Uuid::now_v7(), org, Uuid::now_v7()),
        ));
        assert!(!ab.can::<post::Entity>(
            Action::Create,
            &post_model(Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()),
        ));
    }

    #[test]
    fn admin_manage_posts_scopes_to_caller_org() {
        let org = Uuid::now_v7();
        let ab = admin(org);
        assert!(ab.can::<post::Entity>(
            Action::Delete,
            &post_model(Uuid::now_v7(), org, Uuid::now_v7()),
        ));
        assert!(!ab.can::<post::Entity>(
            Action::Delete,
            &post_model(Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()),
        ));
    }

    #[test]
    fn notifications_are_read_scoped_to_the_callers_org_for_both_roles() {
        let org = Uuid::now_v7();
        let other = Uuid::now_v7();
        for ab in [admin(org), member(org)] {
            assert!(ab.can::<notification::Entity>(
                Action::Read,
                &notification_model(Uuid::now_v7(), org)
            ));
            assert!(
                !ab.can::<notification::Entity>(
                    Action::Read,
                    &notification_model(Uuid::now_v7(), other)
                ),
                "a notification in another org must not be readable",
            );
        }
    }

    #[test]
    fn notifications_are_never_writable_over_the_wire() {
        let org = Uuid::now_v7();
        for ab in [admin(org), member(org)] {
            for action in [Action::Create, Action::Update, Action::Delete] {
                let sql = notification::Entity::find()
                    .filter(ab.condition_for::<notification::Entity>(action))
                    .build(DatabaseBackend::Postgres)
                    .to_string();
                assert!(
                    sql.contains("1 = 0"),
                    "no {action:?} grant on notifications ⇒ pre-filter matches nothing: {sql}",
                );
            }
        }
    }

    #[test]
    fn empty_roles_behave_as_a_non_admin_member() {
        let org = Uuid::now_v7();
        let ab = ability_for(vec![], org);
        assert!(!ab.can::<user::Entity>(Action::Delete, &user_model(Uuid::now_v7(), org)));
        match ab.permitted_fields::<user::Entity>(Action::Read, &user_model(Uuid::now_v7(), org)) {
            FieldSet::Only(cols) => assert!(!cols.contains("email")),
            FieldSet::All => panic!("an empty-roles caller must have a restricted field set"),
        }
    }
}
