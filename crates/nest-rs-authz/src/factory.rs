//! Turn an authenticated actor into an [`Ability`](crate::Ability) — the
//! per-actor capability set that the authorization layers consume.

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
    /// The app's authenticated-actor type — the principal whose claims the
    /// rules are written against.
    type Actor: Clone + Send + Sync + 'static;

    /// Populate `ability` with this actor's rules — the single place an app
    /// declares who may do what, called once per request the actor makes.
    fn define(&self, actor: &Self::Actor, ability: &mut AbilityBuilder);

    /// The unauthenticated visitor's rules, consulted on a `#[public]` route
    /// only — there is no actor to key them off, so this is where a genuinely
    /// public resource is declared. The default grants nothing: an app that
    /// never overrides it keeps every anonymous read fail-closed, and the
    /// rules stay as greppable as [`define`](Self::define)'s.
    ///
    /// ```ignore
    /// fn define_visitor(&self, ab: &mut AbilityBuilder) {
    ///     ab.can(Action::Read, post::Entity)
    ///         .when(|p| p.eq(post::Column::Status, PostStatus::Published));
    /// }
    /// ```
    ///
    /// A `#[public]` route reached *with* a valid token takes
    /// [`define`](Self::define) instead — the visitor branch is the anonymous
    /// case, not a floor added to every caller.
    fn define_visitor(&self, _ability: &mut AbilityBuilder) {}
}
