//! The compiled rule set for one actor, and the four reads the three
//! authorization layers (gate, query filter, response mask) perform against it.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use sea_orm::sea_query::{Condition, Expr};
use sea_orm::{EntityTrait, IdenStatic, Value};

use crate::action::Action;
use crate::predicate::{Predicate, Truth, nothing_invented};
use crate::wire_model::WireModel;

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
    /// A subscription item, a `masked_reply` value or a body's answer to a visitor,
    /// which no read filtered: an undecidable grant grants nothing, an undecidable
    /// denial denies.
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
#[non_exhaustive]
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

    /// The fields both sets permit.
    fn narrowed(mut self, other: &FieldSet) -> FieldSet {
        self.narrow(other);
        self
    }

    /// The same fields, in a set of its own.
    fn copy_of(set: &FieldSet) -> FieldSet {
        FieldSet::All.narrowed(set)
    }
}

/// One grant or denial. The condition is precomputed at build time (the actor's
/// values are known then); the typed [`Predicate`] is kept type-erased for the
/// in-memory check, downcast at the call site where the subject type is known,
/// and absent from a rule no `when` narrowed, which holds for every row.
pub(crate) struct Rule {
    pub(crate) inverted: bool,
    pub(crate) condition: Condition,
    pub(crate) when: Option<When>,
    pub(crate) fields: FieldSet,
}

impl Rule {
    /// The grant a row meets by meeting both: the two conditions conjoined, the
    /// two `when`s, the columns both permit.
    fn and(&self, other: &Rule) -> Rule {
        let when = match (&self.when, &other.when) {
            (None, None) => None,
            (Some(when), None) | (None, Some(when)) => Some(when.clone()),
            (Some(mine), Some(theirs)) => {
                Some(When::Both(Arc::new(mine.clone()), Arc::new(theirs.clone())))
            }
        };
        Rule {
            inverted: false,
            // Every part kept, an empty one included: sea-query renders an empty
            // `any` as FALSE and a negated empty `all` as NOT TRUE. Two rules
            // no `when` narrows conjoin into one, as the builder commits it.
            condition: match when {
                None => Condition::all(),
                Some(_) => Condition::all()
                    .add(self.condition.clone())
                    .add(other.condition.clone()),
            },
            when,
            fields: FieldSet::copy_of(&self.fields).narrowed(&other.fields),
        }
    }

    /// The same rule, kept by a second ability.
    fn duplicate(&self) -> Rule {
        Rule {
            inverted: self.inverted,
            condition: self.condition.clone(),
            when: self.when.clone(),
            fields: FieldSet::copy_of(&self.fields),
        }
    }
}

/// The rows a rule's `when` narrows it to, its entity erased: the predicate a
/// factory wrote, or two that [`Ability::intersection`] conjoined.
#[derive(Clone)]
pub(crate) enum When {
    /// A `Predicate` of the rule's keyed subject.
    Predicate(Arc<dyn Any + Send + Sync>),
    /// Two narrowings a row meets only by meeting both.
    Both(Arc<When>, Arc<When>),
}

impl When {
    /// What it is on `model`, as SQL would compute it, or `None` when a
    /// predicate is not the keyed subject's (see `predicate_of`).
    fn decide<E: EntityTrait>(
        &self,
        model: &E::Model,
        invented: &dyn Fn(E::Column) -> bool,
    ) -> Option<Truth> {
        match self {
            When::Predicate(predicate) => {
                predicate_of::<E>(&**predicate).map(|predicate| predicate.decide(model, invented))
            }
            When::Both(mine, theirs) => Some(Truth::all(
                [
                    mine.decide::<E>(model, invented)?,
                    theirs.decide::<E>(model, invented)?,
                ]
                .into_iter(),
            )),
        }
    }

    /// The columns its conjunction pins by equality, and those it constrains
    /// any other way; `false` when a predicate is not the keyed subject's.
    fn pins<E: EntityTrait>(
        &self,
        pinned: &mut Vec<(E::Column, Value)>,
        other: &mut Vec<E::Column>,
    ) -> bool {
        match self {
            When::Predicate(predicate) => predicate_of::<E>(&**predicate)
                .map(|predicate| predicate.pins(pinned, other))
                .is_some(),
            When::Both(mine, theirs) => {
                mine.pins::<E>(pinned, other) && theirs.pins::<E>(pinned, other)
            }
        }
    }
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

    /// Record that a grant was withheld for lack of `scopes`. The grant itself
    /// is never added — a withheld grant must not widen anything.
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
    /// At a site gated on [`Action::Manage`], which needs every action, the set
    /// is what lifts them all: what each refused action lacks, each scope once,
    /// or the wildcard's own grant, which opens the four, when it asks for less.
    ///
    /// Read it only after [`can_class`](Self::can_class) said no: a withheld rule
    /// and a granted one can coexist. Empty means the refusal was not about scope
    /// — a denial no `when` narrows among them, which no scope lifts, or an
    /// action no grant would open.
    pub fn missing_scopes(&self, action: Action, subject: TypeId) -> Vec<String> {
        if self.denied_outright(action, subject) {
            return Vec::new();
        }
        if action == Action::Manage {
            return self.missing_scopes_of_every_action(subject);
        }
        keys_for(action, subject)
            .filter_map(|key| self.withheld.get(&key))
            .flatten()
            .next()
            .cloned()
            .unwrap_or_default()
    }

    /// [`missing_scopes`](Self::missing_scopes) at a site gated on `Manage`, no
    /// denial barring it.
    fn missing_scopes_of_every_action(&self, subject: TypeId) -> Vec<String> {
        let mut each = Vec::new();
        for action in Action::CONCRETE {
            if self.can_class(action, subject) {
                continue;
            }
            let lacking = self.missing_scopes(action, subject);
            if lacking.is_empty() {
                return Vec::new();
            }
            for scope in lacking {
                if !each.contains(&scope) {
                    each.push(scope);
                }
            }
        }
        // The wildcard's own grant opens all four: it answers when it asks for less.
        match self
            .withheld
            .get(&(Action::Manage, subject))
            .and_then(|grants| grants.first())
        {
            Some(wildcard) if wildcard.len() < each.len() => wildcard.clone(),
            _ => each,
        }
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
    /// operation. Read it from a guard, a posture attribute's gate or a mask judging
    /// what no read of this caller's filtered, only.
    pub fn is_visitor(&self) -> bool {
        self.visitor
    }

    /// The rules both abilities allow: a subject granted where both grant it,
    /// a row where both conditions admit it, a column where both expose it;
    /// either's denial denies, and either's anonymity makes it anonymous.
    pub(crate) fn intersection(&self, other: &Ability) -> Ability {
        let mut both = Ability {
            visitor: self.visitor || other.visitor,
            ..Ability::default()
        };
        let keys: HashSet<(Action, TypeId)> = self
            .rules
            .keys()
            .chain(other.rules.keys())
            .copied()
            .collect();
        for key @ (action, subject) in keys {
            for side in [self, other] {
                for denial in side.rules.get(&key).into_iter().flatten() {
                    if denial.inverted {
                        both.add_rule(action, subject, denial.duplicate());
                    }
                }
            }
            // A query reads its action's rules and the wildcard's, so each
            // pair of grants that query would meet lands under one key.
            let wildcard = (Action::Manage, subject);
            let pairs: &[_] = if action == Action::Manage {
                &[(key, key)]
            } else {
                &[(key, key), (key, wildcard), (wildcard, key)]
            };
            for &(mine, theirs) in pairs {
                for grant in self.grants_under(mine) {
                    for their_grant in other.grants_under(theirs) {
                        both.add_rule(action, subject, grant.and(their_grant));
                    }
                }
            }
        }
        // A withheld grant is named only where the other side grants what it opens. The
        // wildcards go last, landing on an action's key: `missing_scopes` takes the first.
        for wildcards in [false, true] {
            for (side, beside) in [(self, other), (other, self)] {
                for (&(action, subject), scopes) in &side.withheld {
                    if (action == Action::Manage) != wildcards {
                        continue;
                    }
                    let opens: &[Action] = if wildcards && !beside.can_class(action, subject) {
                        &Action::CONCRETE
                    } else {
                        std::slice::from_ref(&action)
                    };
                    for &opened in opens {
                        if beside.can_class(opened, subject) {
                            for lacking in scopes {
                                both.withhold(opened, subject, lacking.clone());
                            }
                        }
                    }
                }
            }
        }
        both
    }

    /// The grants keyed under exactly `key`, the wildcard's not added.
    fn grants_under(&self, key: (Action, TypeId)) -> impl Iterator<Item = &Rule> {
        self.rules
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|rule| !rule.inverted)
    }

    /// Rules a concrete `action` reads on `subject`: those keyed under the action
    /// itself plus those under [`Action::Manage`] (the action wildcard).
    fn rules_for(&self, action: Action, subject: TypeId) -> impl Iterator<Item = &Rule> {
        keys_for(action, subject)
            .filter_map(|key| self.rules.get(&key))
            .flatten()
    }

    /// The concrete actions a site gated on `action` holds a caller to. All four
    /// for [`Action::Manage`], but where no rule names one of them: each then
    /// reads the wildcard's rules alike, and `Read` says it for the others.
    fn actions_at_site(&self, action: Action, subject: TypeId) -> &'static [Action] {
        match action {
            Action::Manage
                if !Action::CONCRETE
                    .iter()
                    .any(|&concrete| self.rules.contains_key(&(concrete, subject))) =>
            {
                &[Action::Read]
            }
            _ => action.concrete(),
        }
    }

    /// Layer ① — the coarse, class-level gate every edge's posture runs: is
    /// there any grant for this action on this subject, and no denial that
    /// holds for every row?
    ///
    /// A denial no `when` narrows matches every row, so it refuses here, before
    /// anything runs for the caller. A grant's or a denial's `when` is left to
    /// the rows: layers ② and ③ enforce it, not this gate.
    ///
    /// [`Action::Manage`] asks it of every concrete action: a denial of one
    /// refuses it, four separate grants pass it.
    pub fn can_class(&self, action: Action, subject: TypeId) -> bool {
        self.actions_at_site(action, subject).iter().all(|&action| {
            let mut granted = false;
            for rule in self.rules_for(action, subject) {
                if rule.inverted && rule.when.is_none() {
                    return false;
                }
                granted |= !rule.inverted;
            }
            granted
        })
    }

    /// Whether a denial no `when` narrows bars `action` on `subject`: it
    /// matches every row, so no grant and no scope lifts it.
    pub(crate) fn denied_outright(&self, action: Action, subject: TypeId) -> bool {
        self.actions_at_site(action, subject).iter().any(|&action| {
            self.rules_for(action, subject)
                .any(|rule| rule.inverted && rule.when.is_none())
        })
    }

    /// Whether some grant opens a concrete `action` on `subject` and none of them
    /// narrows its columns: every row a read admits under this ability ships whole.
    #[cfg(feature = "graphql")]
    pub(crate) fn grants_every_column(&self, action: Action, subject: TypeId) -> bool {
        let mut grants = self
            .rules_for(action, subject)
            .filter(|rule| !rule.inverted)
            .peekable();
        grants.peek().is_some() && grants.all(|rule| matches!(rule.fields, FieldSet::All))
    }

    /// Layer ② — the query pre-filter: `(OR of grant conditions) AND NOT (OR of
    /// denial conditions)`. With no grant the result matches nothing (`1 = 0`).
    ///
    /// [`Action::Manage`] conjoins every concrete action's, a row having to pass
    /// each; where only `Manage` rules exist, that is this one condition.
    pub fn condition_for<E: EntityTrait>(&self, action: Action) -> Condition {
        let subject = TypeId::of::<E>();
        match self.actions_at_site(action, subject) {
            [action] => self.condition_of(*action, subject),
            every => every.iter().fold(Condition::all(), |all, &action| {
                all.add(self.condition_of(action, subject))
            }),
        }
    }

    /// [`condition_for`](Self::condition_for) of one concrete action.
    fn condition_of(&self, action: Action, subject: TypeId) -> Condition {
        let mut grant = Condition::any();
        let mut deny = Condition::any();
        for rule in self.rules_for(action, subject) {
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

    /// The column values the grants of `action` on `E` pin by equality — CASL's
    /// `rulesToFields`: what a fresh row must hold to fall in scope, which
    /// `nest_rs::seaorm::Creatable` fills on a create where the input left a
    /// column unset.
    ///
    /// A column is kept only when every grant constraining it pins it with
    /// `eq` to one value; a grant leaving it free admits any. Two grants
    /// pinning it apart, or one constraining it any other way (`is_in`, a
    /// comparison, a null test, inside `any` or `not`), leave it out, and the
    /// scope check decides. A grant with no `when` and a denial pin nothing.
    /// [`Action::Manage`] reads every concrete action's grants, a row having to
    /// fall in each one's scope.
    pub fn pinned_values<E: EntityTrait>(&self, action: Action) -> Vec<(E::Column, Value)> {
        let mut agreed: Vec<(E::Column, Value)> = Vec::new();
        let mut contested: Vec<E::Column> = Vec::new();
        let mut pinned = Vec::new();
        let subject = TypeId::of::<E>();
        let rules = self
            .actions_at_site(action, subject)
            .iter()
            .flat_map(|&action| self.rules_for(action, subject));
        for rule in rules {
            if rule.inverted {
                continue;
            }
            let Some(when) = &rule.when else {
                continue;
            };
            // Unreachable (see `predicate_of`): a rule this read cannot see
            // pins nothing at all.
            if !when.pins::<E>(&mut pinned, &mut contested) {
                return Vec::new();
            }
            for (column, value) in pinned.drain(..) {
                match agreed.iter().find(|(held, _)| same_column(held, &column)) {
                    Some((_, held)) if *held == value => {}
                    Some(_) => contested.push(column),
                    None => agreed.push((column, value)),
                }
            }
        }
        agreed.retain(|(column, _)| !contested.iter().any(|c| same_column(c, column)));
        agreed
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
    ///
    /// [`Action::Manage`] holds the model to every concrete action.
    pub fn can<E: EntityTrait>(&self, action: Action, model: &E::Model) -> bool {
        self.actions_at_site(action, TypeId::of::<E>())
            .iter()
            .all(|&action| {
                self.evaluate::<E>(action, model, Provenance::Held, &nothing_invented)
                    .allowed
            })
    }

    /// The single rule scan behind [`can`](Self::can),
    /// [`permitted_fields`](Self::permitted_fields) and
    /// [`mask_many`](Self::mask_many): one pass yields both answers.
    ///
    /// A rule the row decides counts as SQL counts it, `UNKNOWN` included.
    /// `invented` names the columns the row was given rather than read, and
    /// `provenance` says what a rule left undecided means.
    ///
    /// A reply at a site gated on [`Action::Manage`] shows what the caller may
    /// read, so `Manage` reads `Read`'s rules here; [`can`](Self::can) asks each
    /// concrete action itself.
    pub(crate) fn evaluate<E: EntityTrait>(
        &self,
        action: Action,
        model: &E::Model,
        provenance: Provenance,
        invented: &dyn Fn(E::Column) -> bool,
    ) -> Verdict {
        let action = match action {
            Action::Manage => Action::Read,
            concrete => concrete,
        };
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
            let truth = match &rule.when {
                None => Truth::True,
                Some(when) => match when.decide::<E>(model, invented) {
                    Some(truth) => truth,
                    None => {
                        // Unreachable type mismatch (see `predicate_of`) — fail
                        // closed: a broken denial denies, a broken grant never widens.
                        denied |= rule.inverted;
                        continue;
                    }
                },
            };
            match (truth, rule.inverted) {
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

    /// Layer ③ — the model's exposed columns, stripped of those this ability
    /// does not permit for `action`.
    ///
    /// Only what `#[expose]` declares is ever written: an unrestricted grant
    /// opens every exposed column, never a hidden one. [`Action::Manage`] shows
    /// what the caller may read.
    pub fn mask<E: WireModel>(&self, action: Action, model: &E::Model) -> serde_json::Value {
        self.mask_with::<E>(action, model, self.permitted_fields::<E>(action, model))
    }

    /// [`mask`](Self::mask) with the field verdict already known.
    fn mask_with<E: WireModel>(
        &self,
        action: Action,
        model: &E::Model,
        fields: FieldSet,
    ) -> serde_json::Value {
        let mut wire = match E::to_wire(model) {
            Ok(wire) => wire,
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
        if let FieldSet::Only(allowed) = fields {
            wire.retain(|key, _| allowed.contains(key.as_str()));
        }
        serde_json::Value::Object(wire)
    }

    /// Layer ③ over a collection: drop the instances the actor may not see and
    /// mask the fields of those it may ([`mask`](Ability::mask)).
    ///
    /// The rows are a reply to a read through `Repo` under this ability for
    /// this action, whose SQL decided what the rows cannot show: a relation
    /// counts toward a row's visibility as that read counted it, and widens no
    /// field set a decided grant narrows. [`Action::Manage`] judges them at
    /// [`Action::Read`].
    pub fn mask_many<'m, E>(
        &self,
        action: Action,
        models: impl IntoIterator<Item = &'m E::Model>,
    ) -> Vec<serde_json::Value>
    where
        E: WireModel,
        E::Model: 'm,
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
    /// [`Action::Manage`] reads [`Action::Read`]'s grants.
    pub fn permitted_fields<E: EntityTrait>(&self, action: Action, model: &E::Model) -> FieldSet {
        self.evaluate::<E>(action, model, Provenance::Loaded, &nothing_invented)
            .fields
    }
}

/// The rule-map keys a concrete `action` reads: its own, then the wildcard's. A
/// site gated on [`Action::Manage`] reads each concrete action, never this with
/// `Manage` itself.
fn keys_for(action: Action, subject: TypeId) -> impl Iterator<Item = (Action, TypeId)> {
    [(action, subject), (Action::Manage, subject)].into_iter()
}

/// Whether two columns are one: sea-orm's `Column` enums derive no `PartialEq`.
fn same_column<C: IdenStatic>(a: &C, b: &C) -> bool {
    a.as_str() == b.as_str()
}

/// What one rule scan concluded about a model: whether it is visible, and which
/// of its columns the matching grants expose.
pub(crate) struct Verdict {
    pub(crate) allowed: bool,
    pub(crate) fields: FieldSet,
}

/// Recover a rule's typed predicate. A mismatch cannot happen (rules are keyed
/// by `TypeId::of::<E>()`), and fails closed at the call sites, never panics.
fn predicate_of<E: EntityTrait>(predicate: &(dyn Any + Send + Sync)) -> Option<&Predicate<E>> {
    let predicate = predicate.downcast_ref::<Predicate<E>>();
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
    use nest_rs_guards::Scope;

    use super::*;
    use crate::predicate::PredicateBuilder;

    mod widget {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
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

    /// How many of `rows` a reply to a read keeps: the verdict `mask_many` drops
    /// a row on.
    fn kept(ability: &Ability, rows: &[post::Model]) -> usize {
        rows.iter()
            .filter(|row| {
                ability
                    .evaluate::<post::Entity>(
                        Action::Read,
                        row,
                        Provenance::Loaded,
                        &nothing_invented,
                    )
                    .allowed
            })
            .count()
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

        assert_eq!(kept(&ability, &[a_post()]), 1);
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

        assert_eq!(kept(&ability, &[a_post(), a_post()]), 2);
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
        assert_eq!(kept(&ability, &[a_post()]), 1);
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
            when: Some(When::Predicate(Arc::new(
                Predicate::<gadget::Entity>::Always,
            ))),
            fields: FieldSet::All,
        }
    }

    /// The one predicate a factory's rule narrows its rows by.
    fn narrowing(rule: &Rule) -> &(dyn Any + Send + Sync) {
        match &rule.when {
            Some(When::Predicate(predicate)) => &**predicate,
            _ => panic!("a rule narrowed by one predicate"),
        }
    }

    #[test]
    fn a_predicate_of_another_subject_yields_no_grant_and_is_reported() {
        let logs = nest_rs_testing::LogCapture::install();
        let rule = mismatched_rule();

        assert!(
            predicate_of::<widget::Entity>(narrowing(&rule)).is_none(),
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
            when: Some(When::Predicate(Arc::new(
                Predicate::<widget::Entity>::Always,
            ))),
            fields: FieldSet::All,
        };

        assert!(predicate_of::<widget::Entity>(narrowing(&rule)).is_some());
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

    /// What a create would be filled with, by column name.
    fn pins(ability: &Ability) -> Vec<(&'static str, sea_orm::Value)> {
        pins_for(ability, Action::Create)
    }

    /// What `action`'s grants pin, by column name.
    fn pins_for(ability: &Ability, action: Action) -> Vec<(&'static str, sea_orm::Value)> {
        ability
            .pinned_values::<post::Entity>(action)
            .into_iter()
            .map(|(column, value)| (column.as_str(), value))
            .collect()
    }

    fn org(id: i32) -> (&'static str, sea_orm::Value) {
        ("org_id", id.into())
    }

    #[test]
    fn a_create_grant_pins_every_column_it_equates() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Create, post::Entity).when(|p| {
            p.all([
                p.eq(post::Column::OrgId, 7),
                p.eq(post::Column::Title, "draft"),
            ])
        });
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), [org(7), ("title", "draft".into())]);
    }

    #[test]
    fn the_manage_wildcard_pins_a_create_and_another_action_does_not() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::Title, "read"));
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), [org(7)]);
    }

    #[test]
    fn a_grant_leaving_a_column_free_never_unpins_what_another_pins() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.can(Action::Create, post::Entity).when(|p| {
            p.all([
                p.eq(post::Column::OrgId, 7),
                p.eq(post::Column::Title, "authored"),
            ])
        });
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), [org(7), ("title", "authored".into())]);
    }

    #[test]
    fn grants_pinning_a_column_apart_leave_it_to_the_recheck() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Create, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.can(Action::Create, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 8));
        b.can(Action::Create, post::Entity).when(|p| {
            p.all([
                p.eq(post::Column::Title, "one"),
                p.eq(post::Column::Title, "two"),
            ])
        });
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), []);
    }

    #[test]
    fn a_column_constrained_but_by_equality_is_never_pinned() {
        type Constraint = fn(PredicateBuilder<post::Entity>) -> Predicate<post::Entity>;
        let constraints: [Constraint; 5] = [
            |p| p.is_in(post::Column::OrgId, [7, 8]),
            |p| p.gte(post::Column::OrgId, 7),
            |p| p.is_not_null(post::Column::OrgId),
            |p| p.any([p.eq(post::Column::OrgId, 7)]),
            |p| p.not(p.eq(post::Column::OrgId, 8)),
        ];
        for constraint in constraints {
            let mut b = crate::AbilityBuilder::new();
            b.can(Action::Create, post::Entity)
                .when(|p| p.eq(post::Column::OrgId, 7));
            b.can(Action::Create, post::Entity).when(constraint);
            let ability = b.build().unwrap();

            assert_eq!(pins(&ability), []);
        }
    }

    #[test]
    fn a_denial_and_an_unconditional_grant_pin_nothing() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Create, post::Entity);
        b.cannot(Action::Create, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 9));
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), []);
    }

    #[test]
    fn a_relation_pins_nothing_and_leaves_an_equated_column_pinned() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Create, post::Entity).when(|p| {
            p.all([
                in_org(7)(PredicateBuilder::new()),
                p.eq(post::Column::Title, "related"),
            ])
        });
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), [("title", "related".into())]);
    }

    #[test]
    fn a_grant_withheld_for_its_scope_pins_nothing() {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut b = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        b.can(Action::Create, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7))
            .requires_scope(Scope::new("posts:write"));
        let ability = b.build().unwrap();

        assert_eq!(pins(&ability), []);
    }

    /// The query `condition_for` lowers `action` on posts to, as Postgres reads it.
    fn sql(ability: &Ability, action: Action) -> String {
        use sea_orm::{DatabaseBackend, QueryFilter, QueryTrait};
        post::Entity::find()
            .filter(ability.condition_for::<post::Entity>(action))
            .build(DatabaseBackend::Postgres)
            .to_string()
    }

    fn posts() -> TypeId {
        TypeId::of::<post::Entity>()
    }

    #[test]
    fn an_intersection_grants_a_subject_only_where_both_do() {
        let mut mine = crate::AbilityBuilder::new();
        mine.can(Action::Read, post::Entity);
        mine.can(Action::Read, widget::Entity);
        let mut theirs = crate::AbilityBuilder::new();
        theirs.can(Action::Read, post::Entity);
        theirs.can(Action::Delete, post::Entity);
        let both = mine.build().unwrap().intersection(&theirs.build().unwrap());

        assert!(both.can_class(Action::Read, posts()));
        assert!(!both.can_class(Action::Read, TypeId::of::<widget::Entity>()));
        assert!(!both.can_class(Action::Delete, posts()));
        assert!(sql(&both, Action::Delete).contains("1 = 0"));
    }

    #[test]
    fn the_wildcard_meets_the_other_sides_action_and_nothing_more() {
        let mut managing = crate::AbilityBuilder::new();
        managing
            .can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        let mut reading = crate::AbilityBuilder::new();
        reading.can(Action::Read, post::Entity);
        let (managing, reading) = (managing.build().unwrap(), reading.build().unwrap());

        for both in [
            managing.intersection(&reading),
            reading.intersection(&managing),
        ] {
            assert!(both.can_class(Action::Read, posts()));
            assert!(!both.can_class(Action::Delete, posts()));
            assert!(!both.can_class(Action::Manage, posts()));
            assert!(both.can::<post::Entity>(Action::Read, &a_post()));
            assert!(!both.can::<post::Entity>(
                Action::Read,
                &post::Model {
                    org_id: 8,
                    ..a_post()
                }
            ));
        }
    }

    #[test]
    fn an_intersection_admits_a_row_both_conditions_admit() {
        let mut in_org = crate::AbilityBuilder::new();
        in_org
            .can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        let mut titled = crate::AbilityBuilder::new();
        titled
            .can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::Title, "hello"));
        let both = in_org
            .build()
            .unwrap()
            .intersection(&titled.build().unwrap());

        let read = |org_id, title: &str| {
            let row = post::Model {
                org_id,
                title: title.into(),
                ..a_post()
            };
            both.can::<post::Entity>(Action::Read, &row)
        };
        assert!(read(7, "hello"));
        assert!(!read(7, "other"));
        assert!(!read(8, "hello"));
        let filter = sql(&both, Action::Read);
        assert!(
            filter.contains(r#""org_id" = 7"#) && filter.contains(r#""title" = 'hello'"#),
            "{filter}"
        );
    }

    #[test]
    fn either_sides_denial_denies() {
        let mut reading = crate::AbilityBuilder::new();
        reading.can(Action::Read, post::Entity);
        let mut barred = crate::AbilityBuilder::new();
        barred.can(Action::Read, post::Entity);
        barred
            .cannot(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 8));
        let (reading, barred) = (reading.build().unwrap(), barred.build().unwrap());

        for both in [reading.intersection(&barred), barred.intersection(&reading)] {
            assert!(both.can::<post::Entity>(Action::Read, &a_post()));
            assert!(!both.can::<post::Entity>(
                Action::Read,
                &post::Model {
                    org_id: 8,
                    ..a_post()
                }
            ));
            assert!(sql(&both, Action::Read).contains("NOT"));
        }
    }

    #[test]
    fn an_intersection_exposes_the_columns_both_expose() {
        let mut mine = crate::AbilityBuilder::new();
        mine.can(Action::Read, post::Entity)
            .fields([post::Column::Title, post::Column::OrgId]);
        let mut theirs = crate::AbilityBuilder::new();
        theirs
            .can(Action::Read, post::Entity)
            .fields([post::Column::Title, post::Column::Id]);
        let both = mine.build().unwrap().intersection(&theirs.build().unwrap());

        assert_eq!(
            only(both.permitted_fields::<post::Entity>(Action::Read, &a_post())),
            Some(vec!["title"]),
        );
    }

    #[test]
    fn a_relation_one_side_cannot_decide_stays_undecided_on_a_pushed_row() {
        let mut related = crate::AbilityBuilder::new();
        related.can(Action::Read, post::Entity).when(in_org(7));
        let mut in_org = crate::AbilityBuilder::new();
        in_org
            .can(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        let both = related
            .build()
            .unwrap()
            .intersection(&in_org.build().unwrap());

        let verdict = |provenance| {
            both.evaluate::<post::Entity>(Action::Read, &a_post(), provenance, &nothing_invented)
                .allowed
        };
        assert!(verdict(Provenance::Loaded), "the read's SQL applied both");
        assert!(!verdict(Provenance::Pushed), "no read applied the relation");
    }

    #[test]
    fn an_intersection_pins_what_either_side_pins_unless_they_disagree() {
        let creating = |column: post::Column, value: sea_orm::Value| {
            let mut b = crate::AbilityBuilder::new();
            b.can(Action::Create, post::Entity)
                .when(move |p| p.eq(column, value));
            b.build().unwrap()
        };
        let in_org = creating(post::Column::OrgId, 7.into());

        assert_eq!(
            pins(&in_org.intersection(&creating(post::Column::Title, "draft".into()))),
            [org(7), ("title", "draft".into())],
        );
        assert_eq!(
            pins(&in_org.intersection(&creating(post::Column::OrgId, 8.into()))),
            []
        );
    }

    #[test]
    fn a_visitor_on_either_side_is_a_visitor() {
        let caller = crate::AbilityBuilder::new().build().unwrap();
        let visitor = crate::AbilityBuilder::new().build_visitor().unwrap();

        assert!(caller.intersection(&visitor).is_visitor());
        assert!(visitor.intersection(&caller).is_visitor());
        assert!(!caller.intersection(&caller).is_visitor());
    }

    #[test]
    fn a_withheld_grant_is_named_only_where_the_other_side_grants() {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut scoped = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        scoped
            .can(Action::Read, post::Entity)
            .requires_scope(Scope::new("posts:read"));
        let scoped = scoped.build().unwrap();
        let mut reading = crate::AbilityBuilder::new();
        reading.can(Action::Read, post::Entity);
        let nothing = crate::AbilityBuilder::new().build().unwrap();

        assert_eq!(
            scoped
                .intersection(&reading.build().unwrap())
                .missing_scopes(Action::Read, posts()),
            ["posts:read"],
        );
        assert!(
            scoped
                .intersection(&nothing)
                .missing_scopes(Action::Read, posts())
                .is_empty(),
            "the scope alone would not grant it",
        );
    }

    /// A `when` as a factory writes one; `None` is a grant no `when` narrows.
    type Shape = Option<fn(PredicateBuilder<post::Entity>) -> Predicate<post::Entity>>;

    /// Every shape a grant's narrowing takes, the ones admitting no row or
    /// every row without a column among them.
    fn shapes() -> [Shape; 13] {
        [
            None,
            Some(|p| p.all(Vec::new())),
            Some(|p| p.any(Vec::new())),
            Some(|p| p.not(p.all(Vec::new()))),
            Some(|p| p.not(p.any(Vec::new()))),
            Some(|p| p.eq(post::Column::OrgId, 7)),
            Some(|p| p.not(p.eq(post::Column::OrgId, 7))),
            Some(|p| p.is_in(post::Column::OrgId, [7, 8])),
            Some(|p| p.is_in(post::Column::OrgId, Vec::<i32>::new())),
            Some(|p| p.ne(post::Column::Title, "hello")),
            Some(|p| p.gt(post::Column::Id, 1)),
            Some(|p| p.is_null(post::Column::ParentId)),
            Some(|p| {
                p.any([
                    p.eq(post::Column::OrgId, 8),
                    p.eq(post::Column::Title, "hello"),
                ])
            }),
        ]
    }

    /// The predicate `shape` narrows to, every row where it narrows nothing.
    fn lowered(shape: Shape) -> Predicate<post::Entity> {
        match shape {
            Some(shape) => shape(PredicateBuilder::new()),
            None => PredicateBuilder::new().all(Vec::new()),
        }
    }

    /// An ability granting `Read` on posts where `shape` admits the row.
    fn reading_where(shape: Shape) -> Ability {
        let mut b = crate::AbilityBuilder::new();
        match shape {
            Some(shape) => {
                b.can(Action::Read, post::Entity).when(shape);
            }
            None => {
                b.can(Action::Read, post::Entity);
            }
        }
        b.build().unwrap()
    }

    #[test]
    fn an_intersection_keeps_a_grant_admitting_no_row_admitting_none() {
        let in_org = reading_where(Some(|p| p.eq(post::Column::OrgId, 7)));
        let every = reading_where(None);
        for nothing in [
            reading_where(Some(|p| p.any(Vec::new()))),
            reading_where(Some(|p| p.not(p.all(Vec::new())))),
        ] {
            for wider in [&in_org, &every] {
                for both in [nothing.intersection(wider), wider.intersection(&nothing)] {
                    assert!(!both.can::<post::Entity>(Action::Read, &a_post()));
                    let filter = sql(&both, Action::Read);
                    assert!(
                        filter.contains("FALSE") || filter.contains("NOT TRUE"),
                        "the intersection reads rows its caller's grant admits none of: {filter}",
                    );
                }
            }
        }
    }

    #[test]
    fn an_intersection_reads_as_the_conjunction_a_factory_would_write() {
        let rows = [
            a_post(),
            post::Model {
                org_id: 8,
                ..a_post()
            },
            post::Model {
                id: 2,
                title: "other".into(),
                parent_id: Some(1),
                ..a_post()
            },
        ];
        for mine in shapes() {
            for theirs in shapes() {
                let both = reading_where(mine).intersection(&reading_where(theirs));
                let mut b = crate::AbilityBuilder::new();
                b.can(Action::Read, post::Entity)
                    .when(|p| p.all([lowered(mine), lowered(theirs)]));
                let written = b.build().unwrap();

                assert_eq!(
                    sql(&both, Action::Read),
                    sql(&written, Action::Read),
                    "SQL reads the intersection as the conjunction",
                );
                for row in &rows {
                    assert_eq!(
                        both.can::<post::Entity>(Action::Read, row),
                        written.can::<post::Entity>(Action::Read, row),
                        "the instance check reads it as SQL does: {row:?}",
                    );
                }
            }
        }
    }

    /// CASL's canonical shape: an org-wide `Manage`, its `Create` clawed back.
    fn managing_but_creating() -> crate::AbilityBuilder {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.cannot(Action::Create, post::Entity);
        b
    }

    #[test]
    fn a_denial_no_when_narrows_refuses_its_action_at_the_class_gate() {
        let ability = managing_but_creating().build().unwrap();

        assert!(!ability.can_class(Action::Create, posts()));
        assert!(
            ability.can_class(Action::Read, posts()),
            "a denial of one action leaves the wildcard's others",
        );
        assert!(ability.can_class(Action::Update, posts()));
    }

    #[test]
    fn a_denial_of_the_wildcard_refuses_every_action() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity);
        b.can(Action::Read, post::Entity);
        b.cannot(Action::Manage, post::Entity);
        let ability = b.build().unwrap();

        for action in [
            Action::Read,
            Action::Create,
            Action::Update,
            Action::Delete,
            Action::Manage,
        ] {
            assert!(!ability.can_class(action, posts()), "{action:?}");
        }
    }

    #[test]
    fn a_denial_a_when_narrows_leaves_the_class_gate_optimistic() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity);
        b.cannot(Action::Create, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 9));
        let ability = b.build().unwrap();

        assert!(
            ability.can_class(Action::Create, posts()),
            "the rows decide a conditional denial, in SQL and on the model",
        );
    }

    /// A `when` that holds for every row by construction narrows nothing: the
    /// denial bars the class as one with no `when` does.
    #[test]
    fn a_denial_whose_when_holds_for_every_row_refuses_at_the_class_gate() {
        type Rows = fn(PredicateBuilder<post::Entity>) -> crate::Predicate<post::Entity>;
        let every_row: [(&str, Rows); 5] = [
            ("an empty conjunction", |p| p.all([])),
            ("the unconditional predicate", |_| crate::Predicate::Always),
            ("a disjunction holding one", |p| {
                p.any([p.eq(post::Column::OrgId, 9), p.all([])])
            }),
            ("a conjunction of them", |p| {
                p.all([p.all([]), p.any([p.all([])])])
            }),
            ("the negation of an empty disjunction", |p| p.not(p.any([]))),
        ];
        for (shape, rows) in every_row {
            let mut b = crate::AbilityBuilder::new();
            b.can(Action::Manage, post::Entity);
            b.cannot(Action::Create, post::Entity).when(rows);
            let ability = b.build().unwrap();

            assert!(!ability.can_class(Action::Create, posts()), "{shape}");
            assert!(ability.can_class(Action::Read, posts()), "{shape}");
        }
    }

    #[test]
    fn a_denial_whose_when_still_reads_a_column_stays_conditional() {
        type Rows = fn(PredicateBuilder<post::Entity>) -> crate::Predicate<post::Entity>;
        let some_rows: [Rows; 3] = [
            |p| p.all([p.all([]), p.eq(post::Column::OrgId, 9)]),
            |p| p.any([p.any([]), p.eq(post::Column::OrgId, 9)]),
            |p| p.not(p.eq(post::Column::OrgId, 9)),
        ];
        for rows in some_rows {
            let mut b = crate::AbilityBuilder::new();
            b.can(Action::Manage, post::Entity);
            b.cannot(Action::Create, post::Entity).when(rows);
            let ability = b.build().unwrap();

            assert!(ability.can_class(Action::Create, posts()));
        }
    }

    #[test]
    fn a_denial_on_another_subject_refuses_nothing_here() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Read, post::Entity);
        b.cannot(Action::Read, widget::Entity);
        let ability = b.build().unwrap();

        assert!(ability.can_class(Action::Read, posts()));
    }

    #[test]
    fn no_scope_lifts_a_denial_so_none_is_named() {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut b = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        b.can(Action::Create, post::Entity)
            .requires_scope(Scope::new("posts:write"));
        b.cannot(Action::Create, post::Entity);
        let ability = b.build().unwrap();

        assert!(!ability.can_class(Action::Create, posts()));
        assert!(
            ability.missing_scopes(Action::Create, posts()).is_empty(),
            "a wider token would be refused all the same",
        );
    }

    #[test]
    fn a_denial_of_any_action_refuses_a_site_gated_on_manage() {
        for denied in Action::CONCRETE {
            let mut b = crate::AbilityBuilder::new();
            b.can(Action::Manage, post::Entity);
            b.cannot(denied, post::Entity);
            let ability = b.build().unwrap();

            assert!(!ability.can_class(Action::Manage, posts()), "{denied:?}");
            assert!(
                ability.denied_outright(Action::Manage, posts()),
                "{denied:?}: the refusal is the denial's, which no scope lifts",
            );
            assert!(ability.missing_scopes(Action::Manage, posts()).is_empty());
            assert!(!ability.can::<post::Entity>(Action::Manage, &a_post()));
            for other in Action::CONCRETE {
                assert_eq!(
                    ability.can_class(other, posts()),
                    other != denied,
                    "{denied:?} leaves {other:?} as it was",
                );
            }
        }
    }

    #[test]
    fn a_grant_of_every_action_passes_a_site_gated_on_manage_and_three_do_not() {
        let mut b = crate::AbilityBuilder::new();
        for action in Action::CONCRETE {
            b.can(action, post::Entity);
        }
        let every = b.build().unwrap();
        assert!(every.can_class(Action::Manage, posts()));
        assert!(every.can::<post::Entity>(Action::Manage, &a_post()));

        for absent in Action::CONCRETE {
            let mut b = crate::AbilityBuilder::new();
            for action in Action::CONCRETE {
                if action != absent {
                    b.can(action, post::Entity);
                }
            }
            let ability = b.build().unwrap();

            assert!(!ability.can_class(Action::Manage, posts()), "{absent:?}");
            assert!(!ability.can::<post::Entity>(Action::Manage, &a_post()));
            assert!(
                sql(&ability, Action::Manage).contains("1 = 0"),
                "{absent:?}: no grant, no row",
            );
        }
    }

    #[test]
    fn the_wildcards_rules_alone_read_as_one_condition_at_a_site_gated_on_manage() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.cannot(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::Title, "draft"));
        let ability = b.build().unwrap();

        let rendered = sql(&ability, Action::Manage);
        assert_eq!(rendered, sql(&ability, Action::Read));
        assert_eq!(
            rendered.matches(r#""org_id" = 7"#).count(),
            1,
            "one condition, not four: {rendered}",
        );
    }

    #[test]
    fn a_site_gated_on_manage_conjoins_the_condition_of_every_action() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.can(Action::Update, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 8));
        b.cannot(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::Title, "draft"));
        b.cannot(Action::Delete, post::Entity)
            .when(|p| p.eq(post::Column::Title, "pinned"));
        let ability = b.build().unwrap();

        let rendered = sql(&ability, Action::Manage);
        assert!(
            rendered.ends_with(concat!(
                r#"WHERE "posts"."org_id" = 7 AND (NOT "posts"."title" = 'draft')"#,
                r#" AND "posts"."org_id" = 7"#,
                r#" AND ("posts"."org_id" = 8 OR "posts"."org_id" = 7)"#,
                r#" AND ("posts"."org_id" = 7 AND (NOT "posts"."title" = 'pinned'))"#,
            )),
            "`Read`, `Create`, `Update` and `Delete`, each with its wildcard: {rendered}",
        );

        let (mine, draft, pinned, elsewhere) = (
            a_post(),
            post::Model {
                title: "draft".into(),
                ..a_post()
            },
            post::Model {
                title: "pinned".into(),
                ..a_post()
            },
            post::Model {
                org_id: 8,
                ..a_post()
            },
        );
        assert!(ability.can::<post::Entity>(Action::Manage, &mine));
        assert!(
            !ability.can::<post::Entity>(Action::Manage, &draft),
            "`Read` refuses it"
        );
        assert!(
            !ability.can::<post::Entity>(Action::Manage, &pinned),
            "`Delete` refuses it",
        );
        assert!(
            !ability.can::<post::Entity>(Action::Manage, &elsewhere),
            "only `Update` admits it",
        );
    }

    #[test]
    fn the_instance_check_of_manage_holds_every_action_to_the_row() {
        let mut b = crate::AbilityBuilder::new();
        for (action, org) in [
            (Action::Read, 7),
            (Action::Create, 7),
            (Action::Update, 7),
            (Action::Delete, 8),
        ] {
            b.can(action, post::Entity)
                .when(move |p| p.eq(post::Column::OrgId, org));
        }
        let ability = b.build().unwrap();

        assert!(ability.can_class(Action::Manage, posts()));
        assert!(ability.can::<post::Entity>(Action::Read, &a_post()));
        assert!(!ability.can::<post::Entity>(Action::Manage, &a_post()));
    }

    #[test]
    fn a_reply_at_a_site_gated_on_manage_shows_what_the_caller_may_read() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .fields([post::Column::Id]);
        b.can(Action::Read, post::Entity)
            .fields([post::Column::Id, post::Column::Title]);
        b.can(Action::Update, post::Entity)
            .fields([post::Column::OrgId]);
        let ability = b.build().unwrap();

        assert_eq!(
            only(ability.permitted_fields::<post::Entity>(Action::Manage, &a_post())),
            Some(vec!["id", "title"]),
            "`Read`'s fields, the wildcard's among them, and never `Update`'s",
        );
    }

    #[test]
    fn a_row_read_refuses_is_refused_a_reply_at_a_site_gated_on_manage() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity);
        b.cannot(Action::Read, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        let ability = b.build().unwrap();

        for provenance in [Provenance::Loaded, Provenance::Pushed] {
            let allowed = |org_id| {
                let row = post::Model { org_id, ..a_post() };
                ability
                    .evaluate::<post::Entity>(Action::Manage, &row, provenance, &nothing_invented)
                    .allowed
            };
            assert!(!allowed(7));
            assert!(allowed(8));
        }
    }

    #[test]
    fn a_site_gated_on_manage_pins_what_every_action_pins() {
        let mut b = crate::AbilityBuilder::new();
        b.can(Action::Manage, post::Entity)
            .when(|p| p.eq(post::Column::OrgId, 7));
        b.can(Action::Update, post::Entity)
            .when(|p| p.eq(post::Column::Title, "pinned"));
        assert_eq!(
            pins_for(&b.build().unwrap(), Action::Manage),
            [org(7), ("title", sea_orm::Value::from("pinned"))],
        );

        let mut b = crate::AbilityBuilder::new();
        for (action, org) in [
            (Action::Read, 7),
            (Action::Create, 8),
            (Action::Update, 7),
            (Action::Delete, 7),
        ] {
            b.can(action, post::Entity)
                .when(move |p| p.eq(post::Column::OrgId, org));
        }
        assert!(
            pins_for(&b.build().unwrap(), Action::Manage).is_empty(),
            "two actions pinning it apart: no row holds both, and the re-check says so",
        );
    }

    #[test]
    fn the_wildcards_withheld_grant_alone_answers_a_site_gated_on_manage() {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut b = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        b.can(Action::Read, post::Entity)
            .requires_scope(Scope::new("posts:read"));
        b.can(Action::Manage, post::Entity)
            .requires_scope(Scope::new("posts:admin"));
        let ability = b.build().unwrap();

        assert!(!ability.can_class(Action::Manage, posts()));
        assert_eq!(
            ability.missing_scopes(Action::Manage, posts()),
            ["posts:admin"],
            "it opens all four at once, so nothing else is needed",
        );
    }

    #[test]
    fn an_actions_own_scope_answers_a_site_gated_on_manage_when_it_asks_no_more() {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut b = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        for action in [Action::Read, Action::Create, Action::Update] {
            b.can(action, post::Entity);
        }
        b.can(Action::Delete, post::Entity)
            .requires_scope(Scope::new("posts:purge"));
        b.can(Action::Manage, post::Entity)
            .requires_scope(Scope::new("posts:admin"));
        let ability = b.build().unwrap();

        assert!(!ability.can_class(Action::Manage, posts()));
        assert_eq!(
            ability.missing_scopes(Action::Manage, posts()),
            ["posts:purge"],
            "`Delete` is all it lacks, and the wildcard's grant is the wider of the two",
        );
    }

    #[test]
    fn a_site_gated_on_manage_names_what_each_refused_action_lacks() {
        let granted: std::sync::Arc<[String]> = vec!["posts:read".to_owned()].into();
        let mut b = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        b.can(Action::Read, post::Entity)
            .requires_scope(Scope::new("posts:read"));
        b.can(Action::Create, post::Entity)
            .requires_scope(Scope::new("posts:write"));
        b.can(Action::Update, post::Entity)
            .requires_scope(Scope::new("posts:write"));
        b.can(Action::Delete, post::Entity)
            .requires_scope(Scope::new("posts:purge"));
        let ability = b.build().unwrap();

        assert!(!ability.can_class(Action::Manage, posts()));
        assert_eq!(
            ability.missing_scopes(Action::Manage, posts()),
            ["posts:write", "posts:purge"],
            "each needed, together sufficient, none twice, none already held",
        );
    }

    #[test]
    fn no_scope_answers_a_site_gated_on_manage_an_action_no_grant_opens() {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut b = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        b.can(Action::Read, post::Entity)
            .requires_scope(Scope::new("posts:read"));
        let ability = b.build().unwrap();

        assert!(!ability.can_class(Action::Manage, posts()));
        assert!(
            ability.missing_scopes(Action::Manage, posts()).is_empty(),
            "a wider token would be refused all the same",
        );
    }

    /// A credential withheld the wildcard's grant for `posts:admin`, intersected
    /// with the ability `grants` builds.
    fn withheld_beside(grants: fn(&mut crate::AbilityBuilder)) -> Ability {
        let granted: std::sync::Arc<[String]> = Vec::new().into();
        let mut scoped = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
        scoped
            .can(Action::Manage, post::Entity)
            .requires_scope(Scope::new("posts:admin"));
        let mut beside = crate::AbilityBuilder::new();
        grants(&mut beside);
        scoped
            .build()
            .unwrap()
            .intersection(&beside.build().unwrap())
    }

    #[test]
    fn a_withheld_wildcard_is_named_for_each_action_the_other_side_grants() {
        let both = withheld_beside(|b| {
            b.can(Action::Read, post::Entity);
        });
        assert_eq!(both.missing_scopes(Action::Read, posts()), ["posts:admin"]);
        assert!(
            both.missing_scopes(Action::Create, posts()).is_empty(),
            "the scope alone would not grant it",
        );
        assert!(
            both.missing_scopes(Action::Manage, posts()).is_empty(),
            "the scope alone opens `Read`, not every action",
        );

        let both = withheld_beside(|b| {
            for action in Action::CONCRETE {
                b.can(action, post::Entity);
            }
        });
        assert_eq!(
            both.missing_scopes(Action::Manage, posts()),
            ["posts:admin"]
        );
        assert_eq!(
            both.missing_scopes(Action::Delete, posts()),
            ["posts:admin"]
        );

        let both = withheld_beside(|b| {
            b.can(Action::Manage, post::Entity);
            b.cannot(Action::Read, post::Entity);
        });
        assert_eq!(
            both.missing_scopes(Action::Create, posts()),
            ["posts:admin"],
            "the other side's denial of `Read` leaves `Create` to the scope",
        );
        assert!(both.missing_scopes(Action::Read, posts()).is_empty());
        assert!(both.missing_scopes(Action::Manage, posts()).is_empty());
    }

    /// The rule map's hasher differs per ability, so one run proves no order.
    #[test]
    fn an_intersection_names_an_actions_own_grant_before_the_wildcards() {
        for _ in 0..32 {
            let granted: std::sync::Arc<[String]> = Vec::new().into();
            let mut scoped = crate::AbilityBuilder::new().with_granted_scopes(Some(granted));
            scoped
                .can(Action::Manage, post::Entity)
                .requires_scope(Scope::new("posts:admin"));
            scoped
                .can(Action::Read, post::Entity)
                .requires_scope(Scope::new("posts:read"));
            let mut reading = crate::AbilityBuilder::new();
            reading.can(Action::Read, post::Entity);
            let both = scoped
                .build()
                .unwrap()
                .intersection(&reading.build().unwrap());

            assert_eq!(both.missing_scopes(Action::Read, posts()), ["posts:read"]);
        }
    }

    #[test]
    fn an_intersection_keeps_a_denial_no_when_narrows_whole() {
        let mut reading = crate::AbilityBuilder::new();
        reading.can(Action::Manage, post::Entity);
        let (reading, barred) = (
            reading.build().unwrap(),
            managing_but_creating().build().unwrap(),
        );

        for both in [reading.intersection(&barred), barred.intersection(&reading)] {
            assert!(!both.can_class(Action::Create, posts()));
            assert!(both.can_class(Action::Update, posts()));
        }
    }
}
