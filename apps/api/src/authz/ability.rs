//! The app's authorization policy. [`AppAbility`]'s rules drive all three
//! `nestrs-authz` enforcement layers: the `Authorize` route gate, the
//! `Ability::condition_for` query pre-filter, and the `Ability::mask` response
//! mask.

use identity::Claims;
use nestrs_authz::{AbilityBuilder, AbilityFactory, Action};
use nestrs_core::injectable;

use crate::orgs::entity as org;
use crate::users::entity as user;

#[injectable]
#[derive(Default)]
pub struct AppAbility;

impl AbilityFactory for AppAbility {
    // The verified token `Claims` is the caller (`api` is a resource server).
    type Actor = Claims;

    fn define(&self, actor: &Claims, ab: &mut AbilityBuilder) {
        // Each branch is a complete statement so the rule commits (on drop)
        // before the builder is reused.
        if actor.is_admin() {
            // Admin: full control over its own org's users (no super-admin).
            ab.can(Action::Read, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            // Orgs are the tenant root, not tenant-scoped data: an admin is the
            // control plane and manages every org (list all, create, read any).
            ab.can(Action::Manage, org::Entity);
        } else {
            // Plain user: read its org's users but not their email, and create.
            ab.can(Action::Read, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id))
                .fields([user::Column::Id, user::Column::Name]);
            ab.can(Action::Create, user::Entity)
                .when(|p| p.eq(user::Column::OrgId, actor.org_id));
            // A tenant member reads only its own org — row-level scoping, by id.
            ab.can(Action::Read, org::Entity)
                .when(|p| p.eq(org::Column::Id, actor.org_id));
        }
    }
}
