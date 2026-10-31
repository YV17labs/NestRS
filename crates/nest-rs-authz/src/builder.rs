//! Fluent builder for `AbilityFactory`:
//! `ab.can(Action::Read, users::Entity).when(|p| …).fields([…])`.
//!
//! A [`RuleSpec`] commits on drop — the rule is finalized by ending the
//! statement, with no terminal call to forget. Its kind, [`Can`] or [`Cannot`],
//! decides what it takes: a denial narrows rows alone, whatever the token.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::sync::Arc;

use sea_orm::sea_query::Condition;
use sea_orm::{EntityTrait, IdenStatic};

use crate::ability::{Ability, FieldSet, Rule, When};
use crate::action::Action;
use crate::error::MalformedRuleError;
use crate::predicate::{Predicate, PredicateBuilder};
use crate::{RowSubject, Subject};

/// Accumulates rules into an [`Ability`]. Handed to
/// [`AbilityFactory::define`](crate::AbilityFactory::define); each
/// [`can`](Self::can) / [`cannot`](Self::cannot) opens a [`RuleSpec`] that
/// commits on drop.
#[derive(Default)]
pub struct AbilityBuilder {
    ability: Ability,
    /// Rules whose relation predicate was rejected, so [`build`](Self::build)
    /// fails rather than a malformed denial going fail-open.
    malformed: Vec<MalformedRuleError>,
    /// What the actor's credential was granted, or `None` when the credential
    /// is not scope-aware. See [`with_granted_scopes`](Self::with_granted_scopes).
    granted_scopes: Option<Arc<[String]>>,
}

impl AbilityBuilder {
    /// An empty builder with no rules — grants nothing until `can` is called.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the OAuth scopes the actor's credential carries, which decides
    /// which [`requires_scope`](RuleSpec::requires_scope) grants materialize.
    ///
    /// `None` — the default — means the credential is not scope-aware (a
    /// session, an mTLS identity, a test actor), and every scoped grant applies
    /// in full. `Some` means it is, and a grant requiring a scope the list does
    /// not carry is **withheld**: not added, and remembered so the refusal can
    /// name it. A denial takes no scope, so it holds whatever the list carries.
    ///
    /// The ability guard calls this with the request's
    /// `nest_rs_guards::GrantedScopes::shared()`; an app writing an
    /// `AbilityFactory` never does.
    pub fn with_granted_scopes(mut self, scopes: Option<Arc<[String]>>) -> Self {
        self.granted_scopes = scopes;
        self
    }

    /// The scopes `required` names that this actor's credential does not carry,
    /// compared exactly (RFC 6749 §3.3: scopes are opaque tokens, never patterns).
    fn missing_from_grant(&self, required: &[String]) -> Vec<String> {
        let Some(granted) = &self.granted_scopes else {
            return Vec::new();
        };
        required
            .iter()
            .filter(|scope| !granted.contains(scope))
            .cloned()
            .collect()
    }

    /// Begin a grant on `subject`: an entity, narrowed with
    /// [`when`](RuleSpec::when) / [`fields`](RuleSpec::fields), or a subject
    /// `#[subject]` declares, which no table holds and the grant covers whole.
    pub fn can<S: Subject>(&mut self, action: Action, _subject: S) -> RuleSpec<'_, S> {
        RuleSpec::new(self, action, false)
    }

    /// Begin a denial — a matching denial overrides a matching grant, and one
    /// no [`when`](RuleSpec::when) narrows matches every row, so the class gate
    /// refuses the action outright.
    ///
    /// An entity's alone: a denial narrows the rows a grant reaches, and a
    /// subject no table holds has none — a grant not written is its denial.
    ///
    /// A denial holds whatever the token carries and hides rows, never
    /// columns: it takes neither `requires_scope` nor `fields`.
    ///
    /// ```compile_fail,E0599
    /// # use nest_rs_authz::{AbilityBuilder, Action};
    /// # mod post {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    /// #     #[sea_orm(table_name = "posts")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key)]
    /// #         pub id: i32,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// # let mut ab = AbilityBuilder::new();
    /// ab.cannot(Action::Delete, post::Entity).requires_scope("posts:admin");
    /// ```
    pub fn cannot<E: RowSubject>(
        &mut self,
        action: Action,
        _subject: E,
    ) -> RuleSpec<'_, E, Cannot> {
        RuleSpec::new(self, action, true)
    }

    /// Finalize the rule set. Fails with [`MalformedRuleError`] if any rule's
    /// relation predicate was rejected as malformed.
    pub fn build(mut self) -> Result<Ability, MalformedRuleError> {
        match self.malformed.drain(..).next() {
            Some(err) => Err(err),
            None => Ok(self.ability),
        }
    }

    /// [`build`](Self::build) for the **anonymous** caller: the rule set is
    /// identical, but the result answers `true` to
    /// [`Ability::is_visitor`](crate::Ability::is_visitor).
    pub fn build_visitor(self) -> Result<Ability, MalformedRuleError> {
        self.build().map(|mut ability| {
            ability.mark_visitor();
            ability
        })
    }
}

/// The kind of rule [`AbilityBuilder::can`] opens: a grant, which a scope may
/// withhold and a field list may narrow.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "uninhabited: a type-level marker no value ever takes"
)]
pub enum Can {}

/// The kind of rule [`AbilityBuilder::cannot`] opens: a denial, which narrows
/// rows alone and holds whatever the token carries.
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "uninhabited: a type-level marker no value ever takes"
)]
pub enum Cannot {}

/// One in-progress rule on subject `S`, a grant ([`Can`]) or a denial
/// ([`Cannot`]). Commits on drop — binding to a variable defers the commit,
/// and the builder cannot be reused while a spec is still alive.
pub struct RuleSpec<'a, S: Subject, K = Can> {
    builder: &'a mut AbilityBuilder,
    action: Action,
    inverted: bool,
    /// The predicate [`when`](Self::when) narrowed the rule's rows to, `None`
    /// while the rule holds for every row.
    rows: Option<Arc<dyn ErasedPredicate>>,
    /// The columns [`fields`](RuleSpec::fields) narrowed a grant to; named
    /// apart from that method, or a call on a denial would read as reaching a
    /// private field.
    columns: FieldSet,
    required_scopes: Vec<String>,
    kind: PhantomData<fn() -> (S, K)>,
}

/// A [`when`](RuleSpec::when) predicate with its entity erased, so a rule on
/// any [`Subject`] carries one; lowered to SQL only when its rule materializes.
trait ErasedPredicate: Any + Send + Sync {
    /// Whether it is the `Deny` a rejected relation leaves.
    fn is_deny(&self) -> bool;

    fn condition(&self) -> Condition;
}

impl<E> ErasedPredicate for Predicate<E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    fn is_deny(&self) -> bool {
        matches!(self, Predicate::Deny)
    }

    fn condition(&self) -> Condition {
        self.to_condition()
    }
}

impl<'a, S: Subject, K> RuleSpec<'a, S, K> {
    fn new(builder: &'a mut AbilityBuilder, action: Action, inverted: bool) -> Self {
        Self {
            builder,
            action,
            inverted,
            rows: None,
            columns: FieldSet::All,
            required_scopes: Vec::new(),
            kind: PhantomData,
        }
    }
}

impl<S: Subject> RuleSpec<'_, S> {
    /// Gate this grant behind an OAuth scope: it materializes only when the
    /// caller's credential carries `scope`.
    ///
    /// ```
    /// # use std::any::TypeId;
    /// # use std::sync::Arc;
    /// # use nest_rs_authz::{AbilityBuilder, Action};
    /// # mod post {
    /// #     use sea_orm::entity::prelude::*;
    /// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
    /// #     #[sea_orm(table_name = "posts")]
    /// #     pub struct Model {
    /// #         #[sea_orm(primary_key)]
    /// #         pub id: i32,
    /// #         pub org_id: i32,
    /// #     }
    /// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    /// #     pub enum Relation {}
    /// #     impl ActiveModelBehavior for ActiveModel {}
    /// # }
    /// # struct Actor {
    /// #     org_id: i32,
    /// # }
    /// # let actor = Actor { org_id: 7 };
    /// # let granted: Arc<[String]> = Arc::from(["profile".to_owned()]);
    /// # let mut ab = AbilityBuilder::new().with_granted_scopes(Some(granted));
    /// ab.can(Action::Read, post::Entity)
    ///     .when(|p| p.eq(post::Column::OrgId, actor.org_id))
    ///     .requires_scope("posts:read");
    /// # let ability = ab.build()?;
    ///
    /// let posts = TypeId::of::<post::Entity>();
    /// assert!(!ability.can_class(Action::Read, posts));
    /// assert_eq!(ability.missing_scopes(Action::Read, posts), ["posts:read"]);
    /// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
    /// ```
    ///
    /// Call it more than once to require **all** of the named scopes; a caller
    /// missing any one of them loses the grant, and the refusal names the ones
    /// they lack. Scopes are opaque tokens compared exactly (RFC 6749 §3.3),
    /// never patterns.
    ///
    /// A credential that is not scope-aware — no
    /// [`PrincipalIdentity::scopes`](https://docs.rs/nest-rs-authn) — is not
    /// gated by this at all.
    pub fn requires_scope(mut self, scope: impl Into<String>) -> Self {
        self.required_scopes.push(scope.into());
        self
    }
}

// Bounded per method, never per impl: a capability calling one is refused
// with `RowSubject`'s sentence, where an unmet impl bound reads as no method.
impl<S: Subject, K> RuleSpec<'_, S, K> {
    /// `.when(|p| p.eq(users::Column::OrgId, actor.org_id))`: the rule holds
    /// for the rows the predicate matches alone.
    pub fn when(mut self, build: impl FnOnce(PredicateBuilder<S>) -> Predicate<S>) -> Self
    where
        S: RowSubject,
    {
        self.rows = Some(Arc::new(build(PredicateBuilder::new())));
        self
    }
}

impl<S: Subject> RuleSpec<'_, S> {
    /// Restrict the grant to these columns — the response masker keeps only
    /// these fields. Without this, every field is permitted.
    pub fn fields<C: IdenStatic>(mut self, columns: impl IntoIterator<Item = C>) -> Self
    where
        S: RowSubject<__Column = C>,
    {
        self.columns = FieldSet::Only(columns.into_iter().map(|c| c.as_str()).collect());
        self
    }
}

impl<S: Subject, K> Drop for RuleSpec<'_, S, K> {
    fn drop(&mut self) {
        // A `Deny` predicate only comes from a rejected `related(...)`. Checked
        // before the scope gate, so a developer error surfaces whatever the token.
        if self.rows.as_deref().is_some_and(|rows| rows.is_deny()) {
            self.builder.malformed.push(MalformedRuleError {
                action: self.action,
                subject: std::any::type_name::<S>(),
                kind: if self.inverted { "denial" } else { "grant" },
            });
        }

        // A grant alone is withheld, and remembered so the refusal can name the
        // scope: a denial withheld for a narrower token would widen it.
        if !self.inverted {
            let missing = self.builder.missing_from_grant(&self.required_scopes);
            if !missing.is_empty() {
                tracing::debug!(
                    target: crate::TARGET,
                    action = ?self.action,
                    subject = std::any::type_name::<S>(),
                    scopes = ?missing,
                    reason = "missing_scope",
                    "rule withheld",
                );
                self.builder
                    .ability
                    .withhold(self.action, TypeId::of::<S>(), missing);
                return;
            }
        }

        let condition = match self.rows.as_deref() {
            Some(rows) => rows.condition(),
            None => Condition::all(),
        };
        self.builder.ability.add_rule(
            self.action,
            TypeId::of::<S>(),
            Rule {
                inverted: self.inverted,
                condition,
                when: self
                    .rows
                    .take()
                    .map(|rows| When::Predicate(rows as Arc<dyn Any + Send + Sync>)),
                fields: std::mem::take(&mut self.columns),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// The shared scope list the ability guard hands the builder.
    fn scopes<const N: usize>(granted: [&str; N]) -> Arc<[String]> {
        granted.iter().map(|s| (*s).to_owned()).collect()
    }

    fn subject() -> TypeId {
        TypeId::of::<widget::Entity>()
    }

    /// One scoped grant, built against the scopes a credential carries.
    fn ability_with(granted: Option<Arc<[String]>>) -> Ability {
        let mut ab = AbilityBuilder::new().with_granted_scopes(granted);
        ab.can(Action::Read, widget::Entity)
            .requires_scope("widgets:read");
        ab.build().expect("a valid rule set")
    }

    #[test]
    fn a_credential_carrying_the_scope_gets_the_rule() {
        let ability = ability_with(Some(scopes(["widgets:read"])));
        assert!(ability.can_class(Action::Read, subject()));
        assert!(
            ability.missing_scopes(Action::Read, subject()).is_empty(),
            "nothing was withheld, so there is nothing to ask for",
        );
    }

    #[test]
    fn a_narrow_credential_loses_the_rule_and_learns_what_to_ask_for() {
        let ability = ability_with(Some(scopes(["ledgers:export"])));
        assert!(
            !ability.can_class(Action::Read, subject()),
            "a withheld grant must not widen anything",
        );
        assert_eq!(
            ability.missing_scopes(Action::Read, subject()),
            ["widgets:read"],
        );
    }

    #[test]
    fn a_scope_is_an_opaque_token_never_a_wildcard() {
        // RFC 6749 §3.3: `widgets:*` is a value, never a pattern.
        let ability = ability_with(Some(scopes(["widgets:*"])));
        assert!(!ability.can_class(Action::Read, subject()));
        assert_eq!(
            ability.missing_scopes(Action::Read, subject()),
            ["widgets:read"]
        );
    }

    #[test]
    fn a_credential_that_is_not_scope_aware_is_never_gated() {
        let ability = ability_with(None);
        assert!(ability.can_class(Action::Read, subject()));
        assert!(ability.missing_scopes(Action::Read, subject()).is_empty());
    }

    #[test]
    fn an_empty_grant_is_not_the_same_as_no_grant_information() {
        // `Some([])` is an OAuth credential that was granted nothing.
        let ability = ability_with(Some(scopes([])));
        assert!(!ability.can_class(Action::Read, subject()));
        assert_eq!(
            ability.missing_scopes(Action::Read, subject()),
            ["widgets:read"],
        );
    }

    #[test]
    fn every_named_scope_is_required_and_only_the_missing_ones_are_reported() {
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes(["widgets:read"])));
        ab.can(Action::Manage, widget::Entity)
            .requires_scope("widgets:read")
            .requires_scope("widgets:write");
        let ability = ab.build().expect("a valid rule set");

        assert!(
            !ability.can_class(Action::Manage, subject()),
            "requiring two scopes means both, not either",
        );
        assert_eq!(
            ability.missing_scopes(Action::Manage, subject()),
            ["widgets:write"],
            "the caller is only told about what they actually lack",
        );
    }

    #[test]
    fn a_withheld_rule_beside_a_granted_one_is_not_a_denial() {
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Read, widget::Entity);
        ab.can(Action::Read, widget::Entity)
            .requires_scope("widgets:admin");
        let ability = ab.build().expect("a valid rule set");

        assert!(ability.can_class(Action::Read, subject()));
    }

    #[test]
    fn a_scope_withheld_on_the_manage_wildcard_answers_a_read_refusal() {
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Manage, widget::Entity)
            .requires_scope("widgets:write");
        let ability = ab.build().expect("a valid rule set");

        assert!(!ability.can_class(Action::Read, subject()));
        assert_eq!(
            ability.missing_scopes(Action::Read, subject()),
            ["widgets:write"],
        );
    }

    /// Two grants, either sufficient: the refusal names one set — every scope
    /// of it needed, and enough — the action's own grant before the wildcard's,
    /// so a client asking for it never asks for more than it needs.
    #[test]
    fn of_two_sufficient_grants_the_refusal_names_the_narrowest_alone() {
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Manage, widget::Entity)
            .requires_scope("widgets:write");
        ab.can(Action::Read, widget::Entity)
            .requires_scope("widgets:read");
        ab.can(Action::Read, widget::Entity)
            .requires_scope("widgets:audit");
        let ability = ab.build().expect("a valid rule set");

        assert!(!ability.can_class(Action::Read, subject()));
        assert_eq!(
            ability.missing_scopes(Action::Read, subject()),
            ["widgets:read"],
            "one grant's scopes, never the union of every grant's",
        );
        assert_eq!(
            ability.missing_scopes(Action::Manage, subject()),
            ["widgets:write"],
        );
    }

    #[test]
    fn a_denial_holds_for_a_token_carrying_no_scope() {
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Read, widget::Entity);
        ab.cannot(Action::Read, widget::Entity);
        let ability = ab.build().expect("a valid rule set");

        assert!(
            !ability.can_class(Action::Read, subject()),
            "a token asking fewer scopes never escapes a denial",
        );
        assert!(
            ability.missing_scopes(Action::Read, subject()).is_empty(),
            "no scope lifts a denial, so none is advertised",
        );
    }

    #[test]
    fn an_unscoped_rule_set_is_untouched_by_any_of_this() {
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Read, widget::Entity);
        let ability = ab.build().expect("a valid rule set");

        assert!(ability.can_class(Action::Read, subject()));
        assert!(ability.missing_scopes(Action::Read, subject()).is_empty());
    }
}
