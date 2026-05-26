//! The app-implemented entry point: turn an authenticated actor into an
//! [`Ability`](crate::Ability).

use crate::builder::AbilityBuilder;

/// Builds an [`Ability`](crate::Ability) for a given actor. An app implements
/// this once for its actor type (the caller resolved from a JWT/session, a
/// tenant), declaring the rules; all three authorization layers consume the
/// result. The analog of NestJS CASL's `CaslAbilityFactory`.
///
/// ```ignore
/// impl AbilityFactory for AppAbility {
///     type Actor = AuthUser;
///     fn define(&self, actor: &AuthUser, ab: &mut AbilityBuilder) {
///         ab.can(Action::Read, users::Entity)
///             .when(|p| p.eq(users::Column::OrgId, actor.org_id));
///         if actor.is_admin() {
///             ab.can(Action::Manage, users::Entity)
///                 .when(|p| p.eq(users::Column::OrgId, actor.org_id));
///         }
///     }
/// }
/// ```
pub trait AbilityFactory: Send + Sync + 'static {
    /// The authenticated actor an ability is built for. The bounds let
    /// [`AbilityGuard`](crate::AbilityGuard) read it back out of the request
    /// extensions an authn guard stored it in.
    type Actor: Clone + Send + Sync + 'static;

    fn define(&self, actor: &Self::Actor, ability: &mut AbilityBuilder);
}
