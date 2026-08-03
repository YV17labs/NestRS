//! Fluent builder for `AbilityFactory`:
//! `ab.can(Action::Read, users::Entity).when(|p| …).fields([…])`.
//!
//! A [`RuleSpec`] commits on drop — the rule is finalized by ending the
//! statement, with no terminal call to forget.

use std::any::TypeId;
use std::sync::Arc;

use sea_orm::{EntityTrait, IdenStatic};

use crate::ability::{Ability, FieldSet, MalformedRuleError, Rule};
use crate::action::Action;
use crate::predicate::{Predicate, PredicateBuilder};

/// Accumulates rules into an [`Ability`]. Handed to
/// [`AbilityFactory::define`](crate::AbilityFactory::define); each
/// [`can`](Self::can) / [`cannot`](Self::cannot) opens a [`RuleSpec`] that
/// commits on drop.
#[derive(Default)]
pub struct AbilityBuilder {
    ability: Ability,
    /// Rules whose relation predicate was rejected (the [`Predicate::Deny`]
    /// sentinel). Collected as rules commit so [`build`](Self::build) can fail
    /// the construction instead of letting a malformed denial silently go
    /// fail-open. Empty in the overwhelmingly common valid case.
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
    /// which [`requires_scope`](RuleSpec::requires_scope) rules materialize.
    ///
    /// `None` — the default — means the credential is not scope-aware (a
    /// session, an mTLS identity, a test actor), and every scoped rule applies
    /// in full. `Some` means it is, and a rule requiring a scope the list does
    /// not carry is **withheld**: not added, and remembered so the refusal can
    /// name it.
    ///
    /// The ability guard calls this with the request's
    /// `nest_rs_guards::GrantedScopes::shared()`; an app writing an
    /// `AbilityFactory` never does. Shared rather than owned because the guard
    /// runs on every authenticated request and the list is already allocated —
    /// and because this crate's engine must not depend on the transport
    /// bindings' `nest-rs-guards`.
    pub fn with_granted_scopes(mut self, scopes: Option<Arc<[String]>>) -> Self {
        self.granted_scopes = scopes;
        self
    }

    /// The scopes `required` names that this actor's credential does not carry.
    /// Empty when nothing is required, or when the credential is not
    /// scope-aware.
    ///
    /// The comparison is an exact string match, which is what RFC 6749 §3.3
    /// defines: scopes are opaque tokens, so `posts:*` is a value a deployment
    /// may mint but never a pattern this framework expands.
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

    /// Begin a grant. Narrow with [`when`](RuleSpec::when) / [`fields`](RuleSpec::fields).
    pub fn can<E>(&mut self, action: Action, _subject: E) -> RuleSpec<'_, E>
    where
        E: EntityTrait,
        E::Column: Send + Sync + 'static,
    {
        RuleSpec::new(self, action, false)
    }

    /// Begin a denial — a matching denial overrides a matching grant.
    pub fn cannot<E>(&mut self, action: Action, _subject: E) -> RuleSpec<'_, E>
    where
        E: EntityTrait,
        E::Column: Send + Sync + 'static,
    {
        RuleSpec::new(self, action, true)
    }

    /// Finalize the rule set. Fails with [`MalformedRuleError`] if any rule's
    /// relation predicate was rejected as malformed — a misconfiguration that,
    /// on the denial side, would otherwise combine fail-*open*. A valid rule set
    /// (the common case) always succeeds.
    pub fn build(mut self) -> Result<Ability, MalformedRuleError> {
        match self.malformed.drain(..).next() {
            Some(err) => Err(err),
            None => Ok(self.ability),
        }
    }

    /// [`build`](Self::build) for the **anonymous** caller: the rule set is
    /// identical, but the result answers `true` to
    /// [`Ability::is_visitor`](crate::Ability::is_visitor). The ability guard
    /// calls this on its
    /// [`define_visitor`](crate::AbilityFactory::define_visitor) branch, which
    /// is what lets a transport that admits anonymous callers at the edge
    /// (GraphQL) still refuse an operation whose declared posture is
    /// `#[authorize(...)]` rather than `#[public]`.
    pub fn build_visitor(self) -> Result<Ability, MalformedRuleError> {
        self.build().map(|mut ability| {
            ability.mark_visitor();
            ability
        })
    }
}

/// One in-progress rule. Commits on drop — binding to a variable defers the
/// commit, and the builder cannot be reused while a spec is still alive.
pub struct RuleSpec<'a, E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    builder: &'a mut AbilityBuilder,
    action: Action,
    inverted: bool,
    predicate: Predicate<E>,
    fields: FieldSet,
    required_scopes: Vec<String>,
}

impl<'a, E> RuleSpec<'a, E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    fn new(builder: &'a mut AbilityBuilder, action: Action, inverted: bool) -> Self {
        Self {
            builder,
            action,
            inverted,
            predicate: Predicate::Always,
            fields: FieldSet::All,
            required_scopes: Vec::new(),
        }
    }

    /// `.when(|p| p.eq(users::Column::OrgId, actor.org_id))`.
    pub fn when(mut self, build: impl FnOnce(PredicateBuilder<E>) -> Predicate<E>) -> Self {
        self.predicate = build(PredicateBuilder::new());
        self
    }

    /// Restrict the rule to these columns — the response masker keeps only
    /// these fields. Without this, every field is permitted.
    pub fn fields(mut self, columns: impl IntoIterator<Item = E::Column>) -> Self {
        self.fields = FieldSet::Only(columns.into_iter().map(|c| c.as_str()).collect());
        self
    }

    /// Gate this rule behind an OAuth scope: it materializes only when the
    /// caller's credential carries `scope`.
    ///
    /// ```rust,ignore
    /// ab.can(Action::Read, post::Entity)
    ///     .when(|p| p.eq(post::Column::OrgId, actor.org_id))
    ///     .requires_scope("posts:read");
    /// ```
    ///
    /// One declaration, three effects, and **no second decision site**: the
    /// rule is withheld when the scope is absent (so the gate, the query filter
    /// and the mask all refuse together, as they already do for a rule that was
    /// never written), the refusal remembers `scope` so the transport can
    /// answer `insufficient_scope` naming it, and the scope stays readable
    /// beside the permission it conditions rather than in a parallel table.
    /// The decision is still the guard's — this only says what the credential
    /// must carry for the rule to exist.
    ///
    /// Call it more than once to require **all** of the named scopes; a caller
    /// missing any one of them loses the rule, and the refusal names the ones
    /// they lack. Scopes are opaque tokens compared exactly (RFC 6749 §3.3),
    /// never patterns.
    ///
    /// A credential that is not scope-aware — no
    /// [`PrincipalIdentity::scopes`](https://docs.rs/nest-rs-authn) — is not
    /// gated by this at all, so adding it to a rule never breaks an app that
    /// authenticates by session.
    pub fn requires_scope(mut self, scope: impl Into<String>) -> Self {
        self.required_scopes.push(scope.into());
        self
    }
}

impl<'a, E> Drop for RuleSpec<'a, E>
where
    E: EntityTrait,
    E::Column: Send + Sync + 'static,
{
    fn drop(&mut self) {
        // A `Deny` predicate only ever comes from a rejected `related(...)`; a
        // denial that carries it is the fail-open case, a grant the silent one.
        // Record it so `build` fails naming the rule, before it can be consumed.
        //
        // Checked before the scope gate below, and deliberately: a malformed
        // rule is a developer error, and whether it surfaces must not depend on
        // which token the caller happened to present.
        if matches!(self.predicate, Predicate::Deny) {
            self.builder.malformed.push(MalformedRuleError {
                action: self.action,
                subject: std::any::type_name::<E>(),
                kind: if self.inverted { "denial" } else { "grant" },
            });
        }

        let missing = self.builder.missing_from_grant(&self.required_scopes);
        if !missing.is_empty() {
            // A grant the credential cannot reach is remembered so the refusal
            // can name the scope, then dropped. A *denial* it cannot reach is
            // dropped silently: `cannot(...)` narrows, so withholding one would
            // let a narrower token see more than a wider one — and there is
            // nothing for the client to go ask for either way.
            if !self.inverted {
                tracing::debug!(
                    target: "nest_rs::authz",
                    action = ?self.action,
                    subject = std::any::type_name::<E>(),
                    scopes = ?missing,
                    reason = "missing_scope",
                    "rule withheld",
                );
                self.builder
                    .ability
                    .withhold(self.action, TypeId::of::<E>(), missing);
            }
            return;
        }

        let condition = self.predicate.to_condition();
        let predicate = std::mem::take(&mut self.predicate);
        let fields = std::mem::take(&mut self.fields);
        self.builder.ability.add_rule(
            self.action,
            TypeId::of::<E>(),
            Rule {
                inverted: self.inverted,
                condition,
                predicate: Box::new(predicate),
                fields,
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
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

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
        // RFC 6749 §3.3 — `widgets:*` is a value a deployment may mint, and it
        // matches the rule requiring exactly it and nothing else. Expanding it
        // would silently widen every credential that carries one.
        let ability = ability_with(Some(scopes(["widgets:*"])));
        assert!(!ability.can_class(Action::Read, subject()));
        assert_eq!(
            ability.missing_scopes(Action::Read, subject()),
            ["widgets:read"]
        );
    }

    #[test]
    fn a_credential_that_is_not_scope_aware_is_never_gated() {
        // The session/mTLS/test case, and the reason adding `requires_scope` to
        // a rule cannot break an app that does not use OAuth at all.
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
        // The case `missing_scopes` must never be read as a check of its own:
        // the narrow token still reaches the subject by the unscoped rule, so
        // the gate allows and there is nothing to ask for.
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Read, widget::Entity);
        ab.can(Action::Read, widget::Entity)
            .requires_scope("widgets:admin");
        let ability = ab.build().expect("a valid rule set");

        assert!(ability.can_class(Action::Read, subject()));
    }

    #[test]
    fn a_scope_withheld_on_the_manage_wildcard_answers_a_read_refusal() {
        // `rules_for` widens `Read` to `Manage` on the grant side; the reason
        // for a refusal has to widen the same way or the client is told
        // nothing.
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

    #[test]
    fn a_withheld_denial_is_dropped_without_being_advertised() {
        // A `cannot` narrows. Withholding one would let a *narrower* token see
        // more than a wider one, and there is nothing for the client to request
        // either way — so it is dropped silently, never reported as missing.
        let mut ab = AbilityBuilder::new().with_granted_scopes(Some(scopes([])));
        ab.can(Action::Read, widget::Entity);
        ab.cannot(Action::Read, widget::Entity)
            .requires_scope("widgets:admin");
        let ability = ab.build().expect("a valid rule set");

        assert!(ability.can_class(Action::Read, subject()));
        assert!(
            ability.missing_scopes(Action::Read, subject()).is_empty(),
            "a withheld denial is not something a client can go fix",
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
