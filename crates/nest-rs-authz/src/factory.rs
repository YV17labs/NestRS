//! Turn an authenticated actor into an [`Ability`](crate::Ability) — the
//! per-actor capability set that the authorization layers consume.

use crate::builder::AbilityBuilder;

/// Implemented once per app for its actor type. All three authorization layers
/// (gate, query filter, response mask) consume the result.
///
/// ```
/// # use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action};
/// # mod users {
/// #     use sea_orm::entity::prelude::*;
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
/// #     #[sea_orm(table_name = "users")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         pub id: i32,
/// #         pub org_id: i32,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # #[derive(Clone)]
/// # struct AuthUser {
/// #     org_id: i32,
/// # }
/// # struct AuthzAbility;
/// impl AbilityFactory for AuthzAbility {
///     type Actor = AuthUser;
///     fn define(&self, actor: &AuthUser, ab: &mut AbilityBuilder) {
///         ab.can(Action::Read, users::Entity)
///             .when(|p| p.eq(users::Column::OrgId, actor.org_id));
///     }
/// }
/// # let mut ab = AbilityBuilder::new();
/// # AuthzAbility.define(&AuthUser { org_id: 7 }, &mut ab);
/// # let ability = ab.build()?;
///
/// assert!(ability.can::<users::Entity>(Action::Read, &users::Model { id: 1, org_id: 7 }));
/// assert!(!ability.can::<users::Entity>(Action::Read, &users::Model { id: 2, org_id: 8 }));
/// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
/// ```
pub trait AbilityFactory: Send + Sync + 'static {
    /// The app's authenticated-actor type — the principal whose claims the
    /// rules are written against.
    type Actor: Clone + Send + Sync + 'static;

    /// Populate `ability` with this actor's rules, called once per request the
    /// actor makes.
    fn define(&self, actor: &Self::Actor, ability: &mut AbilityBuilder);

    /// The unauthenticated visitor's rules, consulted on a `#[public]` route
    /// only. The default grants nothing.
    ///
    /// ```
    /// # use nest_rs_authz::{AbilityBuilder, AbilityFactory, Action};
    /// # mod post {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, serde::Serialize)]
    /// #     #[sea_orm(rs_type = "String", db_type = "Text")]
    /// #     pub enum PostStatus {
    /// #         #[sea_orm(string_value = "draft")]
    /// #         Draft,
    /// #         #[sea_orm(string_value = "published")]
    /// #         Published,
    /// #     }
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    /// #     #[sea_orm(table_name = "posts")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key)]
    /// #         pub id: i32,
    /// #         pub status: PostStatus,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// # use post::PostStatus;
    /// # struct AuthzAbility;
    /// # impl AbilityFactory for AuthzAbility {
    /// #     type Actor = ();
    /// #     fn define(&self, _actor: &(), _ab: &mut AbilityBuilder) {}
    /// fn define_visitor(&self, ab: &mut AbilityBuilder) {
    ///     ab.can(Action::Read, post::Entity)
    ///         .when(|p| p.eq(post::Column::Status, PostStatus::Published));
    /// }
    /// # }
    /// # let mut ab = AbilityBuilder::new();
    /// # AuthzAbility.define_visitor(&mut ab);
    /// # let visitor = ab.build_visitor()?;
    ///
    /// let published = post::Model { id: 1, status: PostStatus::Published };
    /// let draft = post::Model { id: 2, status: PostStatus::Draft };
    /// assert!(visitor.can::<post::Entity>(Action::Read, &published));
    /// assert!(!visitor.can::<post::Entity>(Action::Read, &draft));
    /// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
    /// ```
    ///
    /// A `#[public]` route reached *with* a valid token takes
    /// [`define`](Self::define) instead — the visitor branch is the anonymous
    /// case, not a floor added to every caller.
    fn define_visitor(&self, _ability: &mut AbilityBuilder) {}
}
