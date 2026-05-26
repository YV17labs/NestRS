//! The app's authorization policy. [`AppAbility`]'s rules drive all three
//! `nestrs-authz` enforcement layers: the `Authorize` route gate, the
//! `Ability::condition_for` query pre-filter, and the `Ability::mask` response
//! mask.

use nestrs_authz::{AbilityBuilder, AbilityFactory, Action};
use nestrs_core::injectable;

use crate::authn::AuthUser;
use crate::users::entity;

/// Maps a principal to its [`Ability`](nestrs_authz::Ability) — the app's
/// CASL-style rule set.
#[injectable]
#[derive(Default)]
pub struct AppAbility;

impl AbilityFactory for AppAbility {
    type Actor = AuthUser;

    fn define(&self, actor: &AuthUser, ab: &mut AbilityBuilder) {
        // Each branch is a complete statement so the rule commits (on drop)
        // before the builder is reused.
        if actor.is_admin() {
            // Admin: full control, but still scoped to its own org (no super-admin).
            ab.can(Action::Read, entity::Entity)
                .when(|p| p.eq(entity::Column::OrgId, actor.org_id));
            ab.can(Action::Manage, entity::Entity)
                .when(|p| p.eq(entity::Column::OrgId, actor.org_id));
        } else {
            // Plain user: read its org's users but not their email, and create.
            ab.can(Action::Read, entity::Entity)
                .when(|p| p.eq(entity::Column::OrgId, actor.org_id))
                .fields([entity::Column::Id, entity::Column::Name]);
            ab.can(Action::Create, entity::Entity)
                .when(|p| p.eq(entity::Column::OrgId, actor.org_id));
        }
    }
}
