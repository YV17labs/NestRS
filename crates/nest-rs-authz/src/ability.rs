//! The compiled rule set for one actor, and the four reads the three
//! authorization layers (gate, query filter, response mask) perform against it.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, PoisonError};

use sea_orm::EntityTrait;
use sea_orm::sea_query::{Condition, Expr};

use crate::action::Action;
use crate::predicate::{Predicate, Truth, nothing_invented};

/// The one fail-closed masking warn, for every edge and the ambient
/// [`Ability::mask`]; here, always compiled, so a feature-less build reaches it.
pub(crate) fn warn_mask_failure(
    entity: &'static str,
    action: Action,
    reason: &'static str,
    detail: &'static str,
    // The edge's own context, on this one event; `tracing` drops a `None`.
    transport: Option<&'static str>,
    event: Option<&str>,
    // Rendered by `error_message`, which never quotes the value serde refused.
    err: Option<&(dyn std::error::Error + 'static)>,
) {
    // Two arms: `tracing` fixes an event's fields at the macro.
    match err {
        Some(err) => tracing::warn!(
            target: crate::TARGET,
            entity,
            action = ?action,
            reason,
            detail,
            transport,
            event,
            error = %nest_rs_core::error_message(err),
            "response masking failed",
        ),
        None => tracing::warn!(
            target: crate::TARGET,
            entity,
            action = ?action,
            reason,
            detail,
            transport,
            event,
            "response masking failed",
        ),
    }
}

/// The `(entity, action)` pairs whose pushed rows an undecidable rule already
/// refused, so a stream refusing every item files one line, not one per item.
static UNDECIDED_ON_PUSH: LazyLock<Mutex<HashSet<(TypeId, Action)>>> =
    LazyLock::new(Mutex::default);

/// The first time an undecidable rule refuses a pushed row of `E` for `action`
/// in this process, say so: the subscriber sees nothing, and nothing else would.
fn warn_undecidable_on_push<E: EntityTrait>(action: Action) {
    let first = UNDECIDED_ON_PUSH
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert((TypeId::of::<E>(), action));
    if first {
        tracing::warn!(
            target: crate::TARGET,
            entity = std::any::type_name::<E>(),
            action = ?action,
            reason = "undecidable_on_push",
            "an ability rule cannot be decided on a pushed value",
        );
    }
}

/// Where the rows under a check came from, which decides what a rule the
/// in-memory check cannot decide means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Provenance {
    /// A decorated reply: the unit read these rows through `Repo` under this
    /// ability, for this action, whose SQL applied every rule. Visibility
    /// trusts SQL; fields come from decided grants only.
    Loaded,
    /// A model the caller holds and checks itself ([`Ability::can`]), read for
    /// this action or another, so no read is known to have applied this
    /// action's rules: an undecidable grant grants nothing, an undecidable
    /// denial denies, and no warn is filed — the caller hears `false`.
    Held,
    /// A subscription item or a `masked_reply` value, which no read filtered:
    /// an undecidable grant grants nothing, an undecidable denial denies.
    Pushed,
}

/// Why a mask fell closed, as a value an incident query groups on.
pub(crate) mod mask_reason {
    /// Nothing installed an ability, so nothing decided what may be shown.
    ///
    /// Declared here and aliased by [`gate::reason`](crate::gate::reason):
    /// `gate` is feature-gated, this module is not.
    pub(crate) const NO_AMBIENT_ABILITY: &str = "no_ambient_ability";
    /// The value could not be turned into, or read back from, JSON.
    pub(crate) const NOT_SERIALIZABLE: &str = "not_serializable";
    /// The wire value and the entity model could not be reconciled, so the
    /// mask had no model to apply.
    #[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
    pub(crate) const IRRECONCILABLE: &str = "irreconcilable_wire_value";
    /// The response carried no content type, so it could not be classified as
    /// maskable at all. HTTP's alone — it is the one edge with a content type
    /// to be missing.
    #[cfg(feature = "http")]
    pub(crate) const UNCLASSIFIED_BODY: &str = "unclassified_body";
    // `field_not_granted` is a gate verdict, owned by `gate::reason::FIELD_NOT_GRANTED`.
}

/// Which fields of a subject may be read back in the response.
#[derive(Default)]
pub enum FieldSet {
    /// No restriction — every field is permitted.
    #[default]
    All,
    /// Only these columns (named as they serialize) are permitted.
    Only(HashSet<&'static str>),
}

impl FieldSet {
    /// Keep every field either set permits.
    fn widen(&mut self, other: &FieldSet) {
        match (&mut *self, other) {
            (FieldSet::All, _) => {}
            (_, FieldSet::All) => *self = FieldSet::All,
            (FieldSet::Only(mine), FieldSet::Only(theirs)) => mine.extend(theirs.iter().copied()),
        }
    }

    /// Keep only the fields both sets permit.
    fn narrow(&mut self, other: &FieldSet) {
        match (&mut *self, other) {
            (_, FieldSet::All) => {}
            (FieldSet::All, FieldSet::Only(theirs)) => *self = FieldSet::Only(theirs.clone()),
            (FieldSet::Only(mine), FieldSet::Only(theirs)) => mine.retain(|f| theirs.contains(f)),
        }
    }
}

/// One grant or denial. The condition is precomputed at build time (the actor's
/// values are known then); the typed [`Predicate`] is kept type-erased for the
/// in-memory check, downcast at the call site where the subject type is known.
pub(crate) struct Rule {
    pub(crate) inverted: bool,
    pub(crate) condition: Condition,
    pub(crate) predicate: Box<dyn Any + Send + Sync>,
    pub(crate) fields: FieldSet,
}

/// The authorization rules compiled for a single actor. Built by an
/// [`AbilityFactory`](crate::AbilityFactory) and consumed by the access guard
/// ([`can_class`](Ability::can_class)), the query pre-filter
/// ([`condition_for`](Ability::condition_for)), and the response check/mask
/// ([`can`](Ability::can) / [`permitted_fields`](Ability::permitted_fields)).
#[derive(Default)]
pub struct Ability {
    rules: HashMap<(Action, TypeId), Vec<Rule>>,
    /// What each grant this actor's credential could not reach lacks, in
    /// declaration order — recorded when [`RuleSpec::requires_scope`] withholds
    /// one, so a refusal can name what to ask for instead of being an opaque
    /// `403`.
    ///
    /// [`RuleSpec::requires_scope`]: crate::RuleSpec::requires_scope
    withheld: HashMap<(Action, TypeId), Vec<Vec<String>>>,
    visitor: bool,
}

impl Ability {
    pub(crate) fn add_rule(&mut self, action: Action, subject: TypeId, rule: Rule) {
        self.rules.entry((action, subject)).or_default().push(rule);
    }

    /// Record that a rule was withheld for lack of `scopes`. The rule itself is
    /// never added — a withheld grant must not widen anything.
    pub(crate) fn withhold(&mut self, action: Action, subject: TypeId, scopes: Vec<String>) {
        self.withheld
            .entry((action, subject))
            .or_default()
            .push(scopes);
    }

    /// The scopes one grant of `action` on `subject` lacks: every one of them
    /// is needed, and together with what the credential holds they suffice —
    /// RFC 6750 §3's `scope`, what a client asks for next.
    ///
    /// When several grants would have allowed it, the set is the narrowest's:
    /// the action's own grant before the [`Action::Manage`] wildcard's, the
    /// first declared among them. Never a union, which would make a client ask
    /// for more than it needs.
    ///
    /// Read it only after [`can_class`](Self::can_class) said no: a withheld rule
    /// and a granted one can coexist. Empty means the refusal was not about scope.
    pub fn missing_scopes(&self, action: Action, subject: TypeId) -> Vec<String> {
        keys_for(action, subject)
            .filter_map(|key| self.withheld.get(&key))
            .flatten()
            .next()
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn mark_visitor(&mut self) {
        self.visitor = true;
    }

    /// Whether these rules came from
    /// [`AbilityFactory::define_visitor`](crate::AbilityFactory::define_visitor)
    /// — i.e. the caller is **anonymous**.
    ///
    /// The enforcement layers ignore it; a transport admitting anonymous callers
    /// reads it so a `define_visitor` grant cannot satisfy an `#[authorize]`
    /// operation. Read it from a guard or a posture attribute's gate only.
    pub fn is_visitor(&self) -> bool {
        self.visitor
    }

    /// Rules relevant to `action` on `subject`: those keyed under the action
    /// itself plus those under [`Action::Manage`] (the action wildcard).
    fn rules_for(&self, action: Action, subject: TypeId) -> impl Iterator<Item = &Rule> {
        keys_for(action, subject)
            .filter_map(|key| self.rules.get(&key))
            .flatten()
    }

    /// Layer ① — the coarse, class-level gate the access guard/extractor uses:
    /// is there *any* grant for this action on this subject? Optimistic —
    /// instance conditions are enforced by layers ② and ③, not here.
    pub fn can_class(&self, action: Action, subject: TypeId) -> bool {
        self.rules_for(action, subject).any(|rule| !rule.inverted)
    }

    /// Layer ② — the query pre-filter: `(OR of grant conditions) AND NOT (OR of
    /// denial conditions)`. With no grant the result matches nothing (`1 = 0`).
    pub fn condition_for<E: EntityTrait>(&self, action: Action) -> Condition {
        let mut grant = Condition::any();
        let mut deny = Condition::any();
        for rule in self.rules_for(action, TypeId::of::<E>()) {
            if rule.inverted {
                deny = deny.add(rule.condition.clone());
            } else {
                grant = grant.add(rule.condition.clone());
            }
        }
        if grant.is_empty() {
            return Condition::all().add(Expr::cust("1 = 0"));
        }
        let mut out = Condition::all().add(grant);
        if !deny.is_empty() {
            out = out.add(deny.not());
        }
        out
    }

    /// Layer ③ — instance check: at least one grant matches this model and no
    /// denial does (a denial overrides), decided on the model alone, with no
    /// query.
    ///
    /// The model cannot show a relation, nor compare two value types only the
    /// database compares: a grant resting on one grants nothing here, and a
    /// denial resting on one denies. Where such a rule decides, ask SQL with
    /// `nest_rs::seaorm::Repo::can`, one indexed query under the action's own
    /// condition.
    pub fn can<E: EntityTrait>(&self, action: Action, model: &E::Model) -> bool {
        self.evaluate::<E>(action, model, Provenance::Held, &nothing_invented)
            .allowed
    }

    /// The single rule scan behind [`can`](Self::can),
    /// [`permitted_fields`](Self::permitted_fields) and
    /// [`mask_many`](Self::mask_many): one pass yields both answers.
    ///
    /// A rule the row decides counts as SQL counts it, `UNKNOWN` included.
    /// `invented` names the columns the row was given rather than read, and
    /// `provenance` says what a rule left undecided means.
    pub(crate) fn evaluate<E: EntityTrait>(
        &self,
        action: Action,
        model: &E::Model,
        provenance: Provenance,
        invented: &dyn Fn(E::Column) -> bool,
    ) -> Verdict {
        let mut granted = false;
        let mut denied = false;
        // The union of the decided grants' field sets: denials decide
        // visibility, never which columns a visible row exposes.
        let mut fields = FieldSet::Only(HashSet::new());
        // The intersection of the undecided grants' field sets, `None` while
        // there is none.
        let mut undecided_grants: Option<FieldSet> = None;
        let mut undecided_denial = false;
        for rule in self.rules_for(action, TypeId::of::<E>()) {
            let Some(predicate) = predicate_of::<E>(rule) else {
                // Unreachable type mismatch (see `predicate_of`) — fail
                // closed: a broken denial denies, a broken grant never widens.
                denied |= rule.inverted;
                continue;
            };
            match (predicate.decide(model, invented), rule.inverted) {
                // `NOT UNKNOWN` is `UNKNOWN`, so SQL's filter drops a row its
                // denial cannot compare as surely as one it matches.
                (Truth::True | Truth::Unknown, true) => denied = true,
                // Only `Loaded` reads the undecided grants' fields, and `can`
                // (`Held`) reads no field at all: neither builds a set it drops.
                (Truth::True, false) => {
                    granted = true;
                    if provenance != Provenance::Held {
                        fields.widen(&rule.fields);
                    }
                }
                (Truth::Undecidable, true) => undecided_denial = true,
                (Truth::Undecidable, false) => {
                    let shared = undecided_grants.get_or_insert(FieldSet::All);
                    if provenance == Provenance::Loaded {
                        shared.narrow(&rule.fields);
                    }
                }
                (Truth::False, _) | (Truth::Unknown, false) => {}
            }
        }
        match provenance {
            // The row passed `(OR grants) AND NOT (OR denials)` in SQL, so when
            // no decided grant matched, an undecided one did, and no denial did.
            // Its fields are the narrowest set every undecided grant permits.
            Provenance::Loaded => match undecided_grants {
                Some(shared) if !granted => Verdict {
                    allowed: !denied,
                    fields: shared,
                },
                _ => Verdict {
                    allowed: granted && !denied,
                    fields,
                },
            },
            Provenance::Held | Provenance::Pushed => {
                // Refused by a rule the row cannot decide, and by nothing else:
                // a decided grant met an undecided denial, or no grant was
                // decided and an undecided one would have matched.
                if provenance == Provenance::Pushed
                    && !denied
                    && ((granted && undecided_denial) || (!granted && undecided_grants.is_some()))
                {
                    warn_undecidable_on_push::<E>(action);
                }
                Verdict {
                    allowed: granted && !denied && !undecided_denial,
                    fields,
                }
            }
        }
    }

    /// Layer ③ — serialize a model and strip the fields this ability does not
    /// permit for `action`.
    pub fn mask<E>(&self, action: Action, model: &E::Model) -> serde_json::Value
    where
        E: EntityTrait,
        E::Model: serde::Serialize,
    {
        self.mask_with::<E>(action, model, self.permitted_fields::<E>(action, model))
    }

    /// [`mask`](Self::mask) with the field verdict already known.
    pub(crate) fn mask_with<E>(
        &self,
        action: Action,
        model: &E::Model,
        fields: FieldSet,
    ) -> serde_json::Value
    where
        E: EntityTrait,
        E::Model: serde::Serialize,
    {
        let mut json = match serde_json::to_value(model) {
            Ok(json) => json,
            // Fail closed: an empty body, never the unmasked model.
            Err(err) => {
                warn_mask_failure(
                    std::any::type_name::<E>(),
                    action,
                    mask_reason::NOT_SERIALIZABLE,
                    "model did not serialize",
                    None,
                    None,
                    Some(&err),
                );
                return serde_json::Value::Null;
            }
        };
        if let FieldSet::Only(allowed) = fields
            && let serde_json::Value::Object(map) = &mut json
        {
            map.retain(|key, _| allowed.contains(key.as_str()));
        }
        json
    }

    /// Layer ③ over a collection: drop the instances the actor may not see and
    /// mask the fields of those it may ([`mask`](Ability::mask)).
    ///
    /// The rows are a reply to a read through `Repo` under this ability for
    /// this action, whose SQL decided what the rows cannot show: a relation
    /// counts toward a row's visibility as that read counted it, and widens no
    /// field set a decided grant narrows.
    pub fn mask_many<'m, E>(
        &self,
        action: Action,
        models: impl IntoIterator<Item = &'m E::Model>,
    ) -> Vec<serde_json::Value>
    where
        E: EntityTrait,
        E::Model: serde::Serialize + 'm,
    {
        models
            .into_iter()
            .filter_map(|model| {
                let verdict =
                    self.evaluate::<E>(action, model, Provenance::Loaded, &nothing_invented);
                verdict
                    .allowed
                    .then(|| self.mask_with::<E>(action, model, verdict.fields))
            })
            .collect()
    }

    /// Layer ③ — the union of permitted fields across the grants that match this
    /// model. An unrestricted matching grant permits every field.
    ///
    /// A grant the row alone cannot decide widens nothing; when only such grants
    /// admit the row, it keeps the fields every one of them permits.
    pub fn permitted_fields<E: EntityTrait>(&self, action: Action, model: &E::Model) -> FieldSet {
        self.evaluate::<E>(action, model, Provenance::Loaded, &nothing_invented)
            .fields
    }
}

/// The rule-map keys an operation reads: the action itself, plus
/// [`Action::Manage`] (the action wildcard) unless that *is* the action.
fn keys_for(action: Action, subject: TypeId) -> impl Iterator<Item = (Action, TypeId)> {
    let wildcard = (action != Action::Manage).then_some((Action::Manage, subject));
    std::iter::once((action, subject)).chain(wildcard)
}

/// What one rule scan concluded about a model: whether it is visible, and which
/// of its columns the matching grants expose.
pub(crate) struct Verdict {
    pub(crate) allowed: bool,
    pub(crate) fields: FieldSet,
}

/// Recover a rule's typed predicate. A mismatch cannot happen (rules are keyed
/// by `TypeId::of::<E>()`), and fails closed at the call sites, never panics.
fn predicate_of<E: EntityTrait>(rule: &Rule) -> Option<&Predicate<E>> {
    let predicate = rule.predicate.downcast_ref::<Predicate<E>>();
    if predicate.is_none() {
        tracing::error!(
            target: crate::TARGET,
            reason = "predicate_type_mismatch",
            "ability rule predicate does not match its keyed subject — failing closed",
        );
    }
    predicate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predicate::PredicateBuilder;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "widgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod gadget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "gadgets")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod org {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "orgs")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    mod post {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
        #[sea_orm(table_name = "posts")]
        pub(super) struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub org_id: i32,
            pub title: String,
            /// The post this one replies to; `None` for a thread's root.
            pub parent_id: Option<i32>,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub(super) enum Relation {
            #[sea_orm(
                belongs_to = "super::org::Entity",
                from = "Column::OrgId",
                to = "super::org::Column::Id"
            )]
            Org,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

    fn a_post() -> post::Model {
        post::Model {
            id: 1,
            org_id: 7,
            title: "hello".into(),
            parent_id: None,
        }
    }

    fn in_org(id: i32) -> impl FnOnce(PredicateBuilder<post::Entity>) -> Predicate<post::Entity> {
        move |p| p.related::<org::Entity, _>(post::Relation::Org, |o| o.eq(org::Column::Id, id))
    }

    /// The columns a narrowed set keeps, sorted; `None` for every column.
    fn only(fields: FieldSet) -> Option<Vec<&'static str>> {
        match fields {
            FieldSet::All => None,
            FieldSet::Only(cols) => {
                let mut cols: Vec<_> = cols.into_iter().collect();
                cols.sort_unstable();
                Some(cols)
            }
        }
    }

    #[test]
    fn a_relational_grant_widens_no_field_set_a_decided_grant_narrows() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity)
            .fields([post::Column::Title]);
        b.can(Action::Read, post::Entity).when(in_org(7));
        let ability = b.build().unwrap();

        assert_eq!(
            only(ability.permitted_fields::<post::Entity>(Action::Read, &a_post())),
            Some(vec!["title"]),
        );
    }

    #[test]
    fn undecided_grants_alone_give_the_fields_they_all_share() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity)
            .when(in_org(7))
            .fields([post::Column::Id, post::Column::Title]);
        b.can(Action::Read, post::Entity)
            .when(in_org(8))
            .fields([post::Column::Title, post::Column::OrgId]);
        let ability = b.build().unwrap();

        assert_eq!(
            only(ability.permitted_fields::<post::Entity>(Action::Read, &a_post())),
            Some(vec!["title"]),
            "either grant may be the one SQL matched, so only their common field is certain",
        );
    }

    #[test]
    fn a_relational_grant_alone_admits_a_loaded_row_whole() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity).when(in_org(7));
        let ability = b.build().unwrap();

        assert_eq!(
            ability
                .mask_many::<post::Entity>(Action::Read, [a_post()].iter())
                .len(),
            1,
        );
        assert_eq!(
            only(ability.permitted_fields::<post::Entity>(Action::Read, &a_post())),
            None,
        );
    }

    #[test]
    fn a_relational_denial_refuses_no_row_its_read_admitted() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity);
        b.cannot(Action::Read, post::Entity).when(in_org(9));
        let ability = b.build().unwrap();

        assert_eq!(
            ability
                .mask_many::<post::Entity>(Action::Read, [a_post(), a_post()].iter())
                .len(),
            2,
        );
    }

    /// The model was read for some action, perhaps not this one, so no read is
    /// known to have applied this action's denial: one the row cannot decide
    /// denies.
    #[test]
    fn the_instance_check_counts_a_relational_denial_as_denying() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity);
        b.cannot(Action::Update, post::Entity).when(in_org(9));
        let ability = b.build().unwrap();

        assert!(
            !ability.can::<post::Entity>(Action::Update, &a_post()),
            "the post may belong to org 9 for all the row shows",
        );
        assert!(ability.can::<post::Entity>(Action::Read, &a_post()));
    }

    /// A row read for `Read` says nothing of `Update`'s relation, so the instance
    /// check grants nothing on it, nor on its negation.
    #[test]
    fn the_instance_check_grants_nothing_on_a_relation_it_cannot_see() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity);
        b.can(Action::Update, post::Entity).when(in_org(7));
        b.can(Action::Delete, post::Entity).when(|p| {
            p.not(p.related::<org::Entity, _>(post::Relation::Org, |o| o.eq(org::Column::Id, 9)))
        });
        let ability = b.build().unwrap();
        let read_for_read = a_post();

        assert!(ability.can::<post::Entity>(Action::Read, &read_for_read));
        assert!(
            !ability.can::<post::Entity>(Action::Update, &read_for_read),
            "the post may belong to any org for all the row shows",
        );
        assert!(!ability.can::<post::Entity>(Action::Delete, &read_for_read));
    }

    /// A relation beside a column the row shows settles nothing that column
    /// decides.
    #[test]
    fn the_instance_check_decides_what_the_row_shows_around_a_relation() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Update, post::Entity).when(|p| {
            p.any([
                p.eq(post::Column::OrgId, 7),
                p.related::<org::Entity, _>(post::Relation::Org, |o| o.eq(org::Column::Id, 8)),
            ])
        });
        let ability = b.build().unwrap();

        assert!(ability.can::<post::Entity>(Action::Update, &a_post()));
        assert!(!ability.can::<post::Entity>(
            Action::Update,
            &post::Model {
                org_id: 9,
                ..a_post()
            },
        ));
    }

    /// An `i32` column against an `i64` bound is the database's to compare: a
    /// reply trusts the read that compared it, the instance check grants
    /// nothing on it.
    #[test]
    fn a_grant_across_two_value_types_is_trusted_only_to_a_read() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7_i64));
        let ability = b.build().unwrap();

        assert!(!ability.can::<post::Entity>(Action::Read, &a_post()));
        assert_eq!(
            ability
                .mask_many::<post::Entity>(Action::Read, [a_post()].iter())
                .len(),
            1,
        );
    }

    /// SQL renders `eq(parent_id, None)` as `IS NULL`, so it admitted a root
    /// post through the narrow grant; deciding it `UNKNOWN` would leave the
    /// relational grant's wider field set as the only one in sight.
    #[test]
    fn a_null_bound_grant_keeps_its_field_set_against_a_relational_one() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::ParentId, None::<i32>))
            .fields([post::Column::Id, post::Column::Title]);
        b.can(Action::Read, post::Entity).when(in_org(8));
        let ability = b.build().unwrap();

        assert!(ability.can::<post::Entity>(Action::Read, &a_post()));
        assert_eq!(
            only(ability.permitted_fields::<post::Entity>(Action::Read, &a_post())),
            Some(vec!["id", "title"]),
        );
    }

    /// A push refused because no grant matched it says nothing about a rule
    /// the row could not decide, and spends no warn.
    #[test]
    fn a_push_no_grant_admits_files_no_undecidable_warn() {
        const UNDECIDABLE: &str = "an ability rule cannot be decided on a pushed value";
        let logs = nest_rs_testing::LogCapture::install();
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.cannot(Action::Read, post::Entity).when(in_org(9));
        let ability = b.build().unwrap();
        let push = |org_id| {
            let row = post::Model { org_id, ..a_post() };
            ability
                .evaluate::<post::Entity>(Action::Read, &row, Provenance::Pushed, &nothing_invented)
                .allowed
        };

        assert!(!push(8));
        logs.expect_none(crate::TARGET, UNDECIDABLE);

        assert!(
            !push(7),
            "org 7's grant meets a denial the row cannot decide"
        );
        let event = logs.expect_one(crate::TARGET, UNDECIDABLE);
        assert_eq!(
            event.field("reason").as_deref(),
            Some("undecidable_on_push")
        );
    }

    /// A rule keyed under one subject carrying another's — `AbilityBuilder` cannot build it.
    fn mismatched_rule() -> Rule {
        Rule {
            inverted: false,
            condition: Condition::all(),
            predicate: Box::new(Predicate::<gadget::Entity>::Always),
            fields: FieldSet::All,
        }
    }

    #[test]
    fn a_predicate_of_another_subject_yields_no_grant_and_is_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        let rule = mismatched_rule();

        assert!(
            predicate_of::<widget::Entity>(&rule).is_none(),
            "a predicate that is not this subject's grants nothing — reading it \
             as unrestricted is the one answer that opens rows",
        );

        let event = logs.expect_one(
            "nest_rs::authz",
            "ability rule predicate does not match its keyed subject — failing closed",
        );
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("reason").as_deref(),
            Some("predicate_type_mismatch"),
            "{:?}",
            event.fields,
        );
    }

    #[test]
    fn a_predicate_of_the_keyed_subject_is_recovered_in_silence() {
        let logs = nest_rs_testing::LogCapture::install();
        let rule = Rule {
            inverted: false,
            condition: Condition::all(),
            predicate: Box::new(Predicate::<widget::Entity>::Always),
            fields: FieldSet::All,
        };

        assert!(predicate_of::<widget::Entity>(&rule).is_some());
        logs.expect_none(
            "nest_rs::authz",
            "ability rule predicate does not match its keyed subject — failing closed",
        );
    }

    #[test]
    fn a_mask_failure_names_what_kind_of_value_it_found_never_the_value() {
        #[derive(Debug, serde::Deserialize)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Wire {
            age: u64,
        }
        let logs = nest_rs_testing::LogCapture::install();
        let refused =
            serde_json::from_value::<Wire>(serde_json::json!({ "age": "ada@example.com" }))
                .expect_err("a string is no age");
        warn_mask_failure(
            "Widget",
            Action::Read,
            mask_reason::IRRECONCILABLE,
            "wire value could not be reconciled with the entity model",
            None,
            None,
            Some(&refused),
        );
        let event = logs.expect_one(crate::TARGET, "response masking failed");
        assert_eq!(
            event.field("error").as_deref(),
            Some("invalid type: a string, expected u64")
        );
    }
}
