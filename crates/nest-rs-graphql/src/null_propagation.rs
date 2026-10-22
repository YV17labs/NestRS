//! A field that errs answers `null` beside its error, and a Non-Null one hands
//! its null to the nearest nullable field above it — to `data` itself from a
//! root field: GraphQL (October 2021) §6.4.4, *Handling Field Errors*.
//!
//! async-graphql (`=7.2.1`, `do_resolve_container`) files a failed field's
//! error and inserts no key for it, and answers `null` for an object whose
//! every field failed. The nulls are placed once the operation answered, read
//! off the schema's types and the operation's selections, and only in an
//! answer whose errors name a path. Each error costs a walk down its own path,
//! never a pass over the other errors.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_graphql::futures_util::StreamExt;
use async_graphql::futures_util::stream::BoxStream;
use async_graphql::indexmap::IndexMap;
use async_graphql::parser::types::{
    BaseType, ExecutableDocument, OperationDefinition, OperationType, Selection, SelectionSet,
    Type, TypeKind, TypeSystemDefinition,
};
use async_graphql::parser::{parse_query, parse_schema};
use async_graphql::{Data, Executor, Name, PathSegment, Request, Response, ServerError, Value};

use crate::operation_units::selected_operation;

/// The schema `E`, each field that erred answering `null` where the
/// specification places it.
#[derive(Clone)]
pub(crate) struct NullPropagated<E> {
    executor: E,
    types: Arc<FieldTypes>,
}

impl<E> NullPropagated<E> {
    /// `executor`, its nulls placed by the types its schema's `sdl` declares.
    pub(crate) fn new(executor: E, sdl: &str) -> Self {
        Self {
            executor,
            types: Arc::new(FieldTypes::of(sdl)),
        }
    }
}

impl<E: Executor> Executor for NullPropagated<E> {
    async fn execute(&self, mut request: Request) -> Response {
        let kept = Kept::take(&mut request);
        let mut response = self.executor.execute(request).await;
        if let Some(mut kept) = kept {
            kept.place_nulls(&mut response, &self.types);
        }
        response
    }

    fn execute_stream(
        &self,
        mut request: Request,
        session_data: Option<Arc<Data>>,
    ) -> BoxStream<'static, Response> {
        let mut kept = Kept::take(&mut request);
        let types = Arc::clone(&self.types);
        self.executor
            .execute_stream(request, session_data)
            .map(move |mut response| {
                if let Some(kept) = &mut kept {
                    kept.place_nulls(&mut response, &types);
                }
                response
            })
            .boxed()
    }
}

/// The type of every output field the schema declares, by the type declaring
/// it, and the schema's roots: what a path's nullability is read off.
struct FieldTypes {
    query: Name,
    mutation: Name,
    subscription: Name,
    fields: HashMap<Name, HashMap<Name, Type>>,
}

impl FieldTypes {
    /// Read off `sdl`. One that does not parse declares no field, so each null
    /// stays at the field that erred; said at `warn`.
    fn of(sdl: &str) -> Self {
        let mut types = Self {
            query: Name::new("Query"),
            mutation: Name::new("Mutation"),
            subscription: Name::new("Subscription"),
            fields: HashMap::new(),
        };
        let document = match parse_schema(sdl) {
            Ok(document) => document,
            Err(error) => {
                tracing::warn!(
                    target: crate::TARGET,
                    error = %nest_rs_core::error_message(&error),
                    "graphql schema did not parse back from its SDL; a field that errs answers null where it erred",
                );
                return types;
            }
        };
        for definition in document.definitions {
            match definition {
                TypeSystemDefinition::Schema(schema) => {
                    let schema = schema.node;
                    if let Some(query) = schema.query {
                        types.query = query.node;
                    }
                    if let Some(mutation) = schema.mutation {
                        types.mutation = mutation.node;
                    }
                    if let Some(subscription) = schema.subscription {
                        types.subscription = subscription.node;
                    }
                }
                TypeSystemDefinition::Type(definition) => {
                    let definition = definition.node;
                    let fields = match definition.kind {
                        TypeKind::Object(object) => object.fields,
                        TypeKind::Interface(interface) => interface.fields,
                        _ => continue,
                    };
                    types
                        .fields
                        .entry(definition.name.node)
                        .or_default()
                        .extend(
                            fields
                                .into_iter()
                                .map(|field| (field.node.name.node, field.node.ty.node)),
                        );
                }
                TypeSystemDefinition::Directive(_) => {}
            }
        }
        types
    }

    fn root(&self, operation: OperationType) -> &str {
        match operation {
            OperationType::Query => &self.query,
            OperationType::Mutation => &self.mutation,
            OperationType::Subscription => &self.subscription,
        }
    }

    fn field(&self, parent: &str, name: &str) -> Option<&Type> {
        self.fields.get(parent)?.get(name)
    }
}

/// The document a request executes, kept for the nulls its answer may need.
struct Kept {
    /// The request's text, taken once its parse is cached: async-graphql
    /// (`=7.2.1`) executes that parse and never reads the text again, so only
    /// an answer carrying a field error pays for parsing it twice.
    text: String,
    /// The name the document declares the selected operation under.
    operation: Option<Name>,
    parsed: Option<ExecutableDocument>,
}

impl Kept {
    /// The document `request` executes, when it selects an operation.
    fn take(request: &mut Request) -> Option<Self> {
        let wanted = request.operation_name.take();
        let operation = request
            .parsed_query()
            .ok()
            .and_then(|document| selected_operation(document, wanted.as_deref()))
            .map(|(name, _)| name.cloned());
        request.operation_name = wanted;
        Some(Self {
            operation: operation?,
            text: std::mem::take(&mut request.query),
            parsed: None,
        })
    }

    /// `response`'s nulls, placed where its errors' paths say a field erred.
    fn place_nulls(&mut self, response: &mut Response, types: &FieldTypes) {
        if response.errors.iter().all(|error| error.path.is_empty()) {
            return;
        }
        let document: &ExecutableDocument = match &mut self.parsed {
            Some(document) => document,
            unparsed @ None => match parse_query(&self.text) {
                Ok(document) => unparsed.insert(document),
                Err(_) => {
                    tracing::warn!(
                        target: crate::TARGET,
                        errors = response.errors.len(),
                        "graphql document did not parse again; its field errors keep the answer async-graphql gave",
                    );
                    return;
                }
            },
        };
        let Some((_, operation)) = selected_operation(document, self.operation.as_deref()) else {
            return;
        };
        Selections::new(document, types, &operation.node)
            .place(&mut response.data, &response.errors);
    }
}

/// An operation's selections, merged by response key and read against the
/// schema's types as far as the errors' paths go into them.
struct Selections<'a> {
    document: &'a ExecutableDocument,
    types: &'a FieldTypes,
    /// The operation's root first, then each position read so far.
    positions: Vec<Position<'a>>,
}

/// The index of the operation's root in [`Selections::positions`].
const ROOT: usize = 0;

/// One position of the merged selections, list indices aside: every field
/// selected under one response key at one place.
#[derive(Default)]
struct Position<'a> {
    /// The type the first of those fields declares, when the schema says.
    ty: Option<&'a Type>,
    /// How many selections answer here: async-graphql (`=7.2.1`,
    /// `insert_value`) resolves each apart, and where one answers `null` and
    /// another an object, keeps whichever finished first.
    selections: usize,
    /// Each selection's own selection set, with the type it is selected on.
    sets: Vec<(&'a SelectionSet, Option<&'a str>)>,
    /// The positions below, by response key in the order the selections name
    /// them (§7.2.2), read the first time a path goes below.
    below: Option<IndexMap<&'a str, usize>>,
}

/// One position along an error's path.
#[derive(Clone, Copy)]
struct Step {
    non_null: bool,
    /// The merged selection answering it, when the selections hold it.
    position: Option<usize>,
}

/// Where one error's null goes.
struct Placement<'e> {
    /// The path down to the first position the answer lacks.
    path: &'e [PathSegment],
    /// Each position along `path`.
    steps: Vec<Step>,
    /// The length of the path to the position the null is placed at.
    at: usize,
}

impl Placement<'_> {
    /// The merged selection answering the position `len` long.
    fn position(&self, len: usize) -> Option<usize> {
        match len.checked_sub(1) {
            Some(index) => self.steps[index].position,
            None => Some(ROOT),
        }
    }
}

impl<'a> Selections<'a> {
    fn new(
        document: &'a ExecutableDocument,
        types: &'a FieldTypes,
        operation: &'a OperationDefinition,
    ) -> Self {
        let root = Position {
            selections: 1,
            sets: vec![(
                &operation.selection_set.node,
                Some(types.root(operation.ty)),
            )],
            ..Position::default()
        };
        Self {
            document,
            types,
            positions: vec![root],
        }
    }

    /// Each error's null placed in `data`, the shortest paths first, so a null
    /// placed above another error's position answers for both; errors that
    /// lack one position place one null.
    fn place(&mut self, data: &mut Value, errors: &[ServerError]) {
        let mut lacking = HashSet::new();
        let mut erred_in_null = HashMap::<PathKey<'_>, usize>::new();
        let mut paths = Vec::new();
        for error in errors {
            let Some((len, holder)) = unanswered(data, &error.path) else {
                continue;
            };
            let path = &error.path[..len];
            if !lacking.insert(PathKey(path)) {
                continue;
            }
            if let (Value::Null, [parent @ .., PathSegment::Field(_)]) = (holder, path) {
                *erred_in_null.entry(PathKey(parent)).or_default() += 1;
            }
            paths.push(path);
        }
        let mut placements: Vec<Placement<'_>> = paths
            .into_iter()
            .map(|path| {
                let steps = self.along(path);
                let at = nullable_at_or_above(&steps, path.len());
                Placement { path, steps, at }
            })
            .collect();
        placements.sort_by_key(|placement| placement.at);
        let mut placed = HashSet::new();
        let mut filled = HashMap::new();
        for placement in &placements {
            let mut at = placement.at;
            loop {
                let target = &placement.path[..at];
                let Some(holder) = at.checked_sub(1).map(|len| &target[..len]) else {
                    *data = Value::Null;
                    placed.insert(PathKey(target));
                    break;
                };
                if placed.contains(&PathKey(holder)) {
                    break;
                }
                let position = placement.position(holder.len());
                let whole = self.whole(position, erred_in_null.get(&PathKey(holder)));
                match put_null(data, target, whole) {
                    Put::Keyed(answered) => {
                        filled
                            .entry(PathKey(holder))
                            .or_insert((position, answered));
                    }
                    Put::Item => {}
                    Put::Covered => break,
                    // A list shorter than the index, a scalar on the path, or a
                    // `null` hiding another selection's answer: the nearest
                    // nullable parent holds the null.
                    Put::Refused => {
                        at = nullable_at_or_above(&placement.steps, holder.len());
                        continue;
                    }
                }
                placed.insert(PathKey(target));
                break;
            }
        }
        for (holder, (position, answered)) in filled {
            let below = position.and_then(|position| self.positions[position].below.as_ref());
            if let (Some(Value::Object(fields)), Some(below)) = (value_at(data, holder.0), below) {
                order_filled(fields, answered, below);
            }
        }
    }

    /// Each position along `path`, read off the fields the selections put
    /// there; a position whose type is not read is nullable, so a null stops
    /// there.
    fn along(&mut self, path: &[PathSegment]) -> Vec<Step> {
        let mut position = Some(ROOT);
        let mut ty: Option<&Type> = None;
        path.iter()
            .map(|segment| {
                match segment {
                    PathSegment::Field(key) => {
                        position = position.and_then(|position| self.below(position, key));
                        ty = position.and_then(|position| self.positions[position].ty);
                    }
                    PathSegment::Index(_) => {
                        ty = ty.and_then(|ty| match &ty.base {
                            BaseType::List(item) => Some(&**item),
                            BaseType::Named(_) => None,
                        });
                    }
                }
                Step {
                    non_null: ty.is_some_and(|ty| !ty.nullable),
                    position,
                }
            })
            .collect()
    }

    /// The position under `key` at `position`, its fields read once.
    fn below(&mut self, position: usize, key: &str) -> Option<usize> {
        if self.positions[position].below.is_none() {
            let mut below = IndexMap::new();
            for (set, parent) in std::mem::take(&mut self.positions[position].sets) {
                self.collect(set, parent, &mut below);
            }
            self.positions[position].below = Some(below);
        }
        self.positions[position].below.as_ref()?.get(key).copied()
    }

    /// Each field of `set` under its response key, through its fragments, with
    /// its type as the type it is selected on declares it. A document that
    /// executed passed validation, which refuses a fragment cycle.
    fn collect(
        &mut self,
        set: &'a SelectionSet,
        parent: Option<&'a str>,
        below: &mut IndexMap<&'a str, usize>,
    ) {
        let (document, types) = (self.document, self.types);
        for selection in &set.items {
            match &selection.node {
                Selection::Field(field) => {
                    let field = &field.node;
                    let ty = parent.and_then(|parent| types.field(parent, &field.name.node));
                    let index = *below
                        .entry(field.response_key().node.as_str())
                        .or_insert_with(|| {
                            self.positions.push(Position::default());
                            self.positions.len() - 1
                        });
                    let position = &mut self.positions[index];
                    position.ty = position.ty.or(ty);
                    position.selections += 1;
                    position
                        .sets
                        .push((&field.selection_set.node, ty.map(named)));
                }
                Selection::FragmentSpread(spread) => {
                    if let Some(fragment) = document.fragments.get(&spread.node.fragment_name.node)
                    {
                        let on = fragment.node.type_condition.node.on.node.as_str();
                        self.collect(&fragment.node.selection_set.node, Some(on), below);
                    }
                }
                Selection::InlineFragment(fragment) => {
                    let on = fragment
                        .node
                        .type_condition
                        .as_ref()
                        .map(|condition| condition.node.on.node.as_str())
                        .or(parent);
                    self.collect(&fragment.node.selection_set.node, on, below);
                }
            }
        }
    }

    /// Whether a `null` async-graphql answered at `position` stands for an
    /// object whose every field failed, and so may answer as that object.
    /// Selected more than once, it may instead have hidden another selection's
    /// answer, unless every key the selections name there erred (`erred`).
    fn whole(&self, position: Option<usize>, erred: Option<&usize>) -> bool {
        position.is_none_or(|position| {
            let position = &self.positions[position];
            position.selections <= 1 || position.below.as_ref().map(IndexMap::len) == erred.copied()
        })
    }
}

/// A path compared and hashed by its segments: async-graphql's `PathSegment`
/// derives no `Hash`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PathKey<'e>(&'e [PathSegment]);

impl Hash for PathKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.0.len());
        for segment in self.0 {
            match segment {
                PathSegment::Field(key) => key.hash(state),
                PathSegment::Index(index) => index.hash(state),
            }
        }
    }
}

/// The named type under `ty`'s lists.
fn named(ty: &Type) -> &str {
    match &ty.base {
        BaseType::Named(name) => name,
        BaseType::List(item) => named(item),
    }
}

/// The length of the path to the nearest nullable position at or above the
/// one `len` reaches: `0`, `data` itself, when every one is Non-Null.
fn nullable_at_or_above(steps: &[Step], len: usize) -> usize {
    steps[..len]
        .iter()
        .rposition(|step| !step.non_null)
        .map_or(0, |index| index + 1)
}

/// The length of the path to the first position along `path` that `data`
/// lacks, and the value lacking it; `None` when it answers the whole path.
fn unanswered<'d>(data: &'d Value, path: &[PathSegment]) -> Option<(usize, &'d Value)> {
    let mut value = data;
    for (depth, segment) in path.iter().enumerate() {
        let next = match (value, segment) {
            (Value::Object(fields), PathSegment::Field(key)) => fields.get(key.as_str()),
            (Value::List(items), PathSegment::Index(index)) => items.get(*index),
            _ => None,
        };
        match next {
            Some(next) => value = next,
            None => return Some((depth + 1, value)),
        }
    }
    None
}

/// The value at `path` in `data`.
fn value_at<'d>(data: &'d mut Value, path: &[PathSegment]) -> Option<&'d mut Value> {
    path.iter()
        .try_fold(data, |value, segment| match (value, segment) {
            (Value::Object(fields), PathSegment::Field(key)) => fields.get_mut(key.as_str()),
            (Value::List(items), PathSegment::Index(index)) => items.get_mut(*index),
            _ => None,
        })
}

/// Where [`put_null`] put a `null`, or why it put none.
#[derive(Debug, PartialEq, Eq)]
enum Put {
    /// Under its key, in an object that held this many keys before it.
    Keyed(usize),
    /// As an item of a list.
    Item,
    /// Nowhere: a `null` placed above it answers for it.
    Covered,
    /// Nowhere: no position along the path holds it.
    Refused,
}

/// Put `null` at `path` in `data`. A `null` met above the position holding it
/// is one an earlier, shorter path placed: the answer holds none above the first
/// position it lacks. A `null` async-graphql answered for an object whose every
/// field failed holds it as that object when `whole` says it is one.
fn put_null(data: &mut Value, path: &[PathSegment], whole: bool) -> Put {
    let Some((last, parents)) = path.split_last() else {
        return Put::Refused;
    };
    let mut value = data;
    for segment in parents {
        let next = match (value, segment) {
            (Value::Null, _) => return Put::Covered,
            (Value::Object(fields), PathSegment::Field(key)) => fields.get_mut(key.as_str()),
            (Value::List(items), PathSegment::Index(index)) => items.get_mut(*index),
            _ => None,
        };
        let Some(next) = next else {
            return Put::Refused;
        };
        value = next;
    }
    match (value, last) {
        (Value::Object(fields), PathSegment::Field(key)) => {
            let answered = fields.len();
            fields.insert(Name::new(key), Value::Null);
            Put::Keyed(answered)
        }
        (value @ Value::Null, PathSegment::Field(key)) if whole => {
            *value = Value::Object(IndexMap::from([(Name::new(key), Value::Null)]));
            Put::Keyed(0)
        }
        (Value::List(items), PathSegment::Index(index)) => match items.get_mut(*index) {
            Some(item) => {
                *item = Value::Null;
                Put::Item
            }
            None => Put::Refused,
        },
        _ => Put::Refused,
    }
}

/// The nulls put in `fields` after its first `answered` keys, each moved before
/// the first answered key the selections name after it (`below`, §7.2.2).
fn order_filled(
    fields: &mut IndexMap<Name, Value>,
    answered: usize,
    below: &IndexMap<&str, usize>,
) {
    if fields.len() == answered {
        return;
    }
    let rank = |key: &Name| below.get_index_of(key.as_str()).unwrap_or(usize::MAX);
    let mut nulls: Vec<(usize, Name, Value)> = fields
        .drain(answered..)
        .map(|(key, value)| (rank(&key), key, value))
        .collect();
    nulls.sort_by_key(|&(ranked, ..)| ranked);
    let mut nulls = nulls.into_iter().peekable();
    let capacity = fields.len() + nulls.len();
    for (key, value) in std::mem::replace(fields, IndexMap::with_capacity(capacity)) {
        let after = rank(&key);
        while let Some((_, null, value)) = nulls.next_if(|&(ranked, ..)| ranked < after) {
            fields.insert(null, value);
        }
        fields.insert(key, value);
    }
    fields.extend(nulls.map(|(_, key, value)| (key, value)));
}

#[cfg(test)]
mod tests {
    use async_graphql::futures_util::Stream;
    use async_graphql::futures_util::stream;
    use async_graphql::{Context, Interface, Object, Schema, SimpleObject, Subscription, Union};
    use serde_json::json;

    use super::*;

    #[derive(SimpleObject)]
    struct Fine {
        fine: i32,
    }

    struct Leaf;

    #[Object]
    impl Leaf {
        async fn fine(&self) -> i32 {
            1
        }

        async fn broken(&self) -> async_graphql::Result<Option<i32>> {
            Err("the nullable field failed".into())
        }

        async fn strict(&self) -> async_graphql::Result<i32> {
            Err("the non-null field failed".into())
        }

        async fn child(&self) -> Leaf {
            Leaf
        }
    }

    #[derive(Interface)]
    #[graphql(
        field(name = "fine", ty = "i32"),
        field(name = "strict", ty = "async_graphql::Result<i32>"),
        field(name = "child", ty = "Leaf")
    )]
    enum Node {
        Leaf(Leaf),
    }

    #[derive(Union)]
    enum Either {
        Leaf(Leaf),
        Fine(Fine),
    }

    struct Query;

    #[Object]
    impl Query {
        async fn broken(&self) -> async_graphql::Result<Option<i32>> {
            Err("the nullable root field failed".into())
        }

        async fn leaf(&self) -> Option<Leaf> {
            Some(Leaf)
        }

        async fn sure_leaf(&self) -> Leaf {
            Leaf
        }

        async fn leaves(&self) -> Option<Vec<Leaf>> {
            Some(vec![Leaf, Leaf])
        }

        async fn maybe_leaves(&self) -> Vec<Option<Leaf>> {
            vec![Some(Leaf)]
        }

        async fn many(&self, count: usize) -> Vec<Leaf> {
            (0..count).map(|_| Leaf).collect()
        }

        async fn grid(&self) -> Vec<Option<Vec<Leaf>>> {
            vec![Some(vec![Leaf]), Some(vec![Leaf])]
        }

        async fn node(&self) -> Option<Node> {
            Some(Node::Leaf(Leaf))
        }

        async fn either(&self) -> Option<Either> {
            Some(Either::Leaf(Leaf))
        }

        async fn counts(&self) -> Option<Vec<Result<i32, async_graphql::Error>>> {
            Some(vec![Ok(1), Err("the item failed".into()), Ok(3)])
        }

        /// Files an error at its own path and answers anyway.
        async fn partial(&self, ctx: &Context<'_>) -> Option<i32> {
            ctx.add_error(ctx.set_error_path(async_graphql::ServerError::new(
                "answered beside its error",
                None,
            )));
            Some(1)
        }
    }

    struct Writes;

    #[Object]
    impl Writes {
        async fn done(&self) -> i32 {
            1
        }

        async fn failed(&self) -> async_graphql::Result<Option<i32>> {
            Err("the write failed".into())
        }
    }

    struct Ticks;

    #[Subscription]
    impl Ticks {
        async fn ticks(&self) -> impl Stream<Item = Leaf> {
            stream::iter([Leaf])
        }
    }

    type Bare = Schema<Query, Writes, Ticks>;

    fn bare() -> Bare {
        Schema::new(Query, Writes, Ticks)
    }

    fn schema() -> NullPropagated<Bare> {
        let schema = bare();
        let sdl = schema.sdl();
        NullPropagated::new(schema, &sdl)
    }

    async fn answer(document: &str) -> serde_json::Value {
        let response = schema().execute(Request::new(document)).await;
        serde_json::to_value(&response).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Each document, and the `data` it answers.
    #[tokio::test]
    async fn each_null_lands_at_the_nearest_nullable_position() {
        for (document, data) in [
            // A Non-Null field under a Non-Null parent: the null climbs to `leaf`.
            ("{ leaf { fine strict } }", json!({ "leaf": null })),
            ("{ sureLeaf { fine strict } }", serde_json::Value::Null),
            // `[Leaf!]`: an item cannot be null, so the list is.
            ("{ leaves { strict } }", json!({ "leaves": null })),
            (
                "{ leaves { broken } }",
                json!({ "leaves": [{ "broken": null }, { "broken": null }] }),
            ),
            (
                "{ maybeLeaves { strict } }",
                json!({ "maybeLeaves": [null] }),
            ),
            // `[[Leaf!]]!`: each inner list is nullable.
            ("{ grid { fine strict } }", json!({ "grid": [null, null] })),
            // `child` is read off the interface it is selected on: `Leaf!`.
            (
                "{ node { fine child { strict } } }",
                json!({ "node": null }),
            ),
            (
                "{ node { ... on Leaf { broken } } }",
                json!({ "node": { "broken": null } }),
            ),
            (
                "{ either { ... on Leaf { s: strict } ... on Fine { fine } } }",
                json!({ "either": null }),
            ),
            // async-graphql drops the whole list an item of fails: the list answers it.
            ("{ counts }", json!({ "counts": null })),
            // A resolver answering beside its error keeps its answer.
            ("{ partial }", json!({ "partial": 1 })),
        ] {
            let answer = answer(document).await;
            assert_eq!(answer["data"], data, "{document}: {answer}");
            assert!(answer["errors"].is_array(), "{document}: {answer}");
        }
    }

    /// async-graphql (`=7.2.1`, `insert_value`) keeps the first of a field's
    /// selections to finish: when that one's every field failed, its `null`
    /// hides the others' answers, so it stays a `null` — climbing past a
    /// Non-Null position — never an object missing what they answered.
    #[tokio::test]
    async fn a_null_hiding_another_selections_answer_is_never_rebuilt_without_it() {
        for (document, data) in [
            (
                "{ maybeLeaves { broken } maybeLeaves { fine } }",
                json!({ "maybeLeaves": [null] }),
            ),
            (
                "{ ...A ...B } \
                 fragment A on Query { maybeLeaves { broken } } \
                 fragment B on Query { maybeLeaves { fine } }",
                json!({ "maybeLeaves": [null] }),
            ),
            ("{ leaf { broken } leaf { fine } }", json!({ "leaf": null })),
            // `[Leaf!]`: the item cannot hold it, so the list does.
            (
                "{ leaves { broken } leaves { fine } }",
                json!({ "leaves": null }),
            ),
            // The other order: the answered selection finished first.
            (
                "{ maybeLeaves { fine } maybeLeaves { broken } }",
                json!({ "maybeLeaves": [{ "fine": 1, "broken": null }] }),
            ),
            (
                "{ ...B ...A } \
                 fragment A on Query { maybeLeaves { broken } } \
                 fragment B on Query { maybeLeaves { fine } }",
                json!({ "maybeLeaves": [{ "fine": 1, "broken": null }] }),
            ),
            // Every key the selections name erred: nothing was hidden.
            (
                "{ leaf { broken } leaf { b: broken } }",
                json!({ "leaf": { "broken": null, "b": null } }),
            ),
        ] {
            let answer = answer(document).await;
            assert_eq!(answer["data"], data, "{document}: {answer}");
            assert!(answer["errors"].is_array(), "{document}: {answer}");
        }
    }

    /// A placed `null` answers where the selections name its key (§7.2.2,
    /// *Serialized Map Ordering*), never after the keys that answered.
    #[tokio::test]
    async fn a_placed_null_answers_in_the_order_the_selections_name_it() {
        for (document, text) in [
            (
                "mutation { a: failed b: done c: done }",
                r#"{"a":null,"b":1,"c":1}"#,
            ),
            (
                "{ broken leaf { fine } }",
                r#"{"broken":null,"leaf":{"fine":1}}"#,
            ),
            (
                "{ leaf { broken fine b: broken } }",
                r#"{"leaf":{"broken":null,"fine":1,"b":null}}"#,
            ),
            (
                "{ leaf { b: broken a: broken } }",
                r#"{"leaf":{"b":null,"a":null}}"#,
            ),
        ] {
            let response = schema().execute(Request::new(document)).await;
            let data =
                serde_json::to_string(&response.data).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(data, text, "{document}");
        }
    }

    /// Placing the nulls costs what async-graphql paid to file the errors, not
    /// their square: a few thousand aliases of a failing field over a list,
    /// within the default complexity, once took seconds.
    #[tokio::test]
    async fn the_nulls_cost_what_filing_their_errors_did() {
        let (bare, schema) = (bare(), schema());
        let aliases: String = (0..1000)
            .map(|index| format!("a{index}: broken "))
            .collect();
        let repeats = "broken ".repeat(1000);
        for selection in [aliases, repeats] {
            let document = format!("{{ many(count: 20) {{ fine {selection}}} }}");
            let started = std::time::Instant::now();
            let response = bare.execute(Request::new(document.as_str())).await;
            let filed = started.elapsed();
            let started = std::time::Instant::now();
            let placed = schema.execute(Request::new(document.as_str())).await;
            let placing = started.elapsed();
            assert_eq!(placed.errors.len(), response.errors.len());
            assert!(
                placing < filed * 20,
                "{} errors filed in {filed:?}, answered with their nulls in {placing:?}",
                response.errors.len(),
            );
        }
    }

    /// async-graphql (`=7.2.1`)'s `Interface` derive files the error of a field
    /// resolved through the interface without its path, so no null is placed
    /// for it: when this fails, a release gave it one.
    #[tokio::test]
    async fn an_interface_fields_own_error_names_no_path() {
        let answer = answer("{ node { fine strict } }").await;
        assert_eq!(answer["data"], json!({ "node": { "fine": 1 } }), "{answer}");
        assert!(answer["errors"][0].get("path").is_none(), "{answer}");
    }

    #[tokio::test]
    async fn a_named_operation_is_read_by_its_own_selections() {
        let request = Request::new("query A { leaf { strict } } query B { leaf { broken } }")
            .operation_name("B");
        let response = schema().execute(request).await;
        let answer = serde_json::to_value(&response).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            answer["data"],
            json!({ "leaf": { "broken": null } }),
            "{answer}"
        );
    }

    #[tokio::test]
    async fn an_answer_without_errors_is_left_as_answered() {
        let answer = answer("{ leaf { fine } }").await;
        assert_eq!(answer, json!({ "data": { "leaf": { "fine": 1 } } }));
    }

    #[tokio::test]
    async fn each_item_of_a_stream_answers_its_nulls() {
        for (document, data) in [
            (
                "subscription { ticks { fine broken } }",
                json!({ "ticks": { "fine": 1, "broken": null } }),
            ),
            (
                "subscription { ticks { fine strict } }",
                serde_json::Value::Null,
            ),
        ] {
            let items: Vec<Response> = schema()
                .execute_stream(Request::new(document), None)
                .collect()
                .await;
            let items = serde_json::to_value(&items).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(items[0]["data"], data, "{document}: {items}");
        }
    }

    /// An answer async-graphql never gives — an index past its list — is still
    /// answered by the specification's rule, one position up.
    #[test]
    fn a_null_the_answer_cannot_hold_climbs_to_the_nearest_nullable_parent() {
        let types = FieldTypes::of(&bare().sdl());
        let document = parse_query("{ fine: leaf { fine } maybeLeaves { fine } }")
            .unwrap_or_else(|error| panic!("{error}"));
        let Some((_, operation)) = selected_operation(&document, None) else {
            panic!("the document selects its operation");
        };
        let error = |segments| {
            let mut error = ServerError::new("failed", None);
            error.path = path(segments);
            error
        };
        let mut data = value(json!({ "fine": { "fine": 1 }, "maybeLeaves": [{ "fine": 1 }] }));
        Selections::new(&document, &types, &operation.node)
            .place(&mut data, &[error(json!(["maybeLeaves", 5]))]);
        assert_eq!(data, Value::Null, "`[Leaf]!` hands it to `data`");
    }

    #[test]
    fn the_roots_and_fields_are_read_off_the_schemas_sdl() {
        let types = FieldTypes::of(
            "schema { query: Root mutation: Writes }\n\
             type Root { a: Int! }\n\
             extend type Root { b: [Int] }\n\
             interface Named { name: String! }\n\
             type Writes { w: Int }",
        );
        assert_eq!(types.root(OperationType::Query), "Root");
        assert_eq!(types.root(OperationType::Mutation), "Writes");
        assert_eq!(types.root(OperationType::Subscription), "Subscription");
        assert_eq!(
            types.field("Root", "a").map(ToString::to_string).as_deref(),
            Some("Int!")
        );
        assert_eq!(
            types.field("Root", "b").map(ToString::to_string).as_deref(),
            Some("[Int]")
        );
        assert_eq!(
            types
                .field("Named", "name")
                .map(ToString::to_string)
                .as_deref(),
            Some("String!")
        );
        assert!(types.field("Root", "c").is_none());
    }

    #[test]
    fn an_sdl_that_does_not_parse_reads_no_field_and_says_so() {
        let logs = nest_rs_testing::LogCapture::install();
        let types = FieldTypes::of("type {");
        assert!(types.fields.is_empty());
        assert_eq!(types.root(OperationType::Query), "Query");
        let line = logs.expect_one(
            crate::TARGET,
            "graphql schema did not parse back from its SDL; a field that errs answers null where it erred",
        );
        assert_eq!(line.level, "warn");
        assert!(line.field("error").is_some(), "{:?}", line.fields);
    }

    #[test]
    fn the_nearest_nullable_position_is_read_upwards() {
        let steps = |non_null: &[bool]| -> Vec<Step> {
            non_null
                .iter()
                .map(|&non_null| Step {
                    non_null,
                    position: None,
                })
                .collect()
        };
        assert_eq!(nullable_at_or_above(&steps(&[false, true, true]), 3), 1);
        assert_eq!(nullable_at_or_above(&steps(&[false, true, false]), 3), 3);
        assert_eq!(nullable_at_or_above(&steps(&[true, true]), 2), 0);
        assert_eq!(nullable_at_or_above(&steps(&[false, false]), 1), 1);
        assert_eq!(nullable_at_or_above(&steps(&[]), 0), 0);
    }

    fn path(segments: serde_json::Value) -> Vec<PathSegment> {
        serde_json::from_value(segments).unwrap_or_else(|error| panic!("{error}"))
    }

    fn value(json: serde_json::Value) -> Value {
        Value::from_json(json).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn the_answer_stops_at_the_first_position_it_lacks() {
        let data = value(json!({ "a": { "b": [{ "c": 1 }] }, "n": null }));
        let lacking = |segments| unanswered(&data, &path(segments)).map(|(len, _)| len);
        assert_eq!(lacking(json!(["a", "x"])), Some(2));
        assert_eq!(lacking(json!(["a", "b", 0, "d"])), Some(4));
        assert_eq!(lacking(json!(["a", "b", 3, "c"])), Some(3));
        assert_eq!(lacking(json!(["n", "x"])), Some(2));
        assert_eq!(lacking(json!(["x", "y"])), Some(1));
        assert_eq!(lacking(json!(["a", "b", 0, "c"])), None);
        assert_eq!(
            unanswered(&data, &path(json!(["n", "x"]))).map(|(_, holder)| holder),
            Some(&Value::Null)
        );
    }

    #[test]
    fn a_null_goes_where_the_path_leads_or_nowhere() {
        let mut data = value(json!({ "a": { "b": [{ "c": 1 }] }, "n": null, "h": null }));
        let mut put = |segments, whole| put_null(&mut data, &path(segments), whole);
        assert_eq!(put(json!(["a", "x"]), false), Put::Keyed(1));
        assert_eq!(put(json!(["a", "b", 0]), false), Put::Item);
        assert_eq!(put(json!(["n", "x"]), true), Put::Keyed(0));
        assert_eq!(put(json!(["a", "b", 0, "c", "d"]), true), Put::Covered);
        assert_eq!(put(json!(["h", "x"]), false), Put::Refused);
        assert_eq!(put(json!(["a", "b", 5]), true), Put::Refused);
        assert_eq!(put(json!(["z", "y"]), true), Put::Refused);
        assert_eq!(put(json!([0]), true), Put::Refused);
        assert_eq!(put_null(&mut data, &[], true), Put::Refused);
        assert_eq!(
            data.into_json().unwrap_or_else(|error| panic!("{error}")),
            json!({ "a": { "b": [null], "x": null }, "n": { "x": null }, "h": null })
        );
    }
}
