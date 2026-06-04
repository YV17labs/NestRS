use nestrs_authz::{AbilityBuilder, AbilityFactory, Action};
use nestrs_core::injectable;

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
