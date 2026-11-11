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
//! The document read is the one [`crate::field_collection`] collected as GraphQL
//! §6.3.2 reads it, where async-graphql would have answered it otherwise: what no
//! key joins, a list of lists selected twice, is merged there before any key is
//! given.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use async_graphql::parser::types::{
    DocumentOperations, ExecutableDocument, Field, FragmentDefinition, OperationDefinition,
    OperationType, Selection, SelectionSet,
};
use async_graphql::registry::{MetaType, Registry};
use async_graphql::{Name, Pos, Positioned, ServerError, Value, Variables};

use crate::field_collection::{self, engine_applies};
use crate::operation_units::{RECURSIVE_DEPTH, is_skipped};

/// What the key a request's document gives its objects starts with, sixteen hex
/// digits drawn for the request after it ([`drawn_key`]).
const KEY: &str = "__nestrs_key";

/// The key one request's document gives the objects that could answer none,
/// shared by the read that gives it and the answer it leaves.
#[derive(Clone, Default)]
pub(crate) struct OwnKey(Arc<OnceLock<Name>>);

impl OwnKey {
    /// `document`, its operation `operation` selects, collected as GraphQL §6.3.2
    /// reads it ([`field_collection::collected`]), with every selection set that could
    /// answer no key given one; `None` when it stands as sent. `registry` holds the
    /// schema's types, and `variables` what `@skip` and `@include` read. Read once
    /// the document's cost was counted, which bounds what each fragment expands to.
    ///
    /// # Errors
    ///
    /// The refusal [`field_collection::collected`] answers: a response key whose
    /// copies no one field answers.
    pub(crate) fn give(
        &self,
        document: &ExecutableDocument,
        operation: &OperationDefinition,
        registry: &Registry,
        variables: &Variables,
    ) -> Result<Option<ExecutableDocument>, ServerError> {
        let collected = field_collection::collected(document, operation, registry, variables)?;
        let (read, chosen) = collected
            .as_ref()
            .and_then(|rewritten| {
                Some((rewritten, field_collection::chosen(rewritten, operation)?))
            })
            .unwrap_or((document, operation));
        let sets = {
            let mut marks = Marks::new(&read.fragments, registry, variables);
            marks.of_operation(chosen);
            marks.sets
        };
        if sets.is_empty() {
            return Ok(collected);
        }
        let key = drawn_key();
        // A request is read once; were it read again, the answer could lose only the
        // key it was first given, so no second one is.
        if self.0.set(key.clone()).is_err() {
            return Ok(collected);
        }
        let mut given = collected.unwrap_or_else(|| document.clone());
        if let Some(chosen) = selected_operation_mut(&mut given, operation) {
            give_key(&mut chosen.selection_set, &sets, &key);
        }
        for fragment in given.fragments.values_mut() {
            give_key(&mut fragment.node.selection_set, &sets, &key);
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
        for composite in merged {
            let below = &composite.field.node.selection_set;
            let repeated = repeated || repeats.contains(composite.key);
            match self.walked.get(&below.pos) {
                Some(true) => continue,
                Some(false) if !repeated => continue,
                _ => {}
            }
            self.walked.insert(below.pos, repeated);
            let name = &composite.field.node.name.node;
            let Some(ty) = field_collection::field_type(self.registry, composite.on, name) else {
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
                        Some(condition) => engine_applies(
                            self.registry,
                            condition.node.on.node.as_str(),
                            concrete,
                            resolving,
                        ),
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
            engine_applies(
                self.registry,
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
/// object's response key, in a document holding no fragment. Each key is looked up,
/// never scanned for: this runs before validation scores a document, whose width
/// the body cap alone holds.
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
    use async_graphql::registry::MetaTypeName;
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

    /// The paths of the sets of the document `text`, with its variables
    /// `variables`, given a key; `None` when no set needs one.
    fn given(text: &str, variables: serde_json::Value) -> Option<String> {
        let document = parse_query(text).expect("the document parses");
        let (_, operation) = selected_operation(&document, None).expect("one operation");
        let variables = Variables::from_json(variables);
        let own_key = OwnKey::default();
        own_key
            .give(&document, &operation.node, &registry(), &variables)
            .unwrap_or_else(|refusal| panic!("{text}: {}", refusal.message))
            .and_then(|given| {
                let key = own_key.0.get()?;
                let (_, operation) = selected_operation(&given, None).expect("one operation");
                let mut keyed = Vec::new();
                keyed_sets(&operation.node.selection_set.node, key, "", &mut keyed);
                let mut fragments = Vec::new();
                for (name, fragment) in &given.fragments {
                    keyed_sets(&fragment.node.selection_set.node, key, name, &mut fragments);
                }
                fragments.sort();
                keyed.extend(fragments);
                Some(keyed.join(" "))
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

    /// A condition on a union is read as async-graphql applies it, where that union
    /// is resolved; one on a member resolved elsewhere was collected to apply there
    /// before any key is given (`crate::field_collection`).
    #[test]
    fn a_condition_on_a_union_takes_no_key_where_it_applies() {
        for document in [
            "{ either { ... on Either { ... on Fine { fine } ... on Other { other } } } }",
            "{ either { ...E } } fragment E on Either { ... on Fine { fine } ... on Other { other } }",
            "{ fine { ... on Either { ... on Fine { fine } } } }",
            "{ fine { ...E } } fragment E on Either { ... on Fine { fine } }",
        ] {
            assert_eq!(given(document, json!({})), None, "{document}");
        }
        assert_eq!(
            given(
                "{ either { ... on Either { ... on Fine { fine } } } }",
                json!({})
            )
            .as_deref(),
            Some("/either"),
            "an `Other` meets no condition"
        );
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
