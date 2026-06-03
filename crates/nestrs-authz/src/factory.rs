//! Turn an authenticated actor into an [`Ability`](crate::Ability) — the
//! analog of NestJS CASL's `CaslAbilityFactory`.

use crate::builder::AbilityBuilder;

/// Implemented once per app for its actor type. All three authorization layers
/// (gate, query filter, response mask) consume the result.
///
/// ```ignore
/// impl AbilityFactory for AppAbility {
///     type Actor = AuthUser;
///     fn define(&self, actor: &AuthUser, ab: &mut AbilityBuilder) {
///         ab.can(Action::Read, users::Entity)
///             .when(|p| p.eq(users::Column::OrgId, actor.org_id));
///     }
/// }
/// ```
pub trait AbilityFactory: Send + Sync + 'static {
    type Actor: Clone + Send + Sync + 'static;

    fn define(&self, actor: &Self::Actor, ability: &mut AbilityBuilder);
}
