use nest_rs::authz::{AbilityBuilder, AbilityFactory, Action};
use nest_rs::core::injectable;

use super::constants;
use crate::Claims;
use crate::audio::Transcoding;
use crate::notifications as notification;
use crate::orgs as org;
use crate::posts as post;
use crate::users as user;

#[injectable]
pub struct AuthzAbility;

impl AbilityFactory for AuthzAbility {
    type Actor = Claims;

    fn define(&self, actor: &Claims, ab: &mut AbilityBuilder) {
        if actor.is_admin() {
            ab.can(Action::Read, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, org::Entity);
            ab.can(Action::Manage, Transcoding)
                .requires_scope(constants::AUDIO_TRANSCODE);
            ab.can(Action::Read, post::Entity)
                .when(|p| p.eq(post::Column::OrgId, actor.org_id))
                .requires_scope(constants::POSTS_READ);
            for action in [Action::Update, Action::Delete] {
                ab.can(action, post::Entity)
                    .when(|p| p.eq(post::Column::OrgId, actor.org_id))
                    .requires_scope(constants::POSTS_WRITE);
            }
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
                .when(|p| p.eq(post::Column::OrgId, actor.org_id))
                .requires_scope(constants::POSTS_READ);
            ab.can(Action::Read, notification::Entity)
                .when(|p| p.eq(notification::Column::OrgId, actor.org_id));
        }
        if let Some(author) = actor.sub.user_id() {
            ab.can(Action::Create, post::Entity)
                .when(|p| {
                    p.all([
                        p.eq(post::Column::OrgId, actor.org_id),
                        p.eq(post::Column::AuthorId, author),
                    ])
                })
                .requires_scope(constants::POSTS_WRITE);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::authz::{Ability, AbilityBuilder, FieldSet};
    use sea_orm::{DatabaseBackend, EntityTrait, IdenStatic, QueryFilter, QueryTrait};
    use uuid::Uuid;

    use super::*;
    use crate::authn::{Role, Subject};

    fn ability_for(roles: Vec<Role>, org_id: Uuid) -> Ability {
        ability_scoped(roles, org_id, constants::all())
    }

    fn ability_scoped(roles: Vec<Role>, org_id: Uuid, scopes: Vec<String>) -> Ability {
        ability_of(roles, org_id, scopes, Subject::User(Uuid::nil()))
    }

    fn ability_of(roles: Vec<Role>, org_id: Uuid, scopes: Vec<String>, sub: Subject) -> Ability {
        let claims = Claims {
            sub,
            client_id: None,
            org_id,
            roles,
            scopes: scopes.clone(),
            exp: 0,
        };
        let mut b = AbilityBuilder::new().with_granted_scopes(Some(scopes.into()));
        AuthzAbility.define(&claims, &mut b);
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
            role: user::UserRole::User,
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
            _ => panic!("members must have a restricted field set"),
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
    fn a_user_authors_posts_in_their_own_org_alone() {
        let org = Uuid::now_v7();
        for ab in [admin(org), member(org)] {
            assert!(ab.can_class(Action::Create, TypeId::of::<post::Entity>()));
            assert!(ab.can::<post::Entity>(
                Action::Create,
                &post_model(Uuid::now_v7(), org, Uuid::nil()),
            ));
            assert!(!ab.can::<post::Entity>(
                Action::Create,
                &post_model(Uuid::now_v7(), Uuid::now_v7(), Uuid::nil()),
            ));
            assert!(!ab.can::<post::Entity>(
                Action::Create,
                &post_model(Uuid::now_v7(), org, Uuid::now_v7()),
            ));
        }
    }

    #[test]
    fn a_create_takes_the_callers_org_and_authorship_from_the_grants() {
        let org = Uuid::now_v7();
        for ab in [admin(org), member(org)] {
            let pinned: Vec<_> = ab
                .pinned_values::<post::Entity>(Action::Create)
                .into_iter()
                .map(|(column, value)| (column.as_str(), value))
                .collect();
            assert_eq!(
                pinned,
                [("org_id", org.into()), ("author_id", Uuid::nil().into())]
            );
            let pinned: Vec<_> = ab
                .pinned_values::<user::Entity>(Action::Create)
                .into_iter()
                .map(|(column, value)| (column.as_str(), value))
                .collect();
            assert_eq!(pinned, [("org_id", org.into())]);
        }
    }

    #[test]
    fn a_machine_authors_no_post_and_no_wider_token_would_change_it() {
        let org = Uuid::now_v7();
        let ab = ability_of(
            vec![Role::Admin],
            org,
            constants::all(),
            Subject::Client("ci".into()),
        );
        assert!(!ab.can_class(Action::Create, TypeId::of::<post::Entity>()));
        assert!(
            ab.missing_scopes(Action::Create, TypeId::of::<post::Entity>())
                .is_empty()
        );
        assert!(ab.can::<post::Entity>(
            Action::Update,
            &post_model(Uuid::now_v7(), org, Uuid::now_v7()),
        ));
    }

    #[test]
    fn admin_update_and_delete_of_posts_scope_to_caller_org_whoever_wrote_them() {
        let org = Uuid::now_v7();
        let ab = admin(org);
        for action in [Action::Update, Action::Delete] {
            assert!(
                ab.can::<post::Entity>(action, &post_model(Uuid::now_v7(), org, Uuid::now_v7()))
            );
            assert!(!ab.can::<post::Entity>(
                action,
                &post_model(Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()),
            ));
        }
    }

    #[test]
    fn a_read_only_delegation_cannot_write_even_as_admin() {
        let org = Uuid::now_v7();
        let ab = ability_scoped(
            vec![Role::Admin],
            org,
            vec![constants::POSTS_READ.to_owned()],
        );

        assert!(
            ab.can_class(Action::Read, TypeId::of::<post::Entity>()),
            "the scope it does carry still works",
        );
        for action in [Action::Create, Action::Update, Action::Delete] {
            assert!(!ab.can_class(action, TypeId::of::<post::Entity>()));
            assert_eq!(
                ab.missing_scopes(action, TypeId::of::<post::Entity>()),
                [constants::POSTS_WRITE],
                "and the refusal names what to go ask the authorization server for",
            );
        }
    }

    #[test]
    fn a_delegation_carrying_no_post_scope_reads_nothing() {
        let org = Uuid::now_v7();
        let ab = ability_scoped(
            vec![Role::Admin],
            org,
            vec![constants::AUDIO_TRANSCODE.to_owned()],
        );

        assert!(!ab.can_class(Action::Read, TypeId::of::<post::Entity>()));
        assert!(
            !ab.can::<post::Entity>(
                Action::Read,
                &post_model(Uuid::now_v7(), org, Uuid::now_v7()),
            ),
            "the withheld rule must not leak through the row check either",
        );
    }

    #[test]
    fn an_admin_token_granted_the_transcode_scope_may_transcode() {
        let ab = admin(Uuid::now_v7());
        assert!(ab.can_class(Action::Manage, TypeId::of::<Transcoding>()));
    }

    #[test]
    fn a_member_may_not_transcode_and_no_wider_token_would_change_it() {
        let ab = member(Uuid::now_v7());
        assert!(!ab.can_class(Action::Manage, TypeId::of::<Transcoding>()));
        assert!(
            ab.missing_scopes(Action::Manage, TypeId::of::<Transcoding>())
                .is_empty()
        );
    }

    #[test]
    fn an_admin_token_narrowed_to_reading_posts_lacks_the_transcode_scope() {
        let ab = ability_scoped(
            vec![Role::Admin],
            Uuid::now_v7(),
            vec![constants::POSTS_READ.to_owned()],
        );
        assert!(!ab.can_class(Action::Manage, TypeId::of::<Transcoding>()));
        assert_eq!(
            ab.missing_scopes(Action::Manage, TypeId::of::<Transcoding>()),
            [constants::AUDIO_TRANSCODE],
        );
    }

    #[test]
    fn an_admin_whose_credential_is_not_scope_aware_may_transcode() {
        let claims = Claims {
            sub: Subject::User(Uuid::nil()),
            client_id: None,
            org_id: Uuid::now_v7(),
            roles: vec![Role::Admin],
            scopes: Vec::new(),
            exp: 0,
        };
        let mut b = AbilityBuilder::new();
        AuthzAbility.define(&claims, &mut b);
        let ab = b.build().expect("valid test ability");
        assert!(ab.can_class(Action::Manage, TypeId::of::<Transcoding>()));
    }

    #[test]
    fn scopes_never_widen_what_the_role_denies() {
        let org = Uuid::now_v7();
        let ab = ability_scoped(vec![Role::User], org, constants::all());
        assert!(!ab.can_class(Action::Manage, TypeId::of::<user::Entity>()));
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
            _ => panic!("an empty-roles caller must have a restricted field set"),
        }
    }
}
