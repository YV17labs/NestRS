//! An object that could answer no key, given one of its own.
//!
//! async-graphql (`7.2.1`, `do_resolve_container`) answers `null` for an object
//! with no key to answer, where GraphQL §6.3 answers `{}`, and resolves each
//! selection of one response key apart, keeping whichever finished first where one
//! answered `null` (`insert_value`): an object its directives leave nothing to
//! answer, one no type condition applies to, or a copy whose every field erred,
//! answered `null` and hid what another selection of it answered, with no error.
//!
//! So each selection set that could answer no key is given `__typename` under an
//! alias drawn for the request ([`OwnKey`]), once the document's cost was counted
//! and before validation, which judges nothing new by it: `__typename` is valid on
//! every object, interface and union, scores nothing and nests nothing deeper. The
//! alias leaves the answer before anything reads it. A set that could
//! answer no key is one every selection of which a directive leaves out, one no
//! type condition applies to some type its value may take, and each copy of a
//! response key selected more than once at one place, with what stands below it,
//! since any of them may err whole. The schema's types are read only for a
//! document holding a fragment, a directive or an object selected twice at one
//! place: without one, every set selects a field, and nothing is given.
//!
//! What no key joins is refused: `insert_value` joins a later copy into the first
//! only where both are objects, or lists of objects, so a list nested in a list keeps
//! the first copy to finish whole. A response key a value of some type selects more
//! than once at one place, its copies answering such a list and selecting otherwise,
//! at that place or below copies of its parent, refuses the operation once validation
//! passed ([`OwnKey::give`]); copies selecting alike answer alike, and stay.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use async_graphql::indexmap::IndexMap;
use async_graphql::parser::types::{
    Directive, DocumentOperations, ExecutableDocument, Field, FragmentDefinition,
    OperationDefinition, OperationType, Selection, SelectionSet,
};
use async_graphql::registry::{MetaField, MetaType, MetaTypeName, Registry};
use async_graphql::{Name, Pos, Positioned, ServerError, Value, Variables};

use crate::operation_units::{RECURSIVE_DEPTH, is_skipped};

/// What the key a request's document gives its objects starts with, sixteen hex
/// digits drawn for the request after it ([`drawn_key`]).
const KEY: &str = "__nestrs_key";

/// The key one request's document gives the objects that could answer none,
/// shared by the read that gives it and the answer it leaves.
#[derive(Clone, Default)]
pub(crate) struct OwnKey(Arc<OnceLock<Name>>);

impl OwnKey {
    /// `document`, its operation `operation` selects, with every selection set that
    /// could answer no key given one; `None` when none could. `registry` holds the
    /// schema's types, and `variables` what `@skip` and `@include` read. Read once
    /// the document's cost was counted, which bounds what each fragment expands to.
    ///
    /// # Errors
    ///
    /// The refusal of a response key whose copies no key joins, pointing at the
    /// first copy selecting otherwise than the first: a list of lists selected more
    /// than once at one place.
    pub(crate) fn give(
        &self,
        document: &ExecutableDocument,
        operation: &OperationDefinition,
        registry: &Registry,
        variables: &Variables,
    ) -> Result<Option<ExecutableDocument>, ServerError> {
        let mut marks = Marks::new(&document.fragments, registry, variables);
        marks.of_operation(operation);
        if let Some((key, at)) = marks.unjoined {
            return Err(unjoined(key, at));
        }
        if marks.sets.is_empty() {
            return Ok(None);
        }
        let key = drawn_key();
        // A request is read once; were it read again, the answer could lose only the
        // key it was first given, so no second one is.
        if self.0.set(key.clone()).is_err() {
            return Ok(None);
        }
        let mut given = document.clone();
        if let Some(chosen) = selected_operation_mut(&mut given, operation) {
            give_key(&mut chosen.selection_set, &marks.sets, &key);
        }
        for fragment in given.fragments.values_mut() {
            give_key(&mut fragment.node.selection_set, &marks.sets, &key);
        }
        Ok(Some(given))
    }

    /// `data` without the key the document gave its objects, if it gave one.
    pub(crate) fn take_from(&self, data: &mut Value) {
        if let Some(key) = self.0.get() {
            take_key(data, key);
        }
    }
}

/// The operation of `document` that is `operation`, found again in its copy.
fn selected_operation_mut<'d>(
    document: &'d mut ExecutableDocument,
    operation: &OperationDefinition,
) -> Option<&'d mut OperationDefinition> {
    let at = operation.selection_set.pos;
    match &mut document.operations {
        DocumentOperations::Single(single) => Some(&mut single.node),
        DocumentOperations::Multiple(operations) => operations
            .values_mut()
            .map(|operation| &mut operation.node)
            .find(|candidate| candidate.selection_set.pos == at),
    }
}

/// `__typename` under `key` in each set of `set` that opens at one of `marked`.
fn give_key(set: &mut Positioned<SelectionSet>, marked: &HashSet<Pos>, key: &Name) {
    for selection in &mut set.node.items {
        match &mut selection.node {
            Selection::Field(field) => give_key(&mut field.node.selection_set, marked, key),
            Selection::InlineFragment(fragment) => {
                give_key(&mut fragment.node.selection_set, marked, key);
            }
            Selection::FragmentSpread(_) => {}
        }
    }
    if marked.contains(&set.pos) {
        let at = set.pos;
        let typename = Field {
            alias: Some(Positioned::new(key.clone(), at)),
            name: Positioned::new(Name::new("__typename"), at),
            arguments: Vec::new(),
            directives: Vec::new(),
            selection_set: Positioned::new(SelectionSet::default(), Pos::default()),
        };
        set.node.items.push(Positioned::new(
            Selection::Field(Positioned::new(typename, at)),
            at,
        ));
    }
}

/// `value` without `key` in any object it holds.
fn take_key(value: &mut Value, key: &Name) {
    match value {
        Value::Object(fields) => {
            fields.shift_remove(key);
            for field in fields.values_mut() {
                take_key(field, key);
            }
        }
        Value::List(items) => {
            for item in items {
                take_key(item, key);
            }
        }
        _ => {}
    }
}

/// A selection of an object merged into a set: its response key, the field, and
/// the type it is selected on.
#[derive(Clone, Copy)]
struct Composite<'a> {
    key: &'a str,
    field: &'a Positioned<Field>,
    on: &'a str,
}

/// The selection sets of one operation that could answer no key, read against the
/// schema's types where the document asks for them.
struct Marks<'a> {
    fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
    registry: &'a Registry,
    variables: &'a Variables,
    /// The sets to give a key, by where each opens.
    sets: HashSet<Pos>,
    /// What each fragment merges into a set, read once.
    merged: HashMap<&'a str, Vec<Composite<'a>>>,
    /// Whether each fragment selects a field of a value of a type, resolved as a
    /// type.
    selects: HashMap<(&'a str, &'a str, &'a str), bool>,
    /// Each set read so far, and whether a repeat stood above it then.
    walked: HashMap<Pos, bool>,
    /// Whether a set selects one object's response key more than once.
    repeats: bool,
    /// The fields each fragment selects of a value of a type, resolved as a type,
    /// read once.
    collected: HashMap<(&'a str, &'a str, &'a str), Vec<&'a Positioned<Field>>>,
    /// Each place read for copies no key joins, by where its copies open and the
    /// type it declares.
    joined: HashSet<(Vec<Pos>, &'a str)>,
    /// The first response key whose copies no key joins, and where the copy
    /// selecting otherwise stands.
    unjoined: Option<(&'a str, Pos)>,
}

impl<'a> Marks<'a> {
    fn new(
        fragments: &'a HashMap<Name, Positioned<FragmentDefinition>>,
        registry: &'a Registry,
        variables: &'a Variables,
    ) -> Self {
        Self {
            fragments,
            registry,
            variables,
            sets: HashSet::new(),
            merged: HashMap::new(),
            selects: HashMap::new(),
            walked: HashMap::new(),
            repeats: false,
            collected: HashMap::new(),
            joined: HashSet::new(),
            unjoined: None,
        }
    }

    fn of_operation(&mut self, operation: &'a OperationDefinition) {
        let root = match operation.ty {
            OperationType::Query => Some(self.registry.query_type.as_str()),
            OperationType::Mutation => self.registry.mutation_type.as_deref(),
            OperationType::Subscription => self.registry.subscription_type.as_deref(),
        };
        let Some(root) = root else {
            return;
        };
        let set = &operation.selection_set;
        // Without a fragment, a directive or a repeated key every set selects a
        // field, so no type is read.
        if self.fragments.is_empty()
            && !directed(&set.node, RECURSIVE_DEPTH)
            && !repeats_below(&set.node, RECURSIVE_DEPTH)
        {
            return;
        }
        // A subscription's root holds its one field, another refused once
        // validation passed (`operation_units`): a key there is no answer.
        if operation.ty != OperationType::Subscription && !self.answers(&set.node, root, 0) {
            self.sets.insert(set.pos);
        }
        self.walk(set, root, false, 0);
        // Copies meet only below a repeat, which the walk read wherever it stands.
        if self.repeats {
            self.unjoined = self.first_unjoined(&[set], root, 0);
        }
    }

    /// Each set of an object merged into `set`, selected on `on`, read: given a key
    /// when it could answer none, or when `repeated`, a copy of a key selected more
    /// than once standing above it.
    fn walk(
        &mut self,
        set: &'a Positioned<SelectionSet>,
        on: &'a str,
        repeated: bool,
        depth: usize,
    ) {
        if depth > RECURSIVE_DEPTH {
            return;
        }
        let merged = self.merge(&set.node, on, depth);
        let mut first: HashMap<&str, Pos> = HashMap::with_capacity(merged.len());
        let mut repeats: HashSet<&str> = HashSet::new();
        for composite in &merged {
            let at = composite.field.node.selection_set.pos;
            if *first.entry(composite.key).or_insert(at) != at {
                repeats.insert(composite.key);
            }
        }
        self.repeats |= !repeats.is_empty();
        for composite in merged {
            let below = &composite.field.node.selection_set;
            let repeated = repeated || repeats.contains(composite.key);
            match self.walked.get(&below.pos) {
                Some(true) => continue,
                Some(false) if !repeated => continue,
                _ => {}
            }
            self.walked.insert(below.pos, repeated);
            let Some(ty) = self.field_type(composite.on, &composite.field.node.name.node) else {
                // A field the schema does not declare is validation's to refuse.
                continue;
            };
            if (repeated && self.is_object(ty)) || !self.answers(&below.node, ty, depth + 1) {
                self.sets.insert(below.pos);
            }
            self.walk(below, ty, repeated, depth + 1);
        }
    }

    /// The objects `set`, selected on `on`, merges, through its fragments.
    fn merge(&mut self, set: &'a SelectionSet, on: &'a str, depth: usize) -> Vec<Composite<'a>> {
        let mut merged = Vec::new();
        self.merge_into(set, on, depth, &mut merged);
        merged
    }

    fn merge_into(
        &mut self,
        set: &'a SelectionSet,
        on: &'a str,
        depth: usize,
        merged: &mut Vec<Composite<'a>>,
    ) {
        if depth > RECURSIVE_DEPTH {
            return;
        }
        for selection in &set.items {
            if is_skipped(selection.node.directives(), self.variables) {
                continue;
            }
            match &selection.node {
                Selection::Field(field) => {
                    let key = field.node.response_key().node.as_str();
                    if !field.node.selection_set.node.items.is_empty() {
                        merged.push(Composite { key, field, on });
                    }
                }
                Selection::InlineFragment(fragment) => {
                    let on = fragment
                        .node
                        .type_condition
                        .as_ref()
                        .map_or(on, |condition| condition.node.on.node.as_str());
                    self.merge_into(&fragment.node.selection_set.node, on, depth + 1, merged);
                }
                Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.as_str();
                    merged.extend(self.merge_fragment(name, depth + 1).iter().copied());
                }
            }
        }
    }

    /// What fragment `name` merges, read once: one still being read, a cycle
    /// validation refuses, merges nothing.
    fn merge_fragment(&mut self, name: &'a str, depth: usize) -> &[Composite<'a>] {
        if !self.merged.contains_key(name) {
            self.merged.insert(name, Vec::new());
            if let Some(fragment) = self.fragments.get(name) {
                let on = fragment.node.type_condition.node.on.node.as_str();
                let read = self.merge(&fragment.node.selection_set.node, on, depth);
                self.merged.insert(name, read);
            }
        }
        self.merged.get(name).map_or(&[], Vec::as_slice)
    }

    /// Whether `ty` is an object, an interface or a union, which `__typename` is
    /// valid on.
    fn is_object(&self, ty: &str) -> bool {
        self.registry
            .types
            .get(ty)
            .is_some_and(MetaType::is_composite)
    }

    /// Whether `set`, selected on `ty`, answers a key whatever type its value takes.
    fn answers(&mut self, set: &'a SelectionSet, ty: &'a str, depth: usize) -> bool {
        let registry = self.registry;
        match registry.types.get(ty) {
            Some(MetaType::Object { .. }) => self.selects(set, ty, ty, depth),
            Some(abstract_type) => abstract_type.possible_types().is_none_or(|possible| {
                possible
                    .iter()
                    .all(|concrete| self.selects(set, concrete, ty, depth))
            }),
            None => true,
        }
    }

    /// Whether `set` selects a field of a value of type `concrete` resolved as
    /// `resolving`, the type its position declares until a condition names
    /// `concrete` or an interface of it.
    fn selects(
        &mut self,
        set: &'a SelectionSet,
        concrete: &'a str,
        resolving: &'a str,
        depth: usize,
    ) -> bool {
        if depth > RECURSIVE_DEPTH {
            return true;
        }
        set.items.iter().any(|selection| {
            !is_skipped(selection.node.directives(), self.variables)
                && match &selection.node {
                    Selection::Field(_) => true,
                    Selection::InlineFragment(fragment) => match &fragment.node.type_condition {
                        Some(condition) => {
                            self.applies(condition.node.on.node.as_str(), concrete, resolving)
                        }
                        None => Some(resolving),
                    }
                    .is_some_and(|resolving| {
                        self.selects(
                            &fragment.node.selection_set.node,
                            concrete,
                            resolving,
                            depth + 1,
                        )
                    }),
                    Selection::FragmentSpread(spread) => self.fragment_selects(
                        spread.node.fragment_name.node.as_str(),
                        concrete,
                        resolving,
                        depth,
                    ),
                }
        })
    }

    fn fragment_selects(
        &mut self,
        name: &'a str,
        concrete: &'a str,
        resolving: &'a str,
        depth: usize,
    ) -> bool {
        let read_as = (name, concrete, resolving);
        if let Some(read) = self.selects.get(&read_as) {
            return *read;
        }
        self.selects.insert(read_as, false);
        let read = self.fragments.get(name).is_some_and(|fragment| {
            self.applies(
                fragment.node.type_condition.node.on.node.as_str(),
                concrete,
                resolving,
            )
            .is_some_and(|resolving| {
                self.selects(
                    &fragment.node.selection_set.node,
                    concrete,
                    resolving,
                    depth + 1,
                )
            })
        });
        self.selects.insert(read_as, read);
        read
    }

    /// The type a value of `concrete` resolved as `resolving` is resolved as below a
    /// type condition on `condition`, as async-graphql (`7.2.1`, `Fields::add_set`)
    /// applies it: `concrete` below its own name or an interface it implements,
    /// `resolving` below its own, and `None` below any other. So a condition on a
    /// union applies only where that union is resolved, never to a member selected
    /// elsewhere, where GraphQL §6.3.2 applies it (`TODO.md` §4).
    fn applies(&self, condition: &str, concrete: &'a str, resolving: &'a str) -> Option<&'a str> {
        let implemented = self
            .registry
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

    /// The named type field `name` of `on` answers.
    fn field_type(&self, on: &str, name: &str) -> Option<&'a str> {
        let declared = self.registry.types.get(on)?.field_by_name(name)?;
        Some(MetaTypeName::concrete_typename(&declared.ty))
    }

    /// The first response key, at the place the sets `copies` merge into, all
    /// selected on `ty`, or below it, whose copies of a value of one type select
    /// otherwise and answer a list of lists, and where the first copy selecting
    /// otherwise stands. Each place is read once, and copies alike stand for one.
    fn first_unjoined(
        &mut self,
        copies: &[&'a Positioned<SelectionSet>],
        ty: &'a str,
        depth: usize,
    ) -> Option<(&'a str, Pos)> {
        let place: (Vec<Pos>, &'a str) = (copies.iter().map(|set| set.pos).collect(), ty);
        if depth > RECURSIVE_DEPTH || !self.joined.insert(place) {
            return None;
        }
        let registry = self.registry;
        let concretes: Vec<&'a str> = match registry.types.get(ty)? {
            MetaType::Object { .. } => vec![ty],
            abstract_type => abstract_type
                .possible_types()?
                .iter()
                .map(String::as_str)
                .collect(),
        };
        for concrete in concretes {
            let mut fields = Vec::new();
            for set in copies {
                self.collect(&set.node, concrete, ty, depth, &mut fields);
            }
            let mut seen = HashSet::with_capacity(fields.len());
            let mut keys: IndexMap<&'a str, Vec<&'a Positioned<Field>>> = IndexMap::new();
            for field in fields {
                if seen.insert(field.pos) {
                    let key = field.node.response_key().node.as_str();
                    keys.entry(key).or_default().push(field);
                }
            }
            for (key, group) in keys {
                let first = group[0];
                let otherwise = group[1..]
                    .iter()
                    .find(|copy| !self.alike_fields(&first.node, &copy.node, depth));
                let declared = |copy: &&'a Positioned<Field>| -> Option<&'a MetaField> {
                    registry
                        .types
                        .get(concrete)?
                        .field_by_name(copy.node.name.node.as_str())
                };
                if let Some(otherwise) = otherwise
                    && group
                        .iter()
                        .filter_map(declared)
                        .any(|field| nests_lists(&field.ty))
                {
                    return Some((key, otherwise.pos));
                }
                let Some(below) = declared(&first) else {
                    // A field the schema does not declare is validation's to refuse.
                    continue;
                };
                let below = MetaTypeName::concrete_typename(&below.ty);
                let sets: Vec<_> = match otherwise {
                    Some(_) => group.iter().map(|copy| &copy.node.selection_set).collect(),
                    None => vec![&first.node.selection_set],
                };
                if let Some(found) = self.first_unjoined(&sets, below, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// The fields with a selection set of their own that `set` selects of a value
    /// of type `concrete` resolved as `resolving`, through its fragments, applied as
    /// [`Self::selects`] applies them.
    fn collect(
        &mut self,
        set: &'a SelectionSet,
        concrete: &'a str,
        resolving: &'a str,
        depth: usize,
        fields: &mut Vec<&'a Positioned<Field>>,
    ) {
        if depth > RECURSIVE_DEPTH {
            return;
        }
        for selection in &set.items {
            if is_skipped(selection.node.directives(), self.variables) {
                continue;
            }
            match &selection.node {
                Selection::Field(field) => {
                    if !field.node.selection_set.node.items.is_empty() {
                        fields.push(field);
                    }
                }
                Selection::InlineFragment(fragment) => {
                    let applied = match &fragment.node.type_condition {
                        Some(condition) => {
                            self.applies(condition.node.on.node.as_str(), concrete, resolving)
                        }
                        None => Some(resolving),
                    };
                    if let Some(resolving) = applied {
                        let below = &fragment.node.selection_set.node;
                        self.collect(below, concrete, resolving, depth + 1, fields);
                    }
                }
                Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.as_str();
                    let selected = self.fragment_fields(name, concrete, resolving, depth + 1);
                    fields.extend(selected.iter().copied());
                }
            }
        }
    }

    /// What fragment `name` collects of a value of type `concrete` resolved as
    /// `resolving`, read once: one still being read, a cycle validation refuses,
    /// collects nothing.
    fn fragment_fields(
        &mut self,
        name: &'a str,
        concrete: &'a str,
        resolving: &'a str,
        depth: usize,
    ) -> &[&'a Positioned<Field>] {
        let read_as = (name, concrete, resolving);
        if !self.collected.contains_key(&read_as) {
            self.collected.insert(read_as, Vec::new());
            let mut fields = Vec::new();
            if let Some(fragment) = self.fragments.get(name) {
                let condition = fragment.node.type_condition.node.on.node.as_str();
                if let Some(resolving) = self.applies(condition, concrete, resolving) {
                    let below = &fragment.node.selection_set.node;
                    self.collect(below, concrete, resolving, depth, &mut fields);
                }
            }
            self.collected.insert(read_as, fields);
        }
        self.collected.get(&read_as).map_or(&[], Vec::as_slice)
    }

    /// Whether two copies of one response key answer alike wherever they stand: one
    /// field, read with one argument list and one directive list, `@skip` and
    /// `@include` aside, which were read before, and what runs of each selection
    /// set alike, selection by selection.
    fn alike_fields(&self, a: &Field, b: &Field, depth: usize) -> bool {
        a.name.node == b.name.node
            && a.arguments == b.arguments
            && alike_directives(&a.directives, &b.directives)
            && self.alike_sets(&a.selection_set.node, &b.selection_set.node, depth + 1)
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

/// The refusal of `key`, whose copy at `at` no key joins to the first.
fn unjoined(key: &str, at: Pos) -> ServerError {
    crate::error::invalid_argument(format!(
        r#"the operation selects "{key}", a list of lists, more than once at one place with different selections"#
    ))
    .into_server_error(at)
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

/// The selections of `set` that `@skip` and `@include` leave in.
fn running<'s>(
    set: &'s SelectionSet,
    variables: &'s Variables,
) -> impl Iterator<Item = &'s Positioned<Selection>> {
    set.items
        .iter()
        .filter(move |selection| !is_skipped(selection.node.directives(), variables))
}

/// Whether two selections carry one directive list, `@skip` and `@include` aside,
/// which were read before.
fn alike_directives(a: &[Positioned<Directive>], b: &[Positioned<Directive>]) -> bool {
    let read = |directive: &&Positioned<Directive>| {
        !matches!(directive.node.name.node.as_str(), "skip" | "include")
    };
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

/// A key for one request: the process's secret hash keys over a count of the keys
/// drawn, so no two requests share one and no caller can foresee it.
///
/// The answer loses the key from every object it holds, a scalar's value among
/// them, which no read of the document could tell apart: async-graphql (`7.2.1`)
/// runs no `SameResponseShape` (§5.3.2), so one response key may answer a scalar
/// under one type condition and an object under another. Neither a document nor a
/// stored value spells a drawn key but by a 64-bit guess.
fn drawn_key() -> Name {
    static SEED: LazyLock<RandomState> = LazyLock::new(RandomState::new);
    static DRAWN: AtomicU64 = AtomicU64::new(0);
    let drawn = SEED.hash_one(DRAWN.fetch_add(1, Ordering::Relaxed));
    Name::new(format!("{KEY}_{drawn:016x}"))
}

/// Whether two selections of `set`, or of a set within `depth` sets of it, name one
/// object's response key, in a document holding no fragment, whose sets merge none.
/// Each key is looked up, never scanned for: this runs before validation scores a
/// document, whose width the body cap alone holds.
fn repeats_below(set: &SelectionSet, depth: usize) -> bool {
    let mut keys: HashSet<&str> = HashSet::new();
    depth > 0
        && set.items.iter().any(|selection| match &selection.node {
            Selection::Field(field) if !field.node.selection_set.node.items.is_empty() => {
                !keys.insert(field.node.response_key().node.as_str())
                    || repeats_below(&field.node.selection_set.node, depth - 1)
            }
            _ => false,
        })
}

/// Whether `set` holds a directive or a fragment within `depth` sets of it: the one
/// case a set may select no field.
fn directed(set: &SelectionSet, depth: usize) -> bool {
    depth > 0
        && set.items.iter().any(|selection| {
            !selection.node.directives().is_empty()
                || match &selection.node {
                    Selection::Field(field) => directed(&field.node.selection_set.node, depth - 1),
                    Selection::InlineFragment(_) | Selection::FragmentSpread(_) => true,
                }
        })
}

#[cfg(test)]
mod tests {
    use async_graphql::parser::parse_query;
    use async_graphql::{Interface, Object, OutputType, SimpleObject, Union};
    use serde_json::json;

    use super::*;
    use crate::operation_units::selected_operation;
    use crate::testing::thread_cpu_time;

    #[derive(SimpleObject)]
    struct Fine {
        fine: i32,
    }

    #[derive(SimpleObject)]
    struct Other {
        other: i32,
    }

    #[derive(Union)]
    enum Either {
        Fine(Fine),
        Other(Other),
    }

    #[derive(Interface)]
    #[graphql(field(name = "fine", ty = "&i32"))]
    enum Node {
        Fine(Fine),
    }

    struct Query;

    #[Object]
    impl Query {
        async fn fine(&self) -> Fine {
            Fine { fine: 1 }
        }

        async fn fines(&self) -> Vec<Fine> {
            Vec::new()
        }

        async fn grid(&self) -> Vec<Vec<Fine>> {
            Vec::new()
        }

        async fn either(&self) -> Option<Either> {
            None
        }

        async fn node(&self) -> Option<Node> {
            None
        }

        async fn count(&self) -> i32 {
            0
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::default();
        let root = Query::create_type_info(&mut registry);
        registry.query_type = MetaTypeName::concrete_typename(&root).to_owned();
        registry
    }

    /// The document `text`, its objects given their key, as text again, with its
    /// variables `variables`; `None` when no set needs one.
    fn given(text: &str, variables: serde_json::Value) -> Option<String> {
        let document = parse_query(text).expect("the document parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let variables = Variables::from_json(variables);
        let own_key = OwnKey::default();
        own_key
            .give(&document, &operation.node, &registry(), &variables)
            .unwrap_or_else(|refusal| panic!("{text}: {}", refusal.message))
            .map(|given| {
                let key = own_key.0.get().expect("a key given");
                let (_, operation) = selected_operation(&given, None).expect("one operation");
                let mut keyed = Vec::new();
                keyed_sets(&operation.node.selection_set.node, key, "", &mut keyed);
                let mut fragments = Vec::new();
                for (name, fragment) in &given.fragments {
                    keyed_sets(&fragment.node.selection_set.node, key, name, &mut fragments);
                }
                fragments.sort();
                keyed.extend(fragments);
                keyed.join(" ")
            })
    }

    /// The path, by response keys, of each set holding `key`.
    fn keyed_sets(set: &SelectionSet, key: &str, at: &str, keyed: &mut Vec<String>) {
        for selection in &set.items {
            match &selection.node {
                Selection::Field(field) => {
                    let response_key = field.node.response_key().node.as_str();
                    if response_key == key {
                        keyed.push(if at.is_empty() {
                            "/".to_owned()
                        } else {
                            at.to_owned()
                        });
                    }
                    keyed_sets(
                        &field.node.selection_set.node,
                        key,
                        &format!("{at}/{response_key}"),
                        keyed,
                    );
                }
                Selection::InlineFragment(fragment) => {
                    keyed_sets(&fragment.node.selection_set.node, key, at, keyed);
                }
                Selection::FragmentSpread(_) => {}
            }
        }
    }

    #[test]
    fn a_set_selecting_a_field_whatever_its_value_takes_no_key() {
        for document in [
            "{ fine { fine } count }",
            "{ fines { fine } }",
            "{ either { ... on Fine { fine } ... on Other { other } } }",
            "{ either { __typename } }",
            "{ node { fine } }",
            "{ node { ... on Node { fine } } }",
            "{ node { ... on Fine { fine } } }",
            "{ fine { ...F } } fragment F on Fine { fine }",
            "{ fine { fine @include(if: true) } }",
            "{ a: fine { fine } b: fine { fine } }",
        ] {
            assert_eq!(given(document, json!({})), None, "{document}");
        }
    }

    #[test]
    fn a_set_that_could_answer_no_key_takes_one() {
        for (document, variables, keyed) in [
            ("{ fine { fine @skip(if: true) } }", json!({}), "/fine"),
            (
                "query($x: Boolean) { fine { fine @include(if: $x) } }",
                json!({ "x": false }),
                "/fine",
            ),
            (
                "{ fines { ... @skip(if: true) { fine } } }",
                json!({}),
                "/fines",
            ),
            ("{ either { ... on Fine { fine } } }", json!({}), "/either"),
            (
                "{ either { ...F } } fragment F on Fine { fine }",
                json!({}),
                "/either",
            ),
            ("{ count @skip(if: true) }", json!({}), "/"),
            (
                "{ fine { fine } fine { fine @skip(if: true) } }",
                json!({}),
                "/fine /fine",
            ),
            (
                "{ fine { fine } fine { f: fine } }",
                json!({}),
                "/fine /fine",
            ),
            (
                "{ ...A ...B } fragment A on Query { fine { fine } } \
                 fragment B on Query { fine { fine } }",
                json!({}),
                "A/fine B/fine",
            ),
        ] {
            assert_eq!(
                given(document, variables).as_deref(),
                Some(keyed),
                "{document}"
            );
        }
    }

    /// async-graphql applies a condition on a union where that union is resolved, and
    /// to no member of it selected elsewhere, which is then left with no key.
    #[test]
    fn a_condition_on_a_union_applies_where_that_union_is_resolved_alone() {
        for document in [
            "{ either { ... on Either { ... on Fine { fine } ... on Other { other } } } }",
            "{ either { ...E } } fragment E on Either { ... on Fine { fine } ... on Other { other } }",
        ] {
            assert_eq!(given(document, json!({})), None, "{document}");
        }
        for document in [
            "{ fine { ... on Either { ... on Fine { fine } } } }",
            "{ fine { ...E } } fragment E on Either { ... on Fine { fine } }",
        ] {
            assert_eq!(
                given(document, json!({})).as_deref(),
                Some("/fine"),
                "{document}"
            );
        }
    }

    /// One fragment spread twice runs the same selections twice, which answer alike:
    /// no copy of it hides another.
    #[test]
    fn a_fragment_spread_twice_is_no_repeat() {
        let document = "{ ...A ...A } fragment A on Query { fine { fine } }";
        assert_eq!(given(document, json!({})), None);
    }

    /// Below a repeated key, any object may answer nothing whole in one copy.
    #[test]
    fn what_stands_below_a_repeated_key_takes_a_key_too() {
        let keyed = given("{ fine { fine } fine { fine } } ", json!({}));
        assert_eq!(keyed.as_deref(), Some("/fine /fine"));
        let keyed = given("{ fines { fine } fines { fine } }", json!({}));
        assert_eq!(keyed.as_deref(), Some("/fines /fines"));
    }

    /// The refusal `text`'s operation is read to, when a key's copies no key joins.
    fn refused(text: &str) -> Option<ServerError> {
        let document = parse_query(text).expect("the document parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        OwnKey::default()
            .give(
                &document,
                &operation.node,
                &registry(),
                &Variables::default(),
            )
            .err()
    }

    /// A list of lists whose copies at one place run otherwise is refused at the first
    /// copy that does; copies running alike, a list of objects and two keys are not.
    #[test]
    fn a_list_of_lists_selected_twice_otherwise_is_refused_at_the_copy_that_differs() {
        let refusal = refused("{\n  grid { fine }\n  grid { fine }\n  grid { f: fine }\n}")
            .expect("the third copy selects otherwise");
        assert_eq!(refusal.locations, [Pos { line: 4, column: 3 }]);
        assert_eq!(
            refusal.message,
            r#"the operation selects "grid", a list of lists, more than once at one place with different selections"#
        );
        for document in [
            "{ grid { fine @skip(if: true) } grid { fine } }",
            "{ grid { fine } ... on Query { grid { f: fine } } }",
            "{ ...A grid { f: fine } } fragment A on Query { grid { fine } }",
            "{ fines { fine } fines { f: fine } grid { fine } grid { f: fine } }",
        ] {
            assert!(refused(document).is_some(), "{document}");
        }
        for document in [
            "{ grid { fine } grid { fine } }",
            "{ grid { fine @include(if: true) } grid { fine } }",
            "{ grid @include(if: true) { fine } grid { fine } }",
            "{ grid { ...F } grid { ...F } } fragment F on Fine { fine }",
            "{ ...A ...A } fragment A on Query { grid { fine } }",
            "{ a: grid { fine } b: grid { f: fine } }",
            "{ fines { fine } fines { f: fine } }",
            "{ grid { fine } grid @skip(if: true) { f: fine } }",
        ] {
            assert!(refused(document).is_none(), "{document}");
        }
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
        let mut marks = Marks::new(&document.fragments, &registry, &variables);
        let started = thread_cpu_time();
        let unjoined = marks.first_unjoined(&[&operation.node.selection_set], "Query", 0);
        let past = thread_cpu_time() - started;
        assert!(unjoined.is_none(), "every copy alike");
        assert!(past < parsing, "parsed in {parsing:?}, then {past:?}");
    }

    /// Each request draws a key of its own, which nothing the answer holds spells: a
    /// member a client or a stored value names as a key once was keeps its place.
    #[test]
    fn the_key_is_drawn_for_each_request() {
        let document = parse_query("{ fine { __nestrs_key: fine } fine { fine @skip(if: true) } }")
            .expect("parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let drawn: Vec<OwnKey> = (0..2)
            .map(|_| {
                let own_key = OwnKey::default();
                own_key
                    .give(
                        &document,
                        &operation.node,
                        &registry(),
                        &Variables::default(),
                    )
                    .expect("nothing refused");
                own_key
            })
            .collect();
        let keys: Vec<&str> = drawn
            .iter()
            .map(|own_key| own_key.0.get().expect("a key given").as_str())
            .collect();
        assert_ne!(keys[0], keys[1]);
        for key in &keys {
            let digits = key.strip_prefix("__nestrs_key_").expect("the prefix");
            assert!(
                digits.len() == 16 && digits.bytes().all(|digit| digit.is_ascii_hexdigit()),
                "{key}"
            );
        }
        let mut data = Value::from_json(json!({
            "fine": { "__nestrs_key": 1, "__nestrs_key1": 2, keys[0]: "Fine" },
        }))
        .expect("plain JSON");
        drawn[0].take_from(&mut data);
        assert_eq!(
            data,
            Value::from_json(json!({ "fine": { "__nestrs_key": 1, "__nestrs_key1": 2 } }))
                .expect("plain JSON")
        );
    }

    #[test]
    fn the_key_leaves_every_object_of_the_answer() {
        let own_key = OwnKey::default();
        own_key.0.set(Name::new(KEY)).expect("unset");
        let mut data = Value::from_json(json!({
            KEY: "Query",
            "a": [{ KEY: "A", "b": { KEY: "B", "c": 1 } }, null],
            "d": "kept",
        }))
        .expect("plain JSON");
        own_key.take_from(&mut data);
        assert_eq!(
            data,
            Value::from_json(json!({ "a": [{ "b": { "c": 1 } }, null], "d": "kept" }))
                .expect("plain JSON")
        );
    }

    /// A document no fragment, directive or repeat reads reads no type at all: the
    /// schema below names none of the fields it selects.
    #[test]
    fn a_plain_document_reads_no_type() {
        let document = parse_query("{ unknown { field } other { field } }").expect("parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let given = OwnKey::default().give(
            &document,
            &operation.node,
            &Registry::default(),
            &Variables::default(),
        );
        assert!(matches!(given, Ok(None)));
    }

    /// Each set's keys are looked up, so a wide document costs a thirtieth of its parse
    /// here, where a scan of the keys read so far cost their square: twice its parse
    /// at 20,000 objects, and four times as much at twice the width.
    #[test]
    fn a_wide_document_costs_no_square_of_its_width() {
        let objects: String = (0..20_000)
            .map(|at| format!("a{at}: fine {{ fine }} "))
            .collect();
        let text = format!("{{ {objects}}}");
        let registry = registry();
        let started = thread_cpu_time();
        let document = parse_query(&text).expect("the document parses");
        let parsing = thread_cpu_time() - started;
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let started = thread_cpu_time();
        let given =
            OwnKey::default().give(&document, &operation.node, &registry, &Variables::default());
        let past = thread_cpu_time() - started;
        assert!(matches!(given, Ok(None)), "no object repeats");
        assert!(past < parsing, "parsed in {parsing:?}, then {past:?}");
    }
}
