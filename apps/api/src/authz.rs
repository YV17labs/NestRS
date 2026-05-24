//! Application authorization: the [`AbilityFactory`] mapping a principal to its
//! ability, and the guard that builds it per request.
//!
//! Rules are declared once here; the three layers consume the result — the
//! `Authorize` route gate (class check), the query pre-filter
//! (`Ability::condition_for`), and the response mask (`Ability::mask`).

use std::sync::Arc;

use nestrs_authz::{AbilityBuilder, AbilityFactory, Action};
use nestrs_core::injectable;
use nestrs_http::{async_trait, Guard};
use poem::http::StatusCode;
use poem::{Request, Response};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::users::entity;

/// Seed tenants, so the org-scoped filter is observable: org-scoped reads from
/// one return only that org's rows. Also the org GraphQL `createUser` writes
/// into (it has no request principal).
pub const ORG_ACME: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_ac3e);
pub const ORG_GLOBEX: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_61b3);

/// Maps a principal to its [`Ability`] — the app's CASL-style rule set.
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

/// Builds the request-scoped [`Ability`] from the [`AuthUser`] an upstream
/// [`AuthGuard`](crate::auth::AuthGuard) attached, and stores it as
/// `Arc<Ability>` for the `Authorize` extractor and handlers to read.
#[injectable]
pub struct AbilityGuard {
    #[inject]
    factory: Arc<AppAbility>,
}

#[async_trait]
impl Guard for AbilityGuard {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        let Some(user) = req.extensions().get::<AuthUser>().cloned() else {
            return Err(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("AbilityGuard requires AuthGuard to run first"));
        };
        let mut builder = AbilityBuilder::new();
        self.factory.define(&user, &mut builder);
        req.extensions_mut().insert(Arc::new(builder.build()));
        Ok(())
    }
}
