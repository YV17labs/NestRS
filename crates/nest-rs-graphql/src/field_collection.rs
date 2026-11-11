//! A document async-graphql 7.2.1 would answer otherwise than GraphQL (October
//! 2021) §6.3.2, *Field Collection*, reads it, rewritten before validation into
//! one it answers as the specification does ([`collected`]).
//!
//! async-graphql resolves each selection of one response key apart
//! (`Fields::add_set`) and joins their answers (`insert_value`), where §6.3.2
//! merges them into one field before anything runs. Three documents answer
//! otherwise: a mutation's root field selected twice runs its writes twice; a list
//! nested in a list selected twice keeps the first copy to finish whole, since
//! `insert_value` joins a later copy's items only where they are objects; and a
//! type condition on a union applies only where that union is the type resolved,
//! never to a member of it resolved elsewhere (*DoesFragmentTypeApply*).
//!
//! Such a document is rewritten once its cost was counted, which bounds what its
//! fragments expand to, and before validation, which judges the rewrite. The
//! operation's spreads are inlined, each under the type condition its fragment
//! names, and a fragment no other operation spreads leaves the document; the copies
//! of a key at a mutation's root, and of a key a list of lists stands at or below,
//! merge into the first, the others' selections appended to it; and a condition on
//! a union is rewritten for the members its position may hold. Nothing the client
//! wrote leaves validation's sight: a selection moves rather than goes, a copy
//! merged away leaves the `@skip` and `@include` a variable is read in, and what a
//! rewritten condition cannot apply to stays under it, where nothing runs it.
//! Complexity and depth count what runs. Copies naming different fields or
//! arguments, which no one field answers, are refused, as is a copy the rewrite
//! could not merge.

use std::collections::{HashMap, HashSet};

use async_graphql::indexmap::IndexMap;
use async_graphql::parser::types::{
    Directive, DocumentOperations, ExecutableDocument, Field, FragmentDefinition, InlineFragment,
    OperationDefinition, OperationType, Selection, SelectionSet, TypeCondition,
};
use async_graphql::registry::{MetaField, MetaType, MetaTypeName, Registry};
use async_graphql::{Name, Pos, Positioned, ServerError, Variables};

use crate::operation_units::{RECURSIVE_DEPTH, is_condition, is_skipped};

/// `document`, its operation `operation` collected as §6.3.2 reads it; `None` when
/// async-graphql answers it so as it stands, or when it cannot be rewritten, which
/// only a document validation refuses comes to. `registry` holds the schema's
/// types, and `variables` what `@skip` and `@include` read.
///
/// # Errors
///
/// The refusal of a response key whose copies no one field answers, at the first
/// copy that differs: copies naming different fields or arguments, at a mutation's
/// root or where a list of lists stands, and, in a document that could not be
/// rewritten, a mutation's root field selected twice or a list of lists selected
/// twice otherwise.
pub(crate) fn collected(
    document: &ExecutableDocument,
    operation: &OperationDefinition,
    registry: &Registry,
    variables: &Variables,
) -> Result<Option<ExecutableDocument>, ServerError> {
    let Some(root) = root_type(registry, operation.ty) else {
        return Ok(None);
    };
    let fragments = &document.fragments;
    if !needs_collecting(fragments, operation, root, registry, variables) {
        return Ok(None);
    }
    let mut collection = Collection {
        registry,
        variables,
        drawn: 0,
    };
    let Some(rewritten) = collection.rewrite(document, operation, root)? else {
        return match uncollected(fragments, operation, root, registry, variables) {
            Some(refusal) => Err(refusal),
            None => Ok(None),
        };
    };
    let Some(chosen) = chosen(&rewritten, operation) else {
        return Ok(None);
    };
    match uncollected(&rewritten.fragments, chosen, root, registry, variables) {
        Some(refusal) => Err(refusal),
        None => Ok(Some(rewritten)),
    }
}

/// The type an operation of type `ty` selects on, when the schema has one.
fn root_type(registry: &Registry, ty: OperationType) -> Option<&str> {
    match ty {
        OperationType::Query => Some(registry.query_type.as_str()),
        OperationType::Mutation => registry.mutation_type.as_deref(),
        OperationType::Subscription => registry.subscription_type.as_deref(),
    }
}

/// Whether async-graphql would answer `operation` otherwise than §6.3.2 collects it:
/// a mutation's root selecting one key or spreading one fragment twice, a condition
/// on a union no engine rule applies where a member of it may be resolved, or copies
/// of a key at one place that no one field answers or that a list of lists stands
/// at and selecting otherwise. The schema's types are read only for a document a
/// glance finds naming a union in a condition, or a key twice in one set.
fn needs_collecting(
    fragments: &HashMap<Name, Positioned<FragmentDefinition>>,
    operation: &OperationDefinition,
    root: &str,
    registry: &Registry,
    variables: &Variables,
) -> bool {
    let set = &operation.selection_set;
    if operation.ty == OperationType::Mutation
        && RootSelections::new(fragments, variables)
            .first_repeated(&set.node, 0)
            .is_some()
    {
        return true;
    }
    let glance = Glance::of(fragments, registry, &set.node);
    if glance.unions
        && Divergence::new(fragments, registry, variables).in_set(
            &set.node,
            root,
            &[(root, root)],
            0,
        )
    {
        return true;
    }
    glance.twice
        && Copies::new(fragments, registry, variables)
            .unjoined(&[set], root, 0)
            .is_some()
}

/// What one walk of a document tells before any type it selects is read: whether a
/// type condition in it names a union, and whether one of its selection sets,
/// through its fragments, conditions aside, names a response key twice. Every field's
/// set is read once, every key compared within its set alone.
#[derive(Default)]
struct Glance {
    unions: bool,
    twice: bool,
}

/// The walk a [`Glance`] takes, each buffer shared by the sets it reads, the
/// innermost set's part last.
struct Glancing<'a> {
    fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
    registry: &'a Registry,
    /// The keys of the sets being read.
    keys: Vec<&'a str>,
    /// The fields' sets still to read below the sets being read.
    below: Vec<&'a Positioned<SelectionSet>>,
    /// The fragments being expanded: a cycle, which validation refuses, ends there.
    expanding: Vec<&'a str>,
    /// The fields' sets read so far, in a document holding a fragment.
    walked: HashSet<Pos>,
    /// A wide set's keys, looked up rather than scanned for.
    wide: HashSet<&'a str>,
    seen: Glance,
}

impl Glance {
    /// The glance at `set`, through `fragments`, a condition's type read in
    /// `registry` alone.
    fn of(
        fragments: &HashMap<Name, Positioned<FragmentDefinition>>,
        registry: &Registry,
        set: &SelectionSet,
    ) -> Self {
        let mut walk = Glancing {
            fragments,
            registry,
            keys: Vec::new(),
            below: Vec::new(),
            expanding: Vec::new(),
            walked: HashSet::new(),
            wide: HashSet::new(),
            seen: Self::default(),
        };
        walk.set(set, 0);
        walk.seen
    }
}

impl<'a> Glancing<'a> {
    fn set(&mut self, set: &'a SelectionSet, depth: usize) {
        if depth > RECURSIVE_DEPTH || (self.seen.unions && self.seen.twice) {
            return;
        }
        let (keys, below) = (self.keys.len(), self.below.len());
        self.flatten(set, depth);
        self.seen.twice |= self.repeated(keys);
        self.keys.truncate(keys);
        let read = self.below.len();
        for at in below..read {
            let field_set = self.below[at];
            // Only a fragment spread again reaches a set twice.
            if self.fragments.is_empty() || self.walked.insert(field_set.pos) {
                self.set(&field_set.node, depth + 1);
            }
        }
        self.below.truncate(below);
    }

    /// The keys of `set` and the sets of its fields, through its fragments.
    fn flatten(&mut self, set: &'a SelectionSet, depth: usize) {
        if depth > RECURSIVE_DEPTH {
            return;
        }
        for selection in &set.items {
            match &selection.node {
                Selection::Field(field) => {
                    self.keys.push(field.node.response_key().node.as_str());
                    if !field.node.selection_set.node.items.is_empty() {
                        self.below.push(&field.node.selection_set);
                    }
                }
                Selection::InlineFragment(fragment) => {
                    if let Some(condition) = &fragment.node.type_condition {
                        self.condition(&condition.node.on.node);
                    }
                    self.flatten(&fragment.node.selection_set.node, depth + 1);
                }
                Selection::FragmentSpread(spread) => {
                    let Some((name, fragment)) = self
                        .fragments
                        .get_key_value(spread.node.fragment_name.node.as_str())
                    else {
                        continue;
                    };
                    if self.expanding.contains(&name.as_str()) {
                        continue;
                    }
                    self.condition(&fragment.node.type_condition.node.on.node);
                    self.expanding.push(name.as_str());
                    self.flatten(&fragment.node.selection_set.node, depth + 1);
                    self.expanding.pop();
                }
            }
        }
    }

    fn condition(&mut self, on: &str) {
        self.seen.unions |= matches!(self.registry.types.get(on), Some(MetaType::Union { .. }));
    }

    /// Whether the keys read since `from` name one twice: a few are compared, many
    /// looked up.
    fn repeated(&mut self, from: usize) -> bool {
        let keys = &self.keys[from..];
        if keys.len() <= 16 {
            return keys
                .iter()
                .enumerate()
                .any(|(at, key)| keys[..at].contains(key));
        }
        self.wide.clear();
        !keys.iter().all(|key| self.wide.insert(key))
    }
}

/// The refusal of what `operation` still holds that async-graphql would answer
/// otherwise than §6.3.2 reads: a key its root selects twice, when a mutation, or a
/// list of lists selected twice otherwise. What [`collected`] rewrote holds none:
/// this is the read standing behind it.
fn uncollected(
    fragments: &HashMap<Name, Positioned<FragmentDefinition>>,
    operation: &OperationDefinition,
    root: &str,
    registry: &Registry,
    variables: &Variables,
) -> Option<ServerError> {
    let set = &operation.selection_set;
    if operation.ty == OperationType::Mutation
        && let Some((repeated, at)) =
            RootSelections::new(fragments, variables).first_repeated(&set.node, 0)
    {
        let said = match repeated {
            Repeated::Key(key) => {
                format!(r#"the mutation selects "{key}" more than once at its root"#)
            }
            Repeated::Fragment(name) => {
                format!(r#"the mutation spreads "{name}" more than once at its root"#)
            }
        };
        return Some(crate::error::invalid_argument(said).into_server_error(at));
    }
    let found = Copies::new(fragments, registry, variables).unjoined(&[set], root, 0)?;
    Some(match found.unlike {
        true => unlike(found.key, found.at),
        false => crate::error::invalid_argument(format!(
            r#"the operation selects "{}", a list of lists, more than once at one place with different selections"#,
            found.key
        ))
        .into_server_error(found.at),
    })
}

/// The refusal of `key`, whose copy at `at` names another field or other arguments
/// than the first.
fn unlike(key: &str, at: Pos) -> ServerError {
    crate::error::invalid_argument(format!(
        r#"the operation selects "{key}" more than once at one place with different fields or arguments"#
    ))
    .into_server_error(at)
}

/// The operation of `document` that is `operation`, found again in its copy by
/// where its selections open.
pub(crate) fn chosen<'d>(
    document: &'d ExecutableDocument,
    operation: &OperationDefinition,
) -> Option<&'d OperationDefinition> {
    let at = operation.selection_set.pos;
    match &document.operations {
        DocumentOperations::Single(single) => Some(&single.node),
        DocumentOperations::Multiple(operations) => operations
            .values()
            .map(|candidate| &candidate.node)
            .find(|candidate| candidate.selection_set.pos == at),
    }
}

/// What a value of type `ty` may be: the object itself, or each possible type of an
/// interface or a union.
fn possible<'r>(registry: &'r Registry, ty: &'r str) -> Vec<&'r str> {
    match registry.types.get(ty) {
        Some(MetaType::Object { .. }) => vec![ty],
        Some(abstract_type) => abstract_type
            .possible_types()
            .map(|possible| possible.iter().map(String::as_str).collect())
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Whether a type condition on `condition` applies to a value of `concrete`, as
/// §6.3.2 *DoesFragmentTypeApply* reads it.
fn applies(registry: &Registry, condition: &str, concrete: &str) -> bool {
    condition == concrete
        || registry
            .types
            .get(condition)
            .and_then(MetaType::possible_types)
            .is_some_and(|possible| possible.contains(concrete))
}

/// The type a value of `concrete` resolved as `resolving` is resolved as below a
/// condition on `condition`, as async-graphql (`7.2.1`, `Fields::add_set`) applies
/// it: `concrete` below its own name or an interface it implements, `resolving`
/// below its own, and `None` below any other. So a condition on a union applies
/// only where that union is resolved, which [`collected`] rewrites wherever §6.3.2
/// applies it to a member.
pub(crate) fn engine_applies<'r>(
    registry: &Registry,
    condition: &str,
    concrete: &'r str,
    resolving: &'r str,
) -> Option<&'r str> {
    let implemented = registry
        .implements
        .get(concrete)
        .is_some_and(|interfaces| interfaces.contains(condition));
    if condition == concrete || implemented {
        Some(concrete)
    } else if condition == resolving {
        Some(resolving)
    } else {
        None
    }
}

/// Whether some possible type of `a` is one of `b`'s, which validation asks of a
/// fragment spread inside a set of `b` (`PossibleFragmentSpreads`).
fn overlaps(registry: &Registry, a: &str, b: &[&str]) -> bool {
    possible(registry, a)
        .iter()
        .any(|concrete| b.contains(concrete))
}

/// What field `name` of `on` declares, when the schema declares it.
fn declared<'r>(registry: &'r Registry, on: &str, name: &str) -> Option<&'r MetaField> {
    registry.types.get(on)?.field_by_name(name)
}

/// The named type field `name` of `on` answers, when the schema declares it.
pub(crate) fn field_type<'r>(registry: &'r Registry, on: &str, name: &str) -> Option<&'r str> {
    declared(registry, on, name).map(|declared| MetaTypeName::concrete_typename(&declared.ty))
}

/// Whether `ty`, a field's declared type, holds a list inside a list.
fn nests_lists(ty: &str) -> bool {
    let (mut ty, mut lists) = (ty, 0);
    loop {
        match MetaTypeName::create(ty) {
            MetaTypeName::List(item) => {
                lists += 1;
                ty = item;
            }
            MetaTypeName::NonNull(inner) => ty = inner,
            MetaTypeName::Named(_) => return lists > 1,
        }
    }
}

/// Whether two copies name one field with one set of arguments, in any order, and
/// one list of directives, `@skip` and `@include` aside, which were read before:
/// copies §5.3.2 lets merge.
fn same_field(a: &Field, b: &Field) -> bool {
    a.name.node == b.name.node
        && a.arguments.len() == b.arguments.len()
        && a.arguments.iter().all(|(name, value)| {
            b.arguments
                .iter()
                .any(|(other, written)| other.node == name.node && written.node == value.node)
        })
        && alike_directives(&a.directives, &b.directives)
}

/// Whether two selections carry one directive list, `@skip` and `@include` aside.
fn alike_directives(a: &[Positioned<Directive>], b: &[Positioned<Directive>]) -> bool {
    let read = |directive: &&Positioned<Directive>| !is_condition(directive);
    let same = |a: &Positioned<Directive>, b: &Positioned<Directive>| {
        a.node.name.node == b.node.name.node && a.node.arguments == b.node.arguments
    };
    let (mut a, mut b) = (a.iter().filter(read), b.iter().filter(read));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if same(a, b) => {}
            _ => return false,
        }
    }
}

/// The selections of `set` that `@skip` and `@include` leave in.
fn running<'s>(
    set: &'s SelectionSet,
    variables: &'s Variables,
) -> impl Iterator<Item = &'s Positioned<Selection>> {
    set.items
        .iter()
        .filter(move |selection| !is_skipped(selection.node.directives(), variables))
}

/// A mutation's root selections, read before validation: each fragment is
/// expanded once at most and no deeper than [`RECURSIVE_DEPTH`], so the walk
/// never outgrows the document or the stack, and a fragment cycle, which
/// validation refuses, ends it.
struct RootSelections<'d> {
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    variables: &'d Variables,
    keys: HashSet<&'d str>,
    spread: HashSet<&'d str>,
    /// The fragments being expanded, the innermost last.
    expanding: Vec<&'d str>,
}

/// What a root selection names a second time.
enum Repeated<'d> {
    Key(&'d str),
    Fragment(&'d str),
}

impl<'d> RootSelections<'d> {
    fn new(
        fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
        variables: &'d Variables,
    ) -> Self {
        Self {
            fragments,
            variables,
            keys: HashSet::new(),
            spread: HashSet::new(),
            expanding: Vec::new(),
        }
    }

    /// The first selection of `set`, `depth` selection sets below the root,
    /// naming a response key, or spreading a fragment, the root already holds, and
    /// where it stands. A selection `@skip` or `@include` leaves out runs nothing,
    /// so it repeats nothing.
    fn first_repeated(
        &mut self,
        set: &'d SelectionSet,
        depth: usize,
    ) -> Option<(Repeated<'d>, Pos)> {
        if depth > RECURSIVE_DEPTH {
            return None;
        }
        let variables = self.variables;
        running(set, variables).find_map(|selection| match &selection.node {
            Selection::Field(field) => {
                let key = field.node.response_key().node.as_str();
                (!self.keys.insert(key)).then_some((Repeated::Key(key), selection.pos))
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                if self.expanding.contains(&name) {
                    return None;
                }
                if !self.spread.insert(name) {
                    return Some((Repeated::Fragment(name), selection.pos));
                }
                let fragment = self.fragments.get(name)?;
                self.expanding.push(name);
                let repeated = self.first_repeated(&fragment.node.selection_set.node, depth + 1);
                self.expanding.pop();
                repeated
            }
            Selection::InlineFragment(fragment) => {
                self.first_repeated(&fragment.node.selection_set.node, depth + 1)
            }
        })
    }
}

/// The values a set may meet: each a concrete type, and the type async-graphql
/// resolves it as there.
type Reaching<'a> = Vec<(&'a str, &'a str)>;

/// Where a condition on a union meets a value of a member of it no engine rule
/// applies it to, read through the document's fragments, each field's selections
/// once and each fragment once per values it may meet.
struct Divergence<'a> {
    fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
    registry: &'a Registry,
    variables: &'a Variables,
    /// The fields' selection sets read so far.
    walked: HashSet<Pos>,
    /// What each fragment was found to hold, by the values it was read for.
    spreads: HashMap<(&'a str, Reaching<'a>), bool>,
}

impl<'a> Divergence<'a> {
    fn new(
        fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
        registry: &'a Registry,
        variables: &'a Variables,
    ) -> Self {
        Self {
            fragments,
            registry,
            variables,
            walked: HashSet::new(),
            spreads: HashMap::new(),
        }
    }

    /// Whether `set`, selected on `parent`, holds such a condition, its values each a
    /// concrete type and the type it is resolved as (`reaching`).
    fn in_set(
        &mut self,
        set: &'a SelectionSet,
        parent: &'a str,
        reaching: &[(&'a str, &'a str)],
        depth: usize,
    ) -> bool {
        if depth > RECURSIVE_DEPTH {
            return false;
        }
        let registry = self.registry;
        running(set, self.variables).any(|selection| match &selection.node {
            Selection::Field(field) => {
                let below = &field.node.selection_set;
                if below.node.items.is_empty() || !self.walked.insert(below.pos) {
                    return false;
                }
                let Some(ty) = field_type(registry, parent, &field.node.name.node) else {
                    return false;
                };
                let reaching: Vec<_> = possible(registry, ty)
                    .into_iter()
                    .map(|concrete| (concrete, ty))
                    .collect();
                self.in_set(&below.node, ty, &reaching, depth + 1)
            }
            Selection::InlineFragment(fragment) => match &fragment.node.type_condition {
                Some(condition) => self.under(
                    condition.node.on.node.as_str(),
                    &fragment.node.selection_set.node,
                    reaching,
                    depth,
                ),
                None => self.in_set(
                    &fragment.node.selection_set.node,
                    parent,
                    reaching,
                    depth + 1,
                ),
            },
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                let Some(fragment) = self.fragments.get(name) else {
                    return false;
                };
                let read_as = (name, reaching.to_vec());
                if let Some(found) = self.spreads.get(&read_as) {
                    return *found;
                }
                self.spreads.insert(read_as.clone(), false);
                let found = self.under(
                    fragment.node.type_condition.node.on.node.as_str(),
                    &fragment.node.selection_set.node,
                    reaching,
                    depth,
                );
                self.spreads.insert(read_as, found);
                found
            }
        })
    }

    /// Whether a condition on `condition` holding `set` is one, or holds one.
    fn under(
        &mut self,
        condition: &'a str,
        set: &'a SelectionSet,
        reaching: &[(&'a str, &'a str)],
        depth: usize,
    ) -> bool {
        let registry = self.registry;
        if diverges(registry, condition, reaching) {
            return true;
        }
        let reaching: Vec<_> = reaching
            .iter()
            .filter_map(|&(concrete, resolving)| {
                engine_applies(registry, condition, concrete, resolving)
                    .map(|resolving| (concrete, resolving))
            })
            .collect();
        !reaching.is_empty() && self.in_set(set, condition, &reaching, depth + 1)
    }
}

/// Whether a condition on `condition` is one on a union which async-graphql applies
/// to none of the values `reaching`, where some of them are members of it.
fn diverges(registry: &Registry, condition: &str, reaching: &[(&str, &str)]) -> bool {
    matches!(registry.types.get(condition), Some(MetaType::Union { .. }))
        && reaching
            .iter()
            .all(|&(_, resolving)| resolving != condition)
        && reaching
            .iter()
            .any(|&(concrete, _)| applies(registry, condition, concrete))
}

/// A key a place selects more than once whose copies name another field or other
/// arguments, or answer a list of lists at that place or below and select
/// otherwise: the first copy that does, and whether it names another field.
#[derive(Clone, Copy)]
struct Unjoined<'a> {
    key: &'a str,
    at: Pos,
    unlike: bool,
}

/// The copies of a response key at one place, read against the schema's types, a
/// value of each possible type at a time, as §6.3.2 applies conditions.
struct Copies<'a> {
    fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
    registry: &'a Registry,
    variables: &'a Variables,
    /// The fields each fragment collects of a value of a type, read once.
    collected: HashMap<(&'a str, &'a str), Vec<&'a Positioned<Field>>>,
    /// What each place read was found to hold, by where its copies open and the
    /// type it declares.
    read: HashMap<(Vec<Pos>, &'a str), Option<Unjoined<'a>>>,
}

impl<'a> Copies<'a> {
    fn new(
        fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
        registry: &'a Registry,
        variables: &'a Variables,
    ) -> Self {
        Self {
            fragments,
            registry,
            variables,
            collected: HashMap::new(),
            read: HashMap::new(),
        }
    }

    /// The first key, at the place the sets `copies` merge into, all selected on
    /// `ty`, or below it, whose copies of a value of one type name another field or
    /// other arguments, which §5.3.2 refuses and async-graphql 7.2.1's validation
    /// reads under one type condition alone, or answer a list of lists and select
    /// otherwise. Each place is read once, and copies alike stand for one.
    fn unjoined(
        &mut self,
        copies: &[&'a Positioned<SelectionSet>],
        ty: &'a str,
        depth: usize,
    ) -> Option<Unjoined<'a>> {
        if depth > RECURSIVE_DEPTH {
            return None;
        }
        let place = (copies.iter().map(|set| set.pos).collect::<Vec<_>>(), ty);
        if let Some(found) = self.read.get(&place) {
            return *found;
        }
        self.read.insert(place.clone(), None);
        let found = self.unjoined_at(copies, ty, depth);
        self.read.insert(place, found);
        found
    }

    fn unjoined_at(
        &mut self,
        copies: &[&'a Positioned<SelectionSet>],
        ty: &'a str,
        depth: usize,
    ) -> Option<Unjoined<'a>> {
        let registry = self.registry;
        for concrete in possible(registry, ty) {
            let mut fields = Vec::new();
            for set in copies {
                self.collect(&set.node, concrete, depth, &mut fields);
            }
            for (key, group) in by_key(fields) {
                let first = group[0];
                let Some(declared) = declared(registry, concrete, &first.node.name.node) else {
                    // `__typename`, or a field validation refuses.
                    continue;
                };
                if let Some(other) = group[1..]
                    .iter()
                    .find(|copy| !same_field(&first.node, &copy.node))
                {
                    return Some(Unjoined {
                        key,
                        at: other.pos,
                        unlike: true,
                    });
                }
                if let Some(found) = self.unjoined_below(&group, declared, depth) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// What `group`, copies of one key the first of which names the field
    /// `declared`, answers otherwise than §6.3.2 merges it: the first copy selecting
    /// otherwise than the first where that field answers a list of lists, or else the
    /// first key below them [`Self::unjoined`] finds.
    fn unjoined_below(
        &mut self,
        group: &[&'a Positioned<Field>],
        declared: &'a MetaField,
        depth: usize,
    ) -> Option<Unjoined<'a>> {
        let (&first, others) = group.split_first()?;
        let otherwise = others
            .iter()
            .find(|copy| !self.alike_fields(&first.node, &copy.node, depth));
        if nests_lists(&declared.ty)
            && let Some(otherwise) = otherwise
        {
            return Some(Unjoined {
                key: first.node.response_key().node.as_str(),
                at: otherwise.pos,
                unlike: false,
            });
        }
        let sets: Vec<_> = match otherwise {
            Some(_) => group
                .iter()
                .map(|copy| &copy.node.selection_set)
                .filter(|set| !set.node.items.is_empty())
                .collect(),
            None => vec![&first.node.selection_set],
        };
        if sets.iter().all(|set| set.node.items.is_empty()) {
            return None;
        }
        let below = MetaTypeName::concrete_typename(&declared.ty);
        self.unjoined(&sets, below, depth + 1)
    }

    /// The fields `set` selects of a value of type `concrete`, through its
    /// fragments, each condition applied as §6.3.2 applies it.
    fn collect(
        &mut self,
        set: &'a SelectionSet,
        concrete: &'a str,
        depth: usize,
        fields: &mut Vec<&'a Positioned<Field>>,
    ) {
        if depth > RECURSIVE_DEPTH {
            return;
        }
        let (registry, variables) = (self.registry, self.variables);
        for selection in running(set, variables) {
            match &selection.node {
                Selection::Field(field) => fields.push(field),
                Selection::InlineFragment(fragment) => {
                    let condition = fragment.node.type_condition.as_ref();
                    if condition.is_none_or(|condition| {
                        applies(registry, &condition.node.on.node, concrete)
                    }) {
                        self.collect(
                            &fragment.node.selection_set.node,
                            concrete,
                            depth + 1,
                            fields,
                        );
                    }
                }
                Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.as_str();
                    let selected = self.fragment_fields(name, concrete, depth + 1);
                    fields.extend(selected.iter().copied());
                }
            }
        }
    }

    /// What fragment `name` collects of a value of type `concrete`, read once: one
    /// still being read, a cycle validation refuses, collects nothing.
    fn fragment_fields(
        &mut self,
        name: &'a str,
        concrete: &'a str,
        depth: usize,
    ) -> &[&'a Positioned<Field>] {
        let read_as = (name, concrete);
        if !self.collected.contains_key(&read_as) {
            self.collected.insert(read_as, Vec::new());
            let mut fields = Vec::new();
            if let Some(fragment) = self.fragments.get(name)
                && applies(
                    self.registry,
                    &fragment.node.type_condition.node.on.node,
                    concrete,
                )
            {
                self.collect(
                    &fragment.node.selection_set.node,
                    concrete,
                    depth,
                    &mut fields,
                );
            }
            self.collected.insert(read_as, fields);
        }
        self.collected.get(&read_as).map_or(&[], Vec::as_slice)
    }

    /// Whether two copies of one response key answer alike wherever they stand: one
    /// field, read with one set of arguments and one directive list, `@skip` and
    /// `@include` aside, and what runs of each selection set alike, selection by
    /// selection.
    fn alike_fields(&self, a: &Field, b: &Field, depth: usize) -> bool {
        same_field(a, b) && self.alike_sets(&a.selection_set.node, &b.selection_set.node, depth + 1)
    }

    fn alike_sets(&self, a: &SelectionSet, b: &SelectionSet, depth: usize) -> bool {
        if depth > RECURSIVE_DEPTH {
            return false;
        }
        let (mut a, mut b) = (running(a, self.variables), running(b, self.variables));
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(a), Some(b)) if self.alike_selections(&a.node, &b.node, depth) => {}
                _ => return false,
            }
        }
    }

    fn alike_selections(&self, a: &Selection, b: &Selection, depth: usize) -> bool {
        match (a, b) {
            (Selection::Field(a), Selection::Field(b)) => {
                a.node.response_key().node == b.node.response_key().node
                    && self.alike_fields(&a.node, &b.node, depth)
            }
            (Selection::InlineFragment(a), Selection::InlineFragment(b)) => {
                let (a_on, b_on) = (&a.node.type_condition, &b.node.type_condition);
                a_on.as_ref().map(|condition| &condition.node.on.node)
                    == b_on.as_ref().map(|condition| &condition.node.on.node)
                    && alike_directives(&a.node.directives, &b.node.directives)
                    && self.alike_sets(
                        &a.node.selection_set.node,
                        &b.node.selection_set.node,
                        depth + 1,
                    )
            }
            (Selection::FragmentSpread(a), Selection::FragmentSpread(b)) => {
                a.node.fragment_name.node == b.node.fragment_name.node
                    && alike_directives(&a.node.directives, &b.node.directives)
            }
            _ => false,
        }
    }
}

/// `fields` by response key, in the order they are first selected, one field
/// selected through one fragment spread twice standing once.
fn by_key<'a>(fields: Vec<&'a Positioned<Field>>) -> IndexMap<&'a str, Vec<&'a Positioned<Field>>> {
    let mut seen = HashSet::with_capacity(fields.len());
    let mut keys: IndexMap<&'a str, Vec<&'a Positioned<Field>>> = IndexMap::new();
    for field in fields {
        if seen.insert(field.pos) {
            keys.entry(field.node.response_key().node.as_str())
                .or_default()
                .push(field);
        }
    }
    keys
}

/// One field a place collects: where it stands, by the index of each selection down
/// from the place's set through inline fragments, and the type it is selected on.
struct Occurrence {
    path: Vec<usize>,
    pos: Pos,
    key: Name,
    on: String,
}

/// How a place's copies of one key merge.
enum Merge {
    /// Into the first, where every copy runs for every value the place may hold.
    InPlace(Vec<Occurrence>),
    /// Into one field per possible type of the place, under a condition on it.
    PerType(Vec<(String, Vec<Occurrence>)>),
}

/// The rewrite of one request's document, with the positions it draws for the sets
/// it changes, which no document spells (line 0): a set is read by where it opens,
/// and a changed set is no longer the one the client wrote there.
struct Collection<'a> {
    registry: &'a Registry,
    variables: &'a Variables,
    drawn: usize,
}

impl Collection<'_> {
    /// A position no document spells.
    fn draw(&mut self) -> Pos {
        self.drawn += 1;
        Pos {
            line: 0,
            column: self.drawn,
        }
    }

    /// `document` with `operation`, selecting on `root`, collected; `None` when its
    /// spreads cannot be inlined: a fragment validation does not know, a cycle, or a
    /// nesting async-graphql refuses.
    fn rewrite(
        &mut self,
        document: &ExecutableDocument,
        operation: &OperationDefinition,
        root: &str,
    ) -> Result<Option<ExecutableDocument>, ServerError> {
        let mut inlined = HashSet::new();
        let Some(mut set) = inline(
            &document.fragments,
            &operation.selection_set,
            &mut Vec::new(),
            &mut inlined,
            0,
        ) else {
            return Ok(None);
        };
        self.merge(&mut set, root, operation.ty == OperationType::Mutation, 0)?;
        self.conditions(&mut set.node, root, &[(root, root)], 0);
        Ok(Some(with_operation(document, operation, set, &inlined)))
    }

    /// Merge, in `set`, of type `ty`, the copies of each key that async-graphql would
    /// answer otherwise than §6.3.2 merges them: every key at a mutation's `root`, and
    /// a key whose copies a list of lists stands at or below, selecting otherwise;
    /// then the same below each field.
    fn merge(
        &mut self,
        set: &mut Positioned<SelectionSet>,
        ty: &str,
        root: bool,
        depth: usize,
    ) -> Result<(), ServerError> {
        if depth > RECURSIVE_DEPTH {
            return Ok(());
        }
        let merges = self.merges(set, ty, root)?;
        if !merges.is_empty() {
            // Every path was read before anything moved: a merge leaves each selection
            // where it stood, and what a merge per type adds goes in last.
            let mut added = Vec::new();
            for merge in merges {
                match merge {
                    Merge::InPlace(copies) => self.merge_in_place(set, copies),
                    Merge::PerType(copies) => added.extend(self.merge_per_type(set, copies)),
                }
            }
            added.sort_by_key(|(after, _)| *after);
            while let Some(&(after, _)) = added.last() {
                let from = added.partition_point(|(index, _)| *index < after);
                let block: Vec<_> = added
                    .drain(from..)
                    .map(|(_, selection)| selection)
                    .collect();
                let at = (after + 1).min(set.node.items.len());
                set.node.items.splice(at..at, block);
            }
            prune(&mut set.node);
        }
        let mut below = Vec::new();
        fields_below(&set.node, ty, self.registry, &mut Vec::new(), &mut below);
        for (path, field_ty) in below {
            if let Some(field) = field_at(&mut set.node, &path) {
                self.merge(&mut field.selection_set, field_ty, false, depth + 1)?;
            }
        }
        Ok(())
    }

    /// What merges `set`, of type `ty`, owes, read before anything moves.
    fn merges(
        &self,
        set: &Positioned<SelectionSet>,
        ty: &str,
        root: bool,
    ) -> Result<Vec<Merge>, ServerError> {
        let registry = self.registry;
        let concretes = possible(registry, ty);
        let mut per_type: Vec<(&str, Vec<Occurrence>)> = Vec::with_capacity(concretes.len());
        for concrete in &concretes {
            let mut found = Vec::new();
            occurrences(
                &set.node,
                concrete,
                ty,
                registry,
                self.variables,
                &mut Vec::new(),
                &mut found,
            );
            per_type.push((concrete, found));
        }
        let mut keys: IndexMap<Name, Vec<(&str, Vec<Occurrence>)>> = IndexMap::new();
        for (concrete, found) in per_type {
            for occurrence in found {
                let groups = keys.entry(occurrence.key.clone()).or_default();
                match groups.last_mut() {
                    Some((last, group)) if *last == concrete => group.push(occurrence),
                    _ => groups.push((concrete, vec![occurrence])),
                }
            }
        }
        let no_fragments = HashMap::new();
        let mut copies = Copies::new(&no_fragments, registry, self.variables);
        let mut merges = Vec::new();
        'keys: for (key, mut groups) in keys {
            let mut owed = false;
            for (concrete, group) in &groups {
                let fields: Vec<&Positioned<Field>> = group
                    .iter()
                    .filter_map(|occurrence| selection_field(&set.node, &occurrence.path))
                    .collect();
                if fields.len() != group.len()
                    || !fields.iter().zip(group).all(|(field, occurrence)| {
                        well_declared(registry, &occurrence.on, &field.node)
                    })
                {
                    // A copy validation refuses: the document is answered by it.
                    continue 'keys;
                }
                let distinct: Vec<&Positioned<Field>> = {
                    let mut seen = HashSet::new();
                    fields
                        .iter()
                        .copied()
                        .filter(|field| seen.insert(field.pos))
                        .collect()
                };
                let needed = match root {
                    true => group.len() > 1,
                    false => {
                        distinct.len() > 1
                            && declared(registry, concrete, &distinct[0].node.name.node)
                                .and_then(|declared| copies.unjoined_below(&distinct, declared, 0))
                                .is_some()
                    }
                };
                if needed {
                    if let Some(other) = distinct[1..]
                        .iter()
                        .find(|field| !same_field(&distinct[0].node, &field.node))
                    {
                        return Err(unlike(&key, other.pos));
                    }
                    owed = true;
                }
            }
            if !owed {
                continue;
            }
            let uniform = groups.len() == concretes.len()
                && groups.windows(2).all(|pair| {
                    let (a, b) = (&pair[0].1, &pair[1].1);
                    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.path == b.path)
                });
            merges.push(match uniform {
                true => Merge::InPlace(groups.swap_remove(0).1),
                false => Merge::PerType(
                    groups
                        .into_iter()
                        .map(|(concrete, group)| (concrete.to_owned(), group))
                        .collect(),
                ),
            });
        }
        Ok(merges)
    }

    /// Merge `copies`, which run for every value their place may hold, into the
    /// first: each other's selections appended to it, under a condition on its own
    /// type where that is not the first's, and the copy left as the `@skip` and
    /// `@include` it carried; a copy one fragment spread again repeats goes whole.
    fn merge_in_place(&mut self, set: &mut Positioned<SelectionSet>, copies: Vec<Occurrence>) {
        let registry = self.registry;
        let Some((first, others)) = copies.split_first() else {
            return;
        };
        let target_ty = selection_field(&set.node, &first.path)
            .and_then(|field| field_type(registry, &first.on, &field.node.name.node));
        let mut seen = HashSet::from([first.pos]);
        let mut moved = Vec::new();
        for other in others {
            let Some(taken) = take_field(&mut set.node, &other.path) else {
                continue;
            };
            if !seen.insert(other.pos) {
                continue;
            }
            let source_ty = field_type(registry, &other.on, &taken.node.name.node);
            moved.extend(self.carried(taken, source_ty, target_ty));
        }
        let drawn = self.draw();
        if let Some(target) = field_at(&mut set.node, &first.path) {
            target.selection_set.node.items.extend(moved);
            target.selection_set.pos = drawn;
        }
    }

    /// Merge each possible type's copies into one field under a condition on that
    /// type, every copy left as the `@skip` and `@include` it carried; what it
    /// returns goes after the selection holding the type's first copy, by its index.
    fn merge_per_type(
        &mut self,
        set: &mut Positioned<SelectionSet>,
        copies: Vec<(String, Vec<Occurrence>)>,
    ) -> Vec<(usize, Positioned<Selection>)> {
        let registry = self.registry;
        let mut built = Vec::new();
        for (concrete, group) in &copies {
            let Some(first) = group.first() else {
                continue;
            };
            let Some(field) = selection_field(&set.node, &first.path).cloned() else {
                continue;
            };
            let target_ty = field_type(registry, concrete, &field.node.name.node);
            let mut items = Vec::new();
            let mut seen = HashSet::new();
            for occurrence in group {
                if !seen.insert(occurrence.pos) {
                    continue;
                }
                let Some(copy) = selection_field(&set.node, &occurrence.path).cloned() else {
                    continue;
                };
                let source_ty = field_type(registry, &occurrence.on, &copy.node.name.node);
                items.extend(self.carried(copy, source_ty, target_ty));
            }
            let mut merged = field;
            merged.node.selection_set = Positioned::new(SelectionSet { items }, self.draw());
            let at = merged.pos;
            let items = vec![Positioned::new(Selection::Field(merged), at)];
            let wrapped = Positioned::new(SelectionSet { items }, self.draw());
            built.push((
                first.path[0],
                inline_fragment(Some(concrete), Vec::new(), wrapped, at),
            ));
        }
        for (_, group) in &copies {
            for occurrence in group {
                take_field(&mut set.node, &occurrence.path);
            }
        }
        built
    }

    /// The selections of `copy`, to stand in a field of `target` type: as they are
    /// where `copy`'s own type is the same, under a condition on it otherwise.
    fn carried(
        &mut self,
        copy: Positioned<Field>,
        source: Option<&str>,
        target: Option<&str>,
    ) -> Vec<Positioned<Selection>> {
        let set = copy.node.selection_set;
        match source {
            Some(source) if Some(source) != target && !set.node.items.is_empty() => {
                let at = set.pos;
                vec![inline_fragment(Some(source), Vec::new(), set, at)]
            }
            _ => set.node.items,
        }
    }

    /// Rewrite, in `set`, selected on `parent`, each condition on a union that no
    /// engine rule applies to the values `reaching` where some of them are members of
    /// it: under no condition where each is a member, so the set's own type resolves
    /// it, and under a condition on each member otherwise; what applies to none stays
    /// under the union's condition, which runs nothing there. Then the same below.
    fn conditions(
        &mut self,
        set: &mut SelectionSet,
        parent: &str,
        reaching: &[(&str, &str)],
        depth: usize,
    ) {
        if depth > RECURSIVE_DEPTH {
            return;
        }
        let registry = self.registry;
        let mut index = 0;
        while index < set.items.len() {
            if is_skipped(set.items[index].node.directives(), self.variables) {
                index += 1;
                continue;
            }
            let rewritten = match &set.items[index].node {
                Selection::InlineFragment(fragment) => fragment
                    .node
                    .type_condition
                    .as_ref()
                    .map(|condition| condition.node.on.node.to_string())
                    .filter(|condition| diverges(registry, condition, reaching))
                    .map(|condition| {
                        self.for_members(&set.items[index], &condition, parent, reaching)
                    }),
                _ => None,
            };
            if let Some(replacement) = rewritten {
                let added = replacement.len();
                set.items.splice(index..=index, replacement);
                // The rewritten selections apply as §6.3.2 reads, and are read below.
                for at in index..index + added {
                    self.conditions_of(&mut set.items[at], parent, reaching, depth);
                }
                index += added;
                continue;
            }
            self.conditions_of(&mut set.items[index], parent, reaching, depth);
            index += 1;
        }
    }

    /// The conditions below `selection`, selected on `parent` for the values
    /// `reaching`.
    fn conditions_of(
        &mut self,
        selection: &mut Positioned<Selection>,
        parent: &str,
        reaching: &[(&str, &str)],
        depth: usize,
    ) {
        let registry = self.registry;
        match &mut selection.node {
            Selection::Field(field) => {
                if field.node.selection_set.node.items.is_empty() {
                    return;
                }
                let Some(ty) = field_type(registry, parent, &field.node.name.node) else {
                    return;
                };
                let below: Vec<(&str, &str)> = possible(registry, ty)
                    .into_iter()
                    .map(|concrete| (concrete, ty))
                    .collect();
                self.conditions(&mut field.node.selection_set.node, ty, &below, depth + 1);
            }
            Selection::InlineFragment(fragment) => match &fragment.node.type_condition {
                Some(condition) => {
                    let condition = condition.node.on.node.to_string();
                    let below: Vec<(&str, &str)> = reaching
                        .iter()
                        .filter_map(|&(concrete, resolving)| {
                            engine_applies(registry, &condition, concrete, resolving)
                                .map(|resolving| (concrete, resolving))
                        })
                        .collect();
                    if !below.is_empty() {
                        // `below` borrows the registry's names, never the condition.
                        let condition = registry
                            .types
                            .get_key_value(condition.as_str())
                            .map_or("", |(name, _)| name.as_str());
                        self.conditions(
                            &mut fragment.node.selection_set.node,
                            condition,
                            &below,
                            depth + 1,
                        );
                    }
                }
                None => self.conditions(
                    &mut fragment.node.selection_set.node,
                    parent,
                    reaching,
                    depth + 1,
                ),
            },
            Selection::FragmentSpread(_) => {}
        }
    }

    /// What stands for the condition on the union `union` of `selection`, at a set
    /// selected on `parent` for the values `reaching`.
    fn for_members(
        &mut self,
        selection: &Positioned<Selection>,
        union: &str,
        parent: &str,
        reaching: &[(&str, &str)],
    ) -> Vec<Positioned<Selection>> {
        let registry = self.registry;
        let Selection::InlineFragment(fragment) = &selection.node else {
            return vec![selection.clone()];
        };
        let at = selection.pos;
        let directives = &fragment.node.directives;
        let set = &fragment.node.selection_set;
        let members: Vec<&str> = reaching
            .iter()
            .map(|&(concrete, _)| concrete)
            .filter(|concrete| applies(registry, union, concrete))
            .collect();
        let every = members.len() == reaching.len();
        let mut rewritten = Vec::new();
        let live_sets: Vec<(Option<&str>, Vec<&str>)> = match every {
            true => vec![(None, possible(registry, parent))],
            false => members
                .iter()
                .map(|member| (Some(*member), vec![*member]))
                .collect(),
        };
        for (condition, within) in &live_sets {
            let live = kept(&set.node, &|on| overlaps(registry, on, within));
            rewritten.push(inline_fragment(
                *condition,
                directives.clone(),
                Positioned::new(live, set.pos),
                at,
            ));
        }
        let within: Vec<&str> = live_sets
            .iter()
            .flat_map(|(_, within)| within.iter().copied())
            .collect();
        let residue = left(&set.node, &|on| overlaps(registry, on, &within));
        if !residue.items.is_empty() {
            let mut kept_under_union = fragment.clone();
            kept_under_union.node.selection_set = Positioned::new(residue, set.pos);
            rewritten.push(Positioned::new(
                Selection::InlineFragment(kept_under_union),
                at,
            ));
        }
        rewritten
    }
}

/// What of `set`, the selections of a condition on a union, stands where `live` says
/// of a condition's type: `__typename`, the conditions on a type `live` admits, and
/// what a selection with no condition holds of them, its directives kept.
fn kept(set: &SelectionSet, live: &dyn Fn(&str) -> bool) -> SelectionSet {
    let mut items = Vec::new();
    for selection in &set.items {
        match &selection.node {
            Selection::Field(field) if field.node.name.node == "__typename" => {
                items.push(selection.clone());
            }
            Selection::Field(_) | Selection::FragmentSpread(_) => {}
            Selection::InlineFragment(fragment) => match &fragment.node.type_condition {
                Some(condition) => {
                    if live(&condition.node.on.node) {
                        items.push(selection.clone());
                    }
                }
                None => {
                    let below = kept(&fragment.node.selection_set.node, live);
                    if !below.items.is_empty() || !fragment.node.directives.is_empty() {
                        items.push(with_set(selection, fragment, below));
                    }
                }
            },
        }
    }
    SelectionSet { items }
}

/// What of `set` [`kept`] leaves nowhere `live` admits: every field but
/// `__typename`, which a union declares none of, and the conditions on a type it
/// does not admit, each where it was written.
fn left(set: &SelectionSet, live: &dyn Fn(&str) -> bool) -> SelectionSet {
    let mut items = Vec::new();
    for selection in &set.items {
        match &selection.node {
            Selection::Field(field) if field.node.name.node == "__typename" => {}
            Selection::Field(_) | Selection::FragmentSpread(_) => items.push(selection.clone()),
            Selection::InlineFragment(fragment) => match &fragment.node.type_condition {
                Some(condition) => {
                    if !live(&condition.node.on.node) {
                        items.push(selection.clone());
                    }
                }
                None => {
                    let below = left(&fragment.node.selection_set.node, live);
                    if !below.items.is_empty() {
                        items.push(with_set(selection, fragment, below));
                    }
                }
            },
        }
    }
    SelectionSet { items }
}

/// `selection`, the inline fragment `fragment`, holding `items` instead.
fn with_set(
    selection: &Positioned<Selection>,
    fragment: &Positioned<InlineFragment>,
    items: SelectionSet,
) -> Positioned<Selection> {
    let mut rewritten = fragment.clone();
    rewritten.node.selection_set = Positioned::new(items, fragment.node.selection_set.pos);
    Positioned::new(Selection::InlineFragment(rewritten), selection.pos)
}

/// An inline fragment at `at` holding `set`, carrying `directives`, under a
/// condition on `on` when one is named.
fn inline_fragment(
    on: Option<&str>,
    directives: Vec<Positioned<Directive>>,
    set: Positioned<SelectionSet>,
    at: Pos,
) -> Positioned<Selection> {
    let type_condition = on.map(|on| {
        Positioned::new(
            TypeCondition {
                on: Positioned::new(Name::new(on), at),
            },
            at,
        )
    });
    let fragment = InlineFragment {
        type_condition,
        directives,
        selection_set: set,
    };
    Positioned::new(Selection::InlineFragment(Positioned::new(fragment, at)), at)
}

/// Whether `field`, selected on `on`, is one the schema declares there with every
/// argument it passes, selecting below it exactly where its type is an object, an
/// interface or a union: a copy the merge may stand for, since what validation
/// would refuse of it leaves with it.
fn well_declared(registry: &Registry, on: &str, field: &Field) -> bool {
    declared(registry, on, &field.name.node).is_some_and(|declared| {
        let composite = registry
            .types
            .get(MetaTypeName::concrete_typename(&declared.ty))
            .is_some_and(MetaType::is_composite);
        composite != field.selection_set.node.items.is_empty()
            && field
                .arguments
                .iter()
                .all(|(name, _)| declared.args.contains_key(name.node.as_str()))
    })
}

/// Each field `set`, selected on `on`, holds for a value of `concrete` that runs,
/// through its inline fragments as §6.3.2 applies them, but `__typename`.
fn occurrences(
    set: &SelectionSet,
    concrete: &str,
    on: &str,
    registry: &Registry,
    variables: &Variables,
    path: &mut Vec<usize>,
    found: &mut Vec<Occurrence>,
) {
    if path.len() > RECURSIVE_DEPTH {
        return;
    }
    for (index, selection) in set.items.iter().enumerate() {
        if is_skipped(selection.node.directives(), variables) {
            continue;
        }
        path.push(index);
        match &selection.node {
            Selection::Field(field) if field.node.name.node != "__typename" => {
                found.push(Occurrence {
                    path: path.clone(),
                    pos: field.pos,
                    key: field.node.response_key().node.clone(),
                    on: on.to_owned(),
                });
            }
            Selection::Field(_) | Selection::FragmentSpread(_) => {}
            Selection::InlineFragment(fragment) => match &fragment.node.type_condition {
                Some(condition) => {
                    let condition = condition.node.on.node.as_str();
                    if applies(registry, condition, concrete) {
                        occurrences(
                            &fragment.node.selection_set.node,
                            concrete,
                            condition,
                            registry,
                            variables,
                            path,
                            found,
                        );
                    }
                }
                None => occurrences(
                    &fragment.node.selection_set.node,
                    concrete,
                    on,
                    registry,
                    variables,
                    path,
                    found,
                ),
            },
        }
        path.pop();
    }
}

/// Each field `set`, selected on `on`, holds through its inline fragments with a
/// selection set of its own, by its path, with the type it declares.
fn fields_below<'r>(
    set: &SelectionSet,
    on: &str,
    registry: &'r Registry,
    path: &mut Vec<usize>,
    below: &mut Vec<(Vec<usize>, &'r str)>,
) {
    if path.len() > RECURSIVE_DEPTH {
        return;
    }
    for (index, selection) in set.items.iter().enumerate() {
        path.push(index);
        match &selection.node {
            Selection::Field(field) if !field.node.selection_set.node.items.is_empty() => {
                if let Some(ty) = field_type(registry, on, &field.node.name.node) {
                    below.push((path.clone(), ty));
                }
            }
            Selection::Field(_) | Selection::FragmentSpread(_) => {}
            Selection::InlineFragment(fragment) => {
                let on = fragment
                    .node
                    .type_condition
                    .as_ref()
                    .map_or(on, |condition| condition.node.on.node.as_str());
                fields_below(&fragment.node.selection_set.node, on, registry, path, below);
            }
        }
        path.pop();
    }
}

/// The field at `path` in `set`, through its inline fragments.
fn selection_field<'s>(set: &'s SelectionSet, path: &[usize]) -> Option<&'s Positioned<Field>> {
    let (index, rest) = path.split_first()?;
    match (&set.items.get(*index)?.node, rest) {
        (Selection::Field(field), []) => Some(field),
        (Selection::InlineFragment(fragment), _) => {
            selection_field(&fragment.node.selection_set.node, rest)
        }
        _ => None,
    }
}

/// The field at `path` in `set`, to change.
fn field_at<'s>(set: &'s mut SelectionSet, path: &[usize]) -> Option<&'s mut Field> {
    let (index, rest) = path.split_first()?;
    match (&mut set.items.get_mut(*index)?.node, rest) {
        (Selection::Field(field), []) => Some(&mut field.node),
        (Selection::InlineFragment(fragment), _) => {
            field_at(&mut fragment.node.selection_set.node, rest)
        }
        _ => None,
    }
}

/// The field at `path` in `set`, taken out: an inline fragment holding nothing
/// stands in its place, carrying the `@skip` and `@include` the field carried,
/// which the request's variables may be read in.
fn take_field(set: &mut SelectionSet, path: &[usize]) -> Option<Positioned<Field>> {
    let (index, rest) = path.split_first()?;
    let selection = set.items.get_mut(*index)?;
    match (&mut selection.node, rest) {
        (Selection::Field(field), []) => {
            let at = selection.pos;
            let conditions = field
                .node
                .directives
                .iter()
                .filter(|directive| is_condition(directive))
                .cloned()
                .collect();
            let left = Selection::InlineFragment(Positioned::new(
                InlineFragment {
                    type_condition: None,
                    directives: conditions,
                    selection_set: Positioned::new(SelectionSet::default(), at),
                },
                at,
            ));
            match std::mem::replace(&mut selection.node, left) {
                Selection::Field(field) => Some(field),
                _ => None,
            }
        }
        (Selection::InlineFragment(fragment), _) => {
            take_field(&mut fragment.node.selection_set.node, rest)
        }
        _ => None,
    }
}

/// `set` without the inline fragments that hold nothing, under no condition and no
/// directive: what a merge left, which nothing reads.
fn prune(set: &mut SelectionSet) {
    set.items.retain_mut(|selection| match &mut selection.node {
        Selection::InlineFragment(fragment) => {
            prune(&mut fragment.node.selection_set.node);
            !(fragment.node.selection_set.node.items.is_empty()
                && fragment.node.type_condition.is_none()
                && fragment.node.directives.is_empty())
        }
        Selection::Field(_) | Selection::FragmentSpread(_) => true,
    });
}

/// `set`, each fragment spread in it inlined under the type condition its fragment
/// names and the directives the spread carries, `inlined` naming each; `None` for a
/// spread of a fragment the document does not define, a cycle, or a nesting past
/// [`RECURSIVE_DEPTH`], which async-graphql refuses before validation.
fn inline<'d>(
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    set: &Positioned<SelectionSet>,
    expanding: &mut Vec<&'d str>,
    inlined: &mut HashSet<&'d str>,
    depth: usize,
) -> Option<Positioned<SelectionSet>> {
    if depth > RECURSIVE_DEPTH {
        return None;
    }
    let mut items = Vec::with_capacity(set.node.items.len());
    for selection in &set.node.items {
        let rewritten = match &selection.node {
            Selection::Field(field) => {
                let mut field = field.clone();
                if !field.node.selection_set.node.items.is_empty() {
                    field.node.selection_set = inline(
                        fragments,
                        &field.node.selection_set,
                        expanding,
                        inlined,
                        depth + 1,
                    )?;
                }
                Selection::Field(field)
            }
            Selection::InlineFragment(fragment) => {
                let mut fragment = fragment.clone();
                fragment.node.selection_set = inline(
                    fragments,
                    &fragment.node.selection_set,
                    expanding,
                    inlined,
                    depth + 1,
                )?;
                Selection::InlineFragment(fragment)
            }
            Selection::FragmentSpread(spread) => {
                let (name, definition) =
                    fragments.get_key_value(spread.node.fragment_name.node.as_str())?;
                let name = name.as_str();
                if expanding.contains(&name) {
                    return None;
                }
                expanding.push(name);
                let selections = inline(
                    fragments,
                    &definition.node.selection_set,
                    expanding,
                    inlined,
                    depth + 1,
                );
                expanding.pop();
                inlined.insert(name);
                Selection::InlineFragment(Positioned::new(
                    InlineFragment {
                        type_condition: Some(definition.node.type_condition.clone()),
                        directives: spread.node.directives.clone(),
                        selection_set: selections?,
                    },
                    spread.pos,
                ))
            }
        };
        items.push(Positioned::new(rewritten, selection.pos));
    }
    Some(Positioned::new(SelectionSet { items }, set.pos))
}

/// `document` with `operation`'s selections `set`, and without the fragments it
/// `inlined` no other operation spreads: what validation still reads of one is what
/// each spread inlined, under its condition. A fragment carrying a directive stays,
/// for validation to read it.
fn with_operation(
    document: &ExecutableDocument,
    operation: &OperationDefinition,
    set: Positioned<SelectionSet>,
    inlined: &HashSet<&str>,
) -> ExecutableDocument {
    let at = operation.selection_set.pos;
    let mut operations = document.operations.clone();
    let mut others = Vec::new();
    match &mut operations {
        DocumentOperations::Single(single) => single.node.selection_set = set,
        DocumentOperations::Multiple(all) => {
            let mut set = Some(set);
            for candidate in all.values_mut() {
                if candidate.node.selection_set.pos == at {
                    if let Some(set) = set.take() {
                        candidate.node.selection_set = set;
                    }
                } else {
                    others.push(&candidate.node.selection_set);
                }
            }
        }
    }
    let mut spread_elsewhere = HashSet::new();
    for set in others {
        spreads_in(&document.fragments, &set.node, &mut spread_elsewhere, 0);
    }
    let fragments = document
        .fragments
        .iter()
        .filter(|(name, fragment)| {
            !inlined.contains(name.as_str())
                || spread_elsewhere.contains(name.as_str())
                || !fragment.node.directives.is_empty()
        })
        .map(|(name, fragment)| (name.clone(), fragment.clone()))
        .collect();
    ExecutableDocument {
        operations,
        fragments,
    }
}

/// Each fragment `set` spreads, through the fragments it spreads.
fn spreads_in<'d>(
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    set: &'d SelectionSet,
    spread: &mut HashSet<&'d str>,
    depth: usize,
) {
    if depth > RECURSIVE_DEPTH {
        return;
    }
    for selection in &set.items {
        match &selection.node {
            Selection::Field(field) => {
                spreads_in(fragments, &field.node.selection_set.node, spread, depth + 1);
            }
            Selection::InlineFragment(fragment) => {
                spreads_in(
                    fragments,
                    &fragment.node.selection_set.node,
                    spread,
                    depth + 1,
                );
            }
            Selection::FragmentSpread(name) => {
                if let Some((name, fragment)) =
                    fragments.get_key_value(name.node.fragment_name.node.as_str())
                    && spread.insert(name.as_str())
                {
                    spreads_in(
                        fragments,
                        &fragment.node.selection_set.node,
                        spread,
                        depth + 1,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_graphql::parser::parse_query;
    use async_graphql::{
        EmptySubscription, Interface, Object, OutputType, Schema, SimpleObject, Union,
    };
    use serde_json::json;

    use super::*;
    use crate::operation_units::selected_operation;
    use crate::testing::thread_cpu_time;

    #[derive(SimpleObject)]
    struct Fine {
        fine: i32,
        inner: Option<Box<Fine>>,
        grid: Vec<Vec<Fine>>,
    }

    #[derive(SimpleObject)]
    struct Other {
        other: i32,
    }

    #[derive(SimpleObject)]
    struct Third {
        fine: i32,
        grid: Vec<Vec<Fine>>,
    }

    #[derive(Union)]
    enum Either {
        Fine(Fine),
        Other(Other),
    }

    #[derive(Interface)]
    #[graphql(
        field(name = "fine", ty = "&i32"),
        field(name = "grid", ty = "&Vec<Vec<Fine>>")
    )]
    enum Node {
        Fine(Fine),
        Third(Third),
    }

    fn fine() -> Fine {
        Fine {
            fine: 1,
            inner: None,
            grid: vec![vec![Fine {
                fine: 2,
                inner: None,
                grid: Vec::new(),
            }]],
        }
    }

    struct Query;

    #[Object]
    impl Query {
        async fn fine(&self) -> Fine {
            fine()
        }

        async fn grid(&self) -> Vec<Vec<Fine>> {
            vec![vec![fine(), fine()], vec![fine()]]
        }

        async fn either(&self) -> Option<Either> {
            Some(Either::Fine(fine()))
        }

        async fn node(&self) -> Option<Node> {
            Some(Node::Fine(fine()))
        }

        async fn matrix(&self, n: i32) -> Vec<Vec<i32>> {
            vec![vec![n]]
        }
    }

    /// How many writes [`Mutation`] ran, in this test's process.
    static WRITES: AtomicUsize = AtomicUsize::new(0);

    struct Mutation;

    #[Object]
    impl Mutation {
        async fn write(&self, x: Option<i32>) -> i32 {
            WRITES.fetch_add(1, Ordering::SeqCst);
            x.unwrap_or_default()
        }

        async fn shaped(&self) -> Fine {
            fine()
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::default();
        let root = Query::create_type_info(&mut registry);
        registry.query_type = MetaTypeName::concrete_typename(&root).to_owned();
        let root = Mutation::create_type_info(&mut registry);
        registry.mutation_type = Some(MetaTypeName::concrete_typename(&root).to_owned());
        registry
    }

    /// What `text`, the operation `name` selected and `variables` sent, is
    /// collected as: its selections and the fragments it keeps, as text; `None`
    /// when it stands as sent.
    fn collected_as(
        text: &str,
        name: Option<&str>,
        variables: serde_json::Value,
    ) -> Result<Option<String>, String> {
        let document = parse_query(text).expect("the document parses");
        let (_, operation) = selected_operation(&document, name).expect("an operation");
        let variables = Variables::from_json(variables);
        let rewritten = collected(&document, &operation.node, &registry(), &variables)
            .map_err(|refusal| refusal.message)?;
        Ok(rewritten.map(|rewritten| {
            let chosen = chosen(&rewritten, &operation.node).expect("the operation");
            let mut text = String::new();
            write_set(&mut text, &chosen.selection_set.node);
            let mut kept: Vec<&str> = rewritten.fragments.keys().map(Name::as_str).collect();
            kept.sort_unstable();
            for name in kept {
                text.push_str(&format!(" fragment {name}"));
            }
            text
        }))
    }

    fn rewritten(text: &str) -> Option<String> {
        collected_as(text, None, json!({})).unwrap_or_else(|refusal| panic!("{text}: {refusal}"))
    }

    fn write_set(text: &mut String, set: &SelectionSet) {
        text.push('{');
        for selection in &set.items {
            text.push(' ');
            match &selection.node {
                Selection::Field(field) => {
                    if let Some(alias) = &field.node.alias {
                        text.push_str(&format!("{}: ", alias.node));
                    }
                    text.push_str(&field.node.name.node);
                    if !field.node.arguments.is_empty() {
                        let arguments: Vec<String> = field
                            .node
                            .arguments
                            .iter()
                            .map(|(name, value)| format!("{}: {}", name.node, value.node))
                            .collect();
                        text.push_str(&format!("({})", arguments.join(", ")));
                    }
                    write_directives(text, &field.node.directives);
                    if !field.node.selection_set.node.items.is_empty() {
                        text.push(' ');
                        write_set(text, &field.node.selection_set.node);
                    }
                }
                Selection::InlineFragment(fragment) => {
                    text.push_str("...");
                    if let Some(condition) = &fragment.node.type_condition {
                        text.push_str(&format!(" on {}", condition.node.on.node));
                    }
                    write_directives(text, &fragment.node.directives);
                    text.push(' ');
                    write_set(text, &fragment.node.selection_set.node);
                }
                Selection::FragmentSpread(spread) => {
                    text.push_str(&format!("...{}", spread.node.fragment_name.node));
                }
            }
        }
        text.push_str(" }");
    }

    fn write_directives(text: &mut String, directives: &[Positioned<Directive>]) {
        for directive in directives {
            text.push_str(&format!(" @{}", directive.node.name.node));
            for (name, value) in &directive.node.arguments {
                text.push_str(&format!("({}: {})", name.node, value.node));
            }
        }
    }

    /// Pins why a mutation's root field selected twice merges: async-graphql runs it
    /// once per selection, where §6.3.2 runs one response key once. A bump that
    /// merges them fails here, and the rewrite of the root can go.
    #[tokio::test]
    async fn async_graphql_runs_a_root_field_once_per_selection() {
        let schema = Schema::new(Query, Mutation, EmptySubscription);
        let before = WRITES.load(Ordering::SeqCst);
        let response = schema.execute("mutation { a: write a: write }").await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(WRITES.load(Ordering::SeqCst) - before, 2);
    }

    /// Pins why a list of lists selected twice merges: async-graphql joins a later
    /// copy's items into the first only where they are objects (`insert_value`).
    #[tokio::test]
    async fn async_graphql_answers_one_copy_of_a_list_of_lists() {
        let schema = Schema::new(Query, Mutation, EmptySubscription);
        let response = schema
            .execute("{ grid { fine } grid { inner { fine } } }")
            .await;
        let data = response.data.into_json().expect("plain JSON");
        let cell = &data["grid"][0][0];
        assert!(
            cell.get("fine").is_some() != cell.get("inner").is_some(),
            "one copy's cells alone: {data}"
        );
    }

    /// Pins why a condition on a union is rewritten: async-graphql applies it only
    /// where that union is the type resolved (`Fields::add_set`).
    #[tokio::test]
    async fn async_graphql_applies_a_union_condition_where_the_union_is_resolved_alone() {
        let schema = Schema::new(Query, Mutation, EmptySubscription);
        let response = schema
            .execute("{ fine { ... on Either { ... on Fine { fine } } } }")
            .await;
        assert_eq!(
            response.data.into_json().expect("plain JSON"),
            json!({ "fine": null }),
            "no key, so `null`, where §6.3.2 answers `fine`"
        );
    }

    /// Each copy of a key at a mutation's root merges into the first, through
    /// fragments, one spread twice included, and a copy merged away leaves the
    /// `@skip` and `@include` it carried.
    #[test]
    fn a_mutations_root_selects_each_key_once() {
        for (document, said) in [
            ("mutation { a: write a: write }", "{ a: write }"),
            (
                "mutation { write ... on Mutation { write } }",
                "{ write ... on Mutation { } }",
            ),
            (
                "mutation { ...A ...A } fragment A on Mutation { write }",
                "{ ... on Mutation { write } ... on Mutation { } }",
            ),
            (
                "mutation { ...A ...B } fragment A on Mutation { w: write } \
                 fragment B on Mutation { w: write }",
                "{ ... on Mutation { w: write } ... on Mutation { } }",
            ),
            (
                "mutation { s: shaped { fine } s: shaped { inner { fine } } }",
                "{ s: shaped { fine inner { fine } } }",
            ),
            (
                "mutation { a: write(x: 1) a: write(x: 1) b: write }",
                "{ a: write(x: 1) b: write }",
            ),
            (
                "mutation { s: shaped { fine } s: shaped @include(if: true) { grid { fine } } }",
                "{ s: shaped { fine grid { fine } } ... @include(if: true) { } }",
            ),
        ] {
            assert_eq!(rewritten(document).as_deref(), Some(said), "{document}");
        }
    }

    /// A key selected once, a copy a directive leaves out, and a document naming each
    /// root field once stand as sent.
    #[test]
    fn a_document_selecting_each_key_once_stands_as_sent() {
        for (document, name) in [
            ("mutation { a: write b: write }", None),
            (
                "mutation { s: shaped { fine fine } t: shaped { fine } }",
                None,
            ),
            ("mutation { write @skip(if: true) write }", None),
            (
                "mutation { ...A @include(if: false) ...A } fragment A on Mutation { write }",
                None,
            ),
            ("{ a: fine { fine } a: fine { inner { fine } } }", None),
            ("query Q { fine { fine } } mutation M { write }", Some("M")),
            ("{ grid { fine } grid { fine } }", None),
            (
                "{ grid { fine } grid @skip(if: true) { inner { fine } } }",
                None,
            ),
            (
                "{ either { ... on Either { ... on Fine { fine } } } }",
                None,
            ),
        ] {
            assert_eq!(
                collected_as(document, name, json!({})),
                Ok(None),
                "{document}"
            );
        }
    }

    /// Copies validation refuses — a field the schema does not declare there, an
    /// argument it does not take, an object selecting nothing, a scalar selecting
    /// below it — are merged nowhere, since what validation says of a copy would
    /// leave with it: the document stands as sent, its root's repeat refused behind
    /// what validation says first.
    #[test]
    fn copies_validation_refuses_are_merged_nowhere() {
        for (document, key) in [
            ("mutation { s: shaped { fine } s: shaped }", "s"),
            ("mutation { a: write { fine } a: write }", "a"),
            ("mutation { a: nowhere a: nowhere }", "a"),
            ("mutation { a: write(y: 1) a: write(y: 1) }", "a"),
        ] {
            assert_eq!(
                collected_as(document, None, json!({})),
                Err(format!(
                    r#"the mutation selects "{key}" more than once at its root"#
                )),
                "{document}"
            );
        }
        assert_eq!(
            rewritten("{ grid { fine } grid { nowhere } }").as_deref(),
            Some("{ grid { fine nowhere } }"),
            "what a copy selects moves, and is judged where it lands"
        );
    }

    /// Copies naming another field or other arguments are no one field: refused at
    /// the first that differs, at a mutation's root and where a list of lists stands.
    #[test]
    fn copies_naming_different_fields_or_arguments_are_refused() {
        let refusal = |key: &str| {
            format!(
                r#"the operation selects "{key}" more than once at one place with different fields or arguments"#
            )
        };
        for (document, key) in [
            (
                "mutation { a: write(x: 1) ... on Mutation { a: write(x: 2) } }",
                "a",
            ),
            (
                "mutation { a: write ... on Mutation { a: shaped { fine } } }",
                "a",
            ),
            ("{ m: matrix(n: 1) ... on Query { m: matrix(n: 2) } }", "m"),
            (
                "{ g: grid { fine } ... on Query { g: fine { fine } } }",
                "g",
            ),
        ] {
            assert_eq!(
                collected_as(document, None, json!({})),
                Err(refusal(key)),
                "{document}"
            );
        }
        let document = parse_query(
            "mutation {\n  a: write(x: 1)\n  ... on Mutation {\n    a: write(x: 2)\n  }\n}",
        )
        .expect("parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let refused = collected(
            &document,
            &operation.node,
            &registry(),
            &Variables::default(),
        )
        .expect_err("refused");
        assert_eq!(refused.locations, [Pos { line: 4, column: 5 }]);
    }

    /// A list of lists selected twice otherwise merges, at its place and below copies
    /// of its parent, and what is merged reads below.
    #[test]
    fn a_list_of_lists_selected_twice_otherwise_merges() {
        for (document, said) in [
            (
                "{ grid { fine } grid { inner { fine } } }",
                "{ grid { fine inner { fine } } }",
            ),
            (
                "{ fine { grid { fine } } fine { grid { inner { fine } } } }",
                "{ fine { grid { fine inner { fine } } } }",
            ),
            (
                "{ grid { fine } ... { grid { inner { fine } } } }",
                "{ grid { fine inner { fine } } }",
            ),
            (
                "{ ...A ...B } fragment A on Query { grid { fine } } \
                 fragment B on Query { grid { inner { fine } } }",
                "{ ... on Query { grid { fine inner { fine } } } ... on Query { } }",
            ),
            (
                "{ grid { grid { fine } } grid { grid { inner { fine } } } }",
                "{ grid { grid { fine inner { fine } } } }",
            ),
        ] {
            assert_eq!(rewritten(document).as_deref(), Some(said), "{document}");
        }
    }

    /// Where the copies run for some of the values a position may hold, each type's
    /// copies merge into a field of its own, under a condition on that type.
    #[test]
    fn copies_running_for_some_types_merge_per_type() {
        assert_eq!(
            rewritten("{ node { grid { fine } ... on Fine { grid { inner { fine } } } } }")
                .as_deref(),
            Some(
                "{ node { ... on Fine { grid { fine inner { fine } } } \
                 ... on Third { grid { fine } } ... on Fine { } } }"
            )
        );
    }

    /// A condition on a union at a member's position stands under no condition where
    /// every value it may hold is a member, and under one per member otherwise; what
    /// applies to none stays under the union's.
    #[test]
    fn a_condition_on_a_union_applies_where_a_member_is_resolved() {
        for (document, said) in [
            (
                "{ fine { ... on Either { ... on Fine { fine } } } }",
                "{ fine { ... { ... on Fine { fine } } } }",
            ),
            (
                "{ fine { ... on Either { __typename ... on Fine { fine } ... on Other { other } } } }",
                "{ fine { ... { __typename ... on Fine { fine } } ... on Either { ... on Other { other } } } }",
            ),
            (
                "{ fine { ... on Either { fine } } }",
                "{ fine { ... { } ... on Either { fine } } }",
            ),
            (
                "{ node { ... on Either { __typename ... on Other { other } } } }",
                "{ node { ... on Fine { __typename } ... on Either { ... on Other { other } } } }",
            ),
            (
                "{ either { ... on Fine { ... on Either { ... on Fine { fine } } } } }",
                "{ either { ... on Fine { ... { ... on Fine { fine } } } } }",
            ),
            (
                "query($x: Boolean!) { fine { ... on Either @include(if: $x) { ... on Other { other } } } }",
                "{ fine { ... @include(if: $x) { } ... on Either @include(if: $x) { ... on Other { other } } } }",
            ),
        ] {
            let collected = collected_as(document, None, json!({ "x": true }))
                .unwrap_or_else(|refusal| panic!("{document}: {refusal}"));
            assert_eq!(collected.as_deref(), Some(said), "{document}");
        }
    }

    /// A fragment the operation alone spreads leaves the document once inlined; one
    /// another operation spreads stays, and so does one carrying a directive, for
    /// validation to read.
    #[test]
    fn a_fragment_leaves_the_document_once_nothing_spreads_it() {
        assert_eq!(
            rewritten("{ fine { ...E } } fragment E on Either { ... on Fine { fine } }").as_deref(),
            Some("{ fine { ... { ... on Fine { fine } } } }")
        );
        assert_eq!(
            collected_as(
                "query A { fine { ...E } } query B { either { ...E } } \
                 fragment E on Either { ... on Fine { fine } }",
                Some("A"),
                json!({})
            ),
            Ok(Some(
                "{ fine { ... { ... on Fine { fine } } } } fragment E".to_owned()
            ))
        );
        assert_eq!(
            rewritten(
                "{ fine { ...E } } fragment E on Either @skip(if: true) { ... on Fine { fine } }"
            )
            .as_deref(),
            Some("{ fine { ... { ... on Fine { fine } } } } fragment E")
        );
    }

    /// A document whose spreads cannot be inlined is validation's to refuse: it
    /// stands as sent, or is refused as the read behind the rewrite refuses it.
    #[test]
    fn a_document_that_cannot_be_inlined_stands_as_sent() {
        assert_eq!(
            rewritten("{ fine { ...E } } fragment E on Either { ... on Fine { ...E } }"),
            None
        );
        assert_eq!(
            collected_as(
                "mutation { ...A ...A } fragment A on Mutation { ...A write }",
                None,
                json!({})
            ),
            Err(r#"the mutation spreads "A" more than once at its root"#.to_owned())
        );
        assert_eq!(
            rewritten("{ fine { ...Missing } ... on Query { fine { ...Missing } } }"),
            None
        );
    }

    /// A document naming no union in a condition and no key twice in one set reads no
    /// type: the schema below declares none of the fields it selects.
    #[test]
    fn a_document_naming_no_union_and_no_key_twice_reads_no_type() {
        let document = parse_query(
            "{ unknown { ...F a: other { ...F } } } fragment F on Unknown { field ... on X { b } }",
        )
        .expect("parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let registry = Registry::default();
        assert!(matches!(
            collected(&document, &operation.node, &registry, &Variables::default()),
            Ok(None)
        ));
        let glance = Glance::of(
            &document.fragments,
            &registry,
            &operation.node.selection_set.node,
        );
        assert!(!glance.unions && !glance.twice);
        let document =
            parse_query("{ a { ...F } } fragment F on A { b ... { b } }").expect("parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let glance = Glance::of(
            &document.fragments,
            &registry,
            &operation.node.selection_set.node,
        );
        assert!(glance.twice, "a key twice through a fragment");
    }

    /// One key repeated across a wide document is read once per copy: the copies
    /// compared to the first, and what stands below them read once.
    #[test]
    fn a_wide_repeat_costs_no_square_of_its_width() {
        let copies: String = (0..20_000).map(|_| "grid { fine } ").collect();
        let text = format!("{{ {copies}}}");
        let started = thread_cpu_time();
        let document = parse_query(&text).expect("the document parses");
        let parsing = thread_cpu_time() - started;
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let (registry, variables) = (registry(), Variables::default());
        let started = thread_cpu_time();
        let collected = collected(&document, &operation.node, &registry, &variables);
        let past = thread_cpu_time() - started;
        assert!(matches!(collected, Ok(None)), "every copy alike");
        assert!(past < parsing, "parsed in {parsing:?}, then {past:?}");
    }

    /// A wide document merged is rewritten in time linear in its width.
    #[test]
    fn a_wide_merge_costs_no_square_of_its_width() {
        let copies: String = (0..10_000)
            .map(|_| "grid { fine } grid { inner { fine } } ")
            .collect();
        let text = format!("{{ {copies}}}");
        let started = thread_cpu_time();
        let document = parse_query(&text).expect("the document parses");
        let parsing = thread_cpu_time() - started;
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let (registry, variables) = (registry(), Variables::default());
        let started = thread_cpu_time();
        let collected = collected(&document, &operation.node, &registry, &variables);
        let past = thread_cpu_time() - started;
        assert!(matches!(collected, Ok(Some(_))), "the copies merge");
        assert!(past < parsing * 4, "parsed in {parsing:?}, then {past:?}");
    }
}
