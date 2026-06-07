use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action};
use nest_rs_core::injectable;

use crate::Claims;
use crate::orgs as org;
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
        } else {
            ab.can(Action::Read, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id))
                .fields([user::Column::Id, user::Column::Name]);
            ab.can(Action::Create, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Read, org::Entity)
                .when(|p| p.eq(org::Column::Id, actor.org_id));
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
        b.build()
    }

    fn admin(org_id: Uuid) -> Ability {
        ability_for(vec![Role::Admin], org_id)
    }

    fn member(org_id: Uuid) -> Ability {
        ability_for(vec![Role::User], org_id)
    }

    fn user_model(id: Uuid, org_id: Uuid) -> user::Model {
        user::Model {
            id,
            org_id,
            name: "Bob".into(),
            email: "bob@example.com".into(),
            role: "user".into(),
            password_hash: Some("argon2id$...".into()),
        }
    }

    fn org_model(id: Uuid) -> org::Model {
        org::Model {
            id,
            name: "Acme".into(),
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
        // `Manage` on orgs has no `when` — admins read every org.
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
    fn empty_roles_behave_as_a_non_admin_member() {
        // Defence in depth: a token with no role must not silently get admin powers.
        let org = Uuid::now_v7();
        let ab = ability_for(vec![], org);
        assert!(!ab.can::<user::Entity>(Action::Delete, &user_model(Uuid::now_v7(), org)));
        // Read users still scoped to own org and restricted to {id, name}.
        match ab.permitted_fields::<user::Entity>(Action::Read, &user_model(Uuid::now_v7(), org)) {
            FieldSet::Only(cols) => assert!(!cols.contains("email")),
            FieldSet::All => panic!("an empty-roles caller must have a restricted field set"),
        }
    }
}
