//! What a document costs, counted before anything walks it.
//!
//! async-graphql (`7.2.1`) pays a fragment's whole expansion again at each of its
//! spreads, in `check_recursive_depth` and in every visitor `check_rules` inlines,
//! and only then scores the document: `...F1 ...F1` at each of `n` levels costs
//! `2^n` on the thread, which no deadline reaches, for a few bytes a level, and a
//! `__typename` or an unknown fragment at the bottom scores nothing at all. Its
//! rules recurse through a fragment no operation spreads, which its recursion bound
//! never reads, only to refuse it. `OverlappingFieldsCanBeMerged` looks each argument
//! of a repeated response key up in the other's, a scan, so a key holding thousands
//! costs their square. And an argument is built by copying a variable's value at each
//! reference, once the reference has scanned the operation's definitions for its own,
//! so `[$v, $v, …]` costs the references times the value and the definitions, which
//! no score counts.
//!
//! So each fragment and each variable is read here once. A spread of a fragment
//! already read adds what it expands to, a field its arguments squared, a reference
//! what it copies and scans: the walks ahead cost the document's text plus those
//! sums, each held to a limit. A document spreading each fragment once adds no
//! selection, so a field the schema scores below one stays the visitor's to score. A
//! fragment nothing reached is refused here, as validation would.
//!
//! A field below a list reads its arguments again at each row, copying a literal
//! and a variable's value each time (`Context::get_param_value`'s `.cloned()`,
//! `Context::var_value`'s), every row's copy held at once while the rows resolve
//! together. How many rows a list answers is known only once it ran, so what those
//! fields read is held per row, before anything runs: what stands below a field at
//! all is read without the schema, and only past the bound are the schema's types
//! read for which of those fields a list stands above
//! (`.claude/decisions/graphql-transactions.md`).

use std::collections::HashMap;

use async_graphql::parser::types::{
    Directive, ExecutableDocument, Field, FragmentDefinition, OperationType, Selection,
    SelectionSet,
};
use async_graphql::registry::{MetaTypeName, Registry};
use async_graphql::{Name, Pos, Positioned, ServerError, Value, Variables};
use async_graphql_value::Value as Written;

use crate::operation_units::{RECURSIVE_DEPTH, is_skipped, selected_operation};

/// async-graphql's (`7.2.1`) refusal of a document past `max_complexity`, which a
/// document past a limit here answers word for word.
const TOO_COMPLEX: &str = "Query is too complex.";

/// What a document may add to its text.
#[derive(Clone, Copy)]
pub(crate) struct DocumentLimits {
    /// What spreading a fragment again may add, in selections.
    pub(crate) repeated_selections: usize,
    /// What the arguments it runs may copy, and what its repeated keys may scan, each
    /// on its own, a step of a scan counting as a byte: as its fragments expand them
    /// and its variables fill them.
    pub(crate) copied_bytes: usize,
    /// What the arguments of the fields a list stands above may copy, which each of
    /// its rows copies again; `usize::MAX` when off.
    pub(crate) list_argument_bytes: usize,
}

/// The refusal of `document`, should it cost past `limits`, nest past what
/// async-graphql runs, or hold a fragment nothing reaches. `operation` is the name
/// the request selects its operation by, if any, and `registry` the schema's types,
/// read only for what a list's rows copy past its bound.
pub(crate) fn refusal(
    document: &ExecutableDocument,
    operation: Option<&str>,
    variables: &Variables,
    limits: DocumentLimits,
    registry: &Registry,
) -> Option<ServerError> {
    let mut cost = DocumentCost::new(document, operation, variables, limits);
    let refused = cost
        .of_document(document)
        .and_then(|()| cost.of_rows(document, variables, registry));
    refused.err().map(ServerError::from)
}

/// Why a count stopped, each said in async-graphql's (`7.2.1`) own words.
#[derive(Debug)]
enum Past<'d> {
    /// The document costs more than a limit.
    Limit,
    /// A selection set stands deeper than [`RECURSIVE_DEPTH`].
    Depth(Pos),
    /// No operation reaches this fragment.
    Unspread(&'d str, Pos),
}

impl From<Past<'_>> for ServerError {
    fn from(past: Past<'_>) -> Self {
        match past {
            Past::Limit => Self::new(TOO_COMPLEX, None),
            Past::Depth(at) => Self::new(
                format!(
                    "The recursion depth of the query cannot be greater than `{RECURSIVE_DEPTH}`"
                ),
                Some(at),
            ),
            Past::Unspread(name, at) => {
                Self::new(format!(r#"Fragment "{name}" is never used"#), Some(at))
            }
        }
    }
}

/// What a selection set, or a fragment, holds once expanded.
#[derive(Clone, Copy, Default)]
struct Weight {
    selections: usize,
    /// What its repeated keys' arguments scan, whichever operation runs.
    scanned: usize,
    /// What its references and its text copy, should an operation run it.
    copied: usize,
    /// Of what it copies, what stands below a field of its own: all a list's rows
    /// could copy again, whichever of those fields a list stands above.
    below: usize,
}

impl Weight {
    fn add(&mut self, other: Self) {
        self.selections = self.selections.saturating_add(other.selections);
        self.scanned = self.scanned.saturating_add(other.scanned);
        self.copied = self.copied.saturating_add(other.copied);
        self.below = self.below.saturating_add(other.below);
    }

    /// What it holds as a field's selection set, every copy of which stands below
    /// that field.
    fn below_a_field(self) -> Self {
        Self {
            below: self.copied,
            ..self
        }
    }
}

/// One document's cost, as far as it was read.
struct DocumentCost<'d> {
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    limits: DocumentLimits,
    /// The name the request selects its operation by.
    operation: Option<&'d str>,
    /// What each fragment read so far holds.
    read: HashMap<&'d str, Weight>,
    /// What one reference to each variable costs.
    references: HashMap<&'d str, usize>,
    /// Whether a value can cost past its text: with no fragment to repeat it and no
    /// variable to copy, what the document copies is its text, which the request's
    /// own cap holds.
    multiplies: bool,
    /// Whether what is being read stands in a fragment.
    in_fragment: bool,
    /// What the arguments written in fragments copy, each read once.
    in_fragments: usize,
    /// What the spreads of a fragment already read added.
    repeated: usize,
    /// What the repeated keys read so far scan.
    scanned: usize,
    /// What the arguments the document runs copy, once it was read.
    copied: usize,
    /// What the arguments of the operations it runs copy below a field.
    below: usize,
}

impl<'d> DocumentCost<'d> {
    fn new(
        document: &'d ExecutableDocument,
        operation: Option<&'d str>,
        variables: &'d Variables,
        limits: DocumentLimits,
    ) -> Self {
        let references = references(document, operation, variables);
        Self {
            fragments: &document.fragments,
            limits,
            operation,
            read: HashMap::with_capacity(document.fragments.len()),
            multiplies: !document.fragments.is_empty() || !references.is_empty(),
            references,
            in_fragment: false,
            in_fragments: 0,
            repeated: 0,
            scanned: 0,
            copied: 0,
            below: 0,
        }
    }

    /// Count every operation, async-graphql's walks reading each whichever one the
    /// request selects. The first fragment left unread, by its place in the text, is
    /// what the count stops at: every other walk stays on what an operation reaches.
    ///
    /// An operation a name leaves out copies nothing of its own: `ArgumentsOfCorrectType`
    /// skips it and nothing runs it. That rule copies a fragment's own arguments once
    /// whoever spreads it, so what is held is the larger of that and what the
    /// operations run.
    fn of_document(&mut self, document: &'d ExecutableDocument) -> Result<(), Past<'d>> {
        let mut runs = 0_usize;
        for (name, operation) in document.operations.iter() {
            let mut weight = self.of_set(&operation.node.selection_set, 0)?;
            weight.add(self.of_arguments(&operation.node.directives, None)?);
            let selected = self
                .operation
                .is_none_or(|wanted| name.is_some_and(|name| name.as_str() == wanted));
            if selected {
                runs = runs.saturating_add(weight.copied);
                self.below = self.below.max(weight.below);
            }
        }
        self.copied = runs.max(self.in_fragments);
        if self.read.len() != self.fragments.len() {
            let unread = self
                .fragments
                .iter()
                .filter(|(name, _)| !self.read.contains_key(name.as_str()))
                .min_by_key(|(_, fragment)| fragment.pos);
            if let Some((name, fragment)) = unread {
                return Err(Past::Unspread(name, fragment.pos));
            }
        }
        if self.multiplies && self.copied > self.limits.copied_bytes {
            return Err(Past::Limit);
        }
        Ok(())
    }

    /// Whether what the rows of a list copy stays within its bound, once the document
    /// was read: what stands below a field at all bounds it, and only past the bound
    /// is the schema read for the fields a list stands above, in the operation the
    /// request runs.
    fn of_rows(
        &self,
        document: &'d ExecutableDocument,
        variables: &'d Variables,
        registry: &Registry,
    ) -> Result<(), Past<'d>> {
        let bound = self.limits.list_argument_bytes;
        if self.below <= bound {
            return Ok(());
        }
        let Some((_, operation)) = selected_operation(document, self.operation) else {
            return Ok(());
        };
        let root = match operation.node.ty {
            OperationType::Query => Some(registry.query_type.as_str()),
            OperationType::Mutation => registry.mutation_type.as_deref(),
            OperationType::Subscription => registry.subscription_type.as_deref(),
        };
        let Some(root) = root else {
            return Ok(());
        };
        let mut rows = Rows {
            fragments: self.fragments,
            references: &self.references,
            variables,
            registry,
            read: HashMap::new(),
        };
        match rows.of_set(&operation.node.selection_set.node, root, false) > bound {
            true => Err(Past::Limit),
            false => Ok(()),
        }
    }

    /// What `set`, `depth` selection sets down, holds: a field counts one and what it
    /// selects, a fragment what it holds. Only a leaf's set is empty, so a count of
    /// none hides no walk.
    fn of_set(
        &mut self,
        set: &'d Positioned<SelectionSet>,
        depth: usize,
    ) -> Result<Weight, Past<'d>> {
        // A leaf's empty set is no level: async-graphql goes no deeper.
        if set.node.items.is_empty() {
            return Ok(Weight::default());
        }
        if depth > RECURSIVE_DEPTH {
            return Err(Past::Depth(set.pos));
        }
        let mut weight = Weight::default();
        for selection in &set.node.items {
            let (selected, field) = match &selection.node {
                Selection::Field(field) => (
                    self.of_set(&field.node.selection_set, depth + 1)?
                        .below_a_field(),
                    Some(&field.node),
                ),
                Selection::InlineFragment(fragment) => {
                    (self.of_set(&fragment.node.selection_set, depth + 1)?, None)
                }
                Selection::FragmentSpread(spread) => (
                    self.of_spread(spread.node.fragment_name.node.as_str(), depth)?,
                    None,
                ),
            };
            weight.add(selected);
            weight.add(self.of_arguments(selection.node.directives(), field)?);
            weight.selections = weight
                .selections
                .saturating_add(usize::from(field.is_some()));
        }
        Ok(weight)
    }

    /// What a spread `depth` selection sets down holds. A spread of a fragment no
    /// document defines, or of one still being read, a cycle, is one selection:
    /// validation refuses both, after the walks they cost.
    fn of_spread(&mut self, name: &'d str, depth: usize) -> Result<Weight, Past<'d>> {
        if let Some(weight) = self.read.get(name).copied() {
            self.repeated = self.repeated.saturating_add(weight.selections);
            self.scanned = self.scanned.saturating_add(weight.scanned);
            self.within_limits()?;
            return Ok(weight);
        }
        let one = Weight {
            selections: 1,
            ..Weight::default()
        };
        let Some(fragment) = self.fragments.get(name) else {
            return Ok(one);
        };
        self.read.insert(name, one);
        let outer = std::mem::replace(&mut self.in_fragment, true);
        let mut weight = self.of_set(&fragment.node.selection_set, depth + 1)?;
        weight.add(self.of_arguments(&fragment.node.directives, None)?);
        self.in_fragment = outer;
        self.read.insert(name, weight);
        Ok(weight)
    }

    /// What the arguments of `directives` and of `field` cost. A field's scan each
    /// other wherever its response key repeats, and telling that apart would cost
    /// every selection set a table: no field of a schema holds enough for it to matter.
    fn of_arguments(
        &mut self,
        directives: &[Positioned<Directive>],
        field: Option<&Field>,
    ) -> Result<Weight, Past<'d>> {
        let held = field.map_or(0, |field| field.arguments.len());
        let scanned = held.saturating_mul(held);
        self.scanned = self.scanned.saturating_add(scanned);
        self.within_limits()?;
        let written = directives
            .iter()
            .flat_map(|directive| &directive.node.arguments);
        let copied = copies(
            &self.references,
            field
                .into_iter()
                .flat_map(|field| &field.arguments)
                .chain(written),
        );
        if self.in_fragment {
            self.in_fragments = self.in_fragments.saturating_add(copied);
        }
        Ok(Weight {
            selections: 0,
            scanned,
            copied,
            below: 0,
        })
    }

    fn within_limits(&self) -> Result<(), Past<'d>> {
        if self.repeated > self.limits.repeated_selections
            || self.scanned > self.limits.copied_bytes
        {
            return Err(Past::Limit);
        }
        Ok(())
    }
}

/// What the fields a list stands above copy at each of its rows, read against the
/// schema's types once the count found the document within every other limit, so
/// each fragment is read at most twice, as it stands below a list or does not.
struct Rows<'a, 'd> {
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    references: &'a HashMap<&'d str, usize>,
    variables: &'d Variables,
    registry: &'a Registry,
    /// What each fragment read so far copies at a row, by whether a list stands above
    /// its spread; one still being read, a cycle validation refuses, copies nothing.
    read: HashMap<(&'d str, bool), usize>,
}

impl<'d> Rows<'_, 'd> {
    /// What `set`, selected on `parent`, copies at each row: everything when `listed`,
    /// a list standing above it, and otherwise what stands below a field of a list's
    /// type within it. A selection `@skip` or `@include` leaves out runs nothing, and
    /// those two are read once, before anything runs, so their own arguments copy
    /// nothing at a row. A field the schema does not declare is validation's to refuse.
    fn of_set(&mut self, set: &'d SelectionSet, parent: &str, listed: bool) -> usize {
        let mut copied = 0_usize;
        for selection in &set.items {
            if is_skipped(selection.node.directives(), self.variables) {
                continue;
            }
            let read = match &selection.node {
                Selection::Field(field) => {
                    let field = &field.node;
                    let Some(ty) = self
                        .registry
                        .types
                        .get(parent)
                        .and_then(|ty| ty.field_by_name(&field.name.node))
                        .map(|declared| declared.ty.as_str())
                    else {
                        continue;
                    };
                    let own = match listed {
                        true => {
                            let read = field
                                .directives
                                .iter()
                                .filter(|directive| {
                                    !matches!(directive.node.name.node.as_str(), "skip" | "include")
                                })
                                .flat_map(|directive| &directive.node.arguments);
                            copies(self.references, field.arguments.iter().chain(read))
                        }
                        false => 0,
                    };
                    let below = self.of_set(
                        &field.selection_set.node,
                        MetaTypeName::concrete_typename(ty),
                        listed || MetaTypeName::create(ty).is_list(),
                    );
                    own.saturating_add(below)
                }
                Selection::InlineFragment(fragment) => {
                    let on = fragment
                        .node
                        .type_condition
                        .as_ref()
                        .map_or(parent, |condition| condition.node.on.node.as_str());
                    self.of_set(&fragment.node.selection_set.node, on, listed)
                }
                Selection::FragmentSpread(spread) => {
                    self.of_spread(spread.node.fragment_name.node.as_str(), listed)
                }
            };
            copied = copied.saturating_add(read);
        }
        copied
    }

    fn of_spread(&mut self, name: &'d str, listed: bool) -> usize {
        if let Some(read) = self.read.get(&(name, listed)) {
            return *read;
        }
        let Some(fragment) = self.fragments.get(name) else {
            return 0;
        };
        self.read.insert((name, listed), 0);
        let on = fragment.node.type_condition.node.on.node.as_str();
        let read = self.of_set(&fragment.node.selection_set.node, on, listed);
        self.read.insert((name, listed), read);
        read
    }
}

/// What `arguments` copy where async-graphql reads them: what each value weighs as
/// text, a reference what its variable costs, beside the `null` its place in the
/// value would weigh.
fn copies<'v>(
    references: &HashMap<&str, usize>,
    arguments: impl Iterator<Item = &'v (Positioned<Name>, Positioned<Written>)>,
) -> usize {
    arguments.fold(0, |copied, (_, value)| {
        copied.saturating_add(weight_of(&value.node, references))
    })
}

/// What `value`, as a document writes it, weighs as [`size_of`] counts it, each
/// variable weighing what one reference to it costs, beside the `null` it stands for.
fn weight_of(value: &Written, references: &HashMap<&str, usize>) -> usize {
    match value {
        Written::Variable(name) => references
            .get(name.as_str())
            .copied()
            .unwrap_or(0)
            .saturating_add(1),
        Written::Null | Written::Boolean(_) | Written::Number(_) => 1,
        Written::String(text) => text.len(),
        Written::Enum(name) => name.len(),
        Written::Binary(bytes) => bytes.len(),
        Written::List(items) => items.iter().fold(1_usize, |weight, item| {
            weight.saturating_add(weight_of(item, references).saturating_add(1))
        }),
        Written::Object(members) => members.iter().fold(1_usize, |weight, (key, member)| {
            weight.saturating_add(
                key.len()
                    .saturating_add(weight_of(member, references))
                    .saturating_add(2),
            )
        }),
    }
}

/// What one reference to each variable costs: what its value weighs, and the steps
/// `Context::var_value` scans the selected operation's definitions in, down to its
/// own. A variable no definition names copies at validation all the same, and one
/// no value fills scans all the same.
fn references<'d>(
    document: &'d ExecutableDocument,
    operation: Option<&str>,
    variables: &'d Variables,
) -> HashMap<&'d str, usize> {
    let mut costs: HashMap<&'d str, usize> = variables
        .iter()
        .map(|(name, value)| (name.as_str(), size_of(value)))
        .collect();
    if let Some((_, selected)) = selected_operation(document, operation) {
        for (at, definition) in selected.node.variable_definitions.iter().enumerate() {
            *costs.entry(definition.node.name.node.as_str()).or_default() += at + 1;
        }
    }
    costs
}

/// What `value` weighs at least as text, in a variable's JSON as in a document,
/// what stands between its members included. No request carrying it carries less, so
/// a limit in what a request may carry is never spent by counting past the text.
fn size_of(value: &Value) -> usize {
    match value {
        Value::Null | Value::Boolean(_) | Value::Number(_) => 1,
        Value::String(text) => text.len(),
        Value::Enum(name) => name.len(),
        Value::Binary(bytes) => bytes.len(),
        // Its opening, and what closes or follows each member.
        Value::List(items) => 1 + items.iter().map(|item| size_of(item) + 1).sum::<usize>(),
        Value::Object(members) => {
            1 + members
                .iter()
                .map(|(key, member)| key.len() + 1 + size_of(member) + 1)
                .sum::<usize>()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::time::Duration;

    use async_graphql::parser::parse_query;
    use async_graphql::{EmptyMutation, EmptySubscription, Object, OutputType, Request, Schema};

    use super::*;
    use crate::testing::thread_cpu_time;

    const NO_COPIES_COUNTED: usize = usize::MAX;

    /// What [`size_of`] counts of a `null`, which is what a reference leaves in a value.
    const NULL: usize = 1;

    /// What the first definition's reference scans.
    const FIRST: usize = 1;

    /// What counting `document` under `repeated` selections stops at, held as what a
    /// test compares, or what its fragments' repeats added when it runs to its end.
    fn counted(document: &str, repeated: usize) -> Result<usize, String> {
        let document = parse_query(document).expect("the document parses");
        let variables = Variables::default();
        let limits = DocumentLimits {
            repeated_selections: repeated,
            copied_bytes: NO_COPIES_COUNTED,
            list_argument_bytes: usize::MAX,
        };
        let mut cost = DocumentCost::new(&document, None, &variables, limits);
        match cost.of_document(&document) {
            Ok(()) => Ok(cost.repeated),
            Err(past) => Err(format!("{past:?}")),
        }
    }

    /// What a count past its limit stops with.
    fn past_limit() -> Result<usize, String> {
        Err(format!("{:?}", Past::Limit))
    }

    /// The [`refusal`] of `document`, read against a schema of no types, which only
    /// a list's rows past their bound read, the request naming no operation.
    fn schemaless_refusal(
        document: &ExecutableDocument,
        variables: &Variables,
        limits: DocumentLimits,
    ) -> Option<ServerError> {
        refusal(document, None, variables, limits, &Registry::default())
    }

    /// What `document`'s arguments copy, given `$v` of `size` bytes.
    fn copied(document: &str, size: usize) -> usize {
        copied_selecting(document, None, size)
    }

    /// What `document`'s arguments copy, its operation selected by `name`.
    fn copied_selecting(document: &str, name: Option<&str>, size: usize) -> usize {
        let document = parse_query(document).expect("the document parses");
        let variables = variables(size);
        let limits = DocumentLimits {
            repeated_selections: usize::MAX,
            copied_bytes: NO_COPIES_COUNTED,
            list_argument_bytes: usize::MAX,
        };
        let mut cost = DocumentCost::new(&document, name, &variables, limits);
        cost.of_document(&document).expect("nothing past a limit");
        cost.copied
    }

    /// What `document`'s repeated keys scan, whichever operation a request selects.
    fn scanned(document: &str, name: Option<&str>) -> usize {
        let document = parse_query(document).expect("the document parses");
        let variables = Variables::default();
        let limits = DocumentLimits {
            repeated_selections: usize::MAX,
            copied_bytes: NO_COPIES_COUNTED,
            list_argument_bytes: usize::MAX,
        };
        let mut cost = DocumentCost::new(&document, name, &variables, limits);
        cost.of_document(&document).expect("nothing past a limit");
        cost.scanned
    }

    fn variables(size: usize) -> Variables {
        Variables::from_value(
            Value::from_json(serde_json::json!({ "v": "x".repeat(size) })).expect("plain JSON"),
        )
    }

    /// `query { ...F0 }`, each fragment spreading the next `width` times, the last
    /// holding `leaf`.
    fn fan_out(depth: usize, width: usize, leaf: &str) -> String {
        let mut document = String::from("query { ...F0 }");
        for at in 0..depth {
            let spreads = format!(" ...F{}", at + 1).repeat(width);
            write!(document, " fragment F{at} on Query {{{spreads} }}").expect("a String");
        }
        write!(document, " fragment F{depth} on Query {{ {leaf} }}").expect("a String");
        document
    }

    /// `query { ...F0 }` over `links` fragments, each spreading the next once.
    fn chain(links: usize) -> String {
        fan_out(links - 1, 1, "leaf")
    }

    /// Past the limit by its values and within it by its arguments: what a document
    /// nothing multiplies copies is its text, which the request's own cap holds.
    #[test]
    fn a_document_nothing_multiplies_is_held_by_its_text_alone() {
        let wide = format!("query {{ {} }}", "a(x: \"text\") ".repeat(100));
        let limits = DocumentLimits {
            repeated_selections: 0,
            copied_bytes: 100,
            list_argument_bytes: usize::MAX,
        };
        let document = parse_query(&wide).expect("the document parses");
        assert!(schemaless_refusal(&document, &Variables::default(), limits).is_none());

        let defining = wide.replacen("query", "query($unused: Int)", 1);
        let document = parse_query(&defining).expect("the document parses");
        let refused = schemaless_refusal(&document, &Variables::default(), limits);
        assert_eq!(refused.expect("past the limit").message, TOO_COMPLEX);
    }

    #[test]
    fn each_spread_of_a_fragment_already_read_adds_what_it_expands_to() {
        for (document, added) in [
            ("{ ...A } fragment A on Q { a b c }", 0),
            (
                "{ ...A ...B } fragment A on Q { a } fragment B on Q { b }",
                0,
            ),
            ("{ ...A ...A } fragment A on Q { a b }", 2),
            ("{ x { ...A } y { ...A } } fragment A on Q { a { b } }", 2),
            ("{ ...A ...A ...A } fragment A on Q { ... on Q { a } }", 2),
            (
                "{ ...A ...B } fragment A on Q { ...C } fragment B on Q { ...C } \
                 fragment C on Q { c }",
                1,
            ),
            (
                "query X { ...A } query Y { ...A } fragment A on Q { a b }",
                2,
            ),
        ] {
            assert_eq!(counted(document, usize::MAX), Ok(added), "{document}");
        }
    }

    /// async-graphql's rules recurse through a fragment nothing reaches, so what such
    /// a fragment spreads is never read: the first in the text says why.
    #[test]
    fn a_fragment_no_operation_reaches_stops_the_count_at_the_first_in_the_text() {
        let document = parse_query(
            "{ ...A } fragment A on Q { a }\nfragment U on Q { ...V }\nfragment V on Q { b }",
        )
        .expect("the document parses");
        let refused =
            schemaless_refusal(&document, &Variables::default(), LIMITS).expect("two unreached");
        assert_eq!(refused.message, r#"Fragment "U" is never used"#);
        assert_eq!(refused.locations, [Pos { line: 2, column: 1 }]);
    }

    const LIMITS: DocumentLimits = DocumentLimits {
        repeated_selections: 2000,
        copied_bytes: 4 * 1024 * 1024,
        list_argument_bytes: usize::MAX,
    };

    #[test]
    fn the_limit_admits_what_it_names_and_refuses_one_past() {
        let document = "{ ...A ...A ...A } fragment A on Q { a }";
        assert_eq!(counted(document, 2), Ok(2));
        assert_eq!(counted(document, 1), past_limit());
    }

    /// Unmemoized, the first of these would count `2^31` selections and the second
    /// `64^31`, which is how a regression fails here: it does not return.
    #[test]
    fn a_fan_out_as_deep_as_a_document_runs_is_counted_once_per_fragment() {
        for width in [2, 64] {
            assert_eq!(
                counted(&fan_out(31, width, "leaf"), 10_000_000),
                past_limit(),
                "{width}",
            );
        }
    }

    #[test]
    fn a_fan_out_ending_in_nothing_a_score_counts_is_counted() {
        for leaf in ["__typename", "...Missing", "... { ...Missing }"] {
            assert_eq!(counted(&fan_out(12, 2, leaf), 2000), past_limit(), "{leaf}");
        }
    }

    #[test]
    fn a_cycle_ends_the_read_and_is_left_to_what_refuses_it() {
        for document in [
            "{ ...F } fragment F on Q { ...F }",
            "{ ...F } fragment F on Q { ...G } fragment G on Q { ...F }",
            "{ ...F } fragment F on Q { x { ...F } }",
        ] {
            assert!(counted(document, usize::MAX).is_ok(), "{document}");
        }
    }

    #[test]
    fn a_read_stops_at_the_depth_async_graphql_runs() {
        assert_eq!(counted(&chain(32), 2000), Ok(0));
        for links in [33, 1_000] {
            let document = parse_query(chain(links)).expect("the document parses");
            let refused = schemaless_refusal(&document, &Variables::default(), LIMITS)
                .expect("past the depth");
            assert_eq!(
                refused.message,
                "The recursion depth of the query cannot be greater than `32`"
            );
        }
    }

    /// A field's empty set is no level, so a fan-out whose leaf stands at the last
    /// depth stays counted: stopping on that set would let it through uncounted.
    #[test]
    fn a_leaf_at_the_last_depth_is_no_level_deeper() {
        let deepest = format!(
            "{} fragment L on Query {{ a b }} fragment M on Query {{ ...L ...L }}",
            fan_out(29, 1, "...M"),
        );
        assert_eq!(counted(&deepest, 2000), Ok(2));
        assert_eq!(counted(&deepest, 1), past_limit());
    }

    #[test]
    fn a_refusal_is_worded_as_async_graphql_words_it() {
        let document = parse_query(fan_out(16, 2, "leaf")).expect("the document parses");
        let refused =
            schemaless_refusal(&document, &Variables::default(), LIMITS).expect("past the limit");
        assert_eq!(refused.message, TOO_COMPLEX);
        assert!(refused.locations.is_empty());
        assert!(refused.path.is_empty());
    }

    #[test]
    fn each_reference_copies_its_variable_wherever_it_stands() {
        let size = 1_000;
        for (document, references) in [
            ("query($v: String) { a(x: $v) }", 1),
            ("query($v: String) { a(x: $v) b(x: $v) }", 2),
            ("query($v: String) { a(x: [$v, $v, [$v]]) }", 3),
            ("query($v: String) { a(x: { y: $v, z: { y: $v } }) }", 2),
            ("query($v: String) { a @d(if: $v) b @d(if: $v, x: $v) }", 3),
            ("query($v: String) @d(if: $v) { a }", 1),
            ("query($v: String) { ... @d(if: $v) { a } }", 1),
            (
                "query($v: String) { ...F @d(if: $v) } fragment F on Q { a }",
                1,
            ),
            (
                "query($v: String) { ...F } fragment F on Q @d(if: $v) { a }",
                1,
            ),
        ] {
            let copied = copied(document, size);
            assert!(
                (references * size..references * size + 100).contains(&copied),
                "{document}: {copied}",
            );
        }
    }

    #[test]
    fn a_fragment_spread_again_copies_what_its_arguments_do_again() {
        // What a reference copies and scans, and the stand-in it leaves in the value.
        let reference = 1_000 + FIRST + NULL;
        let document = "query($v: String) { ...F ...F ...F } fragment F on Q { a(x: $v) }";
        assert_eq!(copied(document, 1_000), 3 * reference);
        let text = format!(
            "query($v: String) {{ ...F ...F }} fragment F on Q {{ a(x: \"{}\") b(x: $v) }}",
            "y".repeat(500),
        );
        assert_eq!(copied(&text, 1_000), 2 * (500 + reference));
    }

    /// The list, its two nulls, and the definition each reference scans for.
    #[test]
    fn a_variable_nothing_gave_copies_nothing_and_scans_all_the_same() {
        let document = "query($w: String) { a(x: [$w, $w]) }";
        assert_eq!(copied(document, 1_000), 5 + 2 * FIRST);
    }

    #[test]
    fn a_reference_scans_the_definitions_down_to_its_own() {
        let third = "query($a: Int, $b: Int, $v: String) { x(y: $v) z(y: $v) }";
        assert_eq!(copied(third, 1_000), 2 * (1_000 + 3 + NULL));
    }

    /// Validation copies what the request sent by its name alone, before it refuses a
    /// reference no definition stands behind, which then never scans.
    #[test]
    fn a_reference_no_definition_names_copies_and_scans_nothing() {
        assert_eq!(copied("{ x(y: $v) }", 1_000), 1_000 + NULL);
    }

    /// `ArgumentsOfCorrectType` skips an operation a name leaves out, and with no name
    /// reads them all, which no run follows: nothing is selected, so nothing scans.
    #[test]
    fn only_the_operation_a_name_selects_copies_its_references() {
        let document = "query A($v: String) { a(x: $v) } query B($v: String) { b(x: $v) c(x: $v) }";
        let run = 1_000 + FIRST + NULL;
        assert_eq!(copied_selecting(document, Some("A"), 1_000), run);
        assert_eq!(copied_selecting(document, Some("B"), 1_000), 2 * run);
        assert_eq!(copied_selecting(document, Some("Z"), 1_000), 0);
        assert_eq!(copied_selecting(document, None, 1_000), 3 * (1_000 + NULL));
    }

    /// A fragment's own arguments are copied once wherever it is spread, and again at
    /// each run of it: the run, being larger, is what the copies are then held to.
    #[test]
    fn a_fragment_copies_its_arguments_once_whichever_operation_spreads_it() {
        let document =
            "query A { x } query B($v: String) { ...F ...F } fragment F on Q { a(x: $v) }";
        assert_eq!(copied_selecting(document, Some("A"), 1_000), 1_000 + NULL);
        assert_eq!(
            copied_selecting(document, Some("B"), 1_000),
            2 * (1_000 + FIRST + NULL),
        );
    }

    #[test]
    fn the_copies_limit_admits_what_it_names_and_refuses_one_past() {
        let document = parse_query("query($v: String) { a(x: [$v, $v]) }").expect("it parses");
        let variables = variables(1_000);
        let limits = |copied_bytes| DocumentLimits {
            repeated_selections: 2000,
            copied_bytes,
            list_argument_bytes: usize::MAX,
        };
        let exactly = 2 * (1_000 + FIRST) + 5;
        assert!(schemaless_refusal(&document, &variables, limits(exactly)).is_none());
        let refused =
            schemaless_refusal(&document, &variables, limits(exactly - 1)).expect("one byte past");
        assert_eq!(refused.message, TOO_COMPLEX);
    }

    #[test]
    fn a_fields_arguments_scan_each_other_in_every_operation() {
        for (document, name, steps) in [
            ("{ a }", None, 0),
            ("{ a(x: 1) }", None, 1),
            ("{ a(x: 1, y: 2) a(x: 1, y: 2) }", None, 8),
            ("{ a(x: 1, y: 2) { b(x: 1, y: 2, z: 3) } }", None, 13),
            ("{ ... { a(x: 1, y: 2) } }", None, 4),
            ("{ ...F ...F } fragment F on Q { a(x: 1, y: 2) }", None, 8),
            ("query A { a } query B { b(x: 1, y: 2) }", Some("A"), 4),
            ("{ a @d(x: 1, y: 2, z: 3) }", None, 0),
        ] {
            assert_eq!(scanned(document, name), steps, "{document}");
        }
    }

    #[test]
    fn the_scans_limit_admits_what_it_names_and_refuses_one_past() {
        let document = parse_query("{ a(x: 1, y: 2) a(x: 1, y: 2) }").expect("it parses");
        let variables = Variables::default();
        let limits = |copied_bytes| DocumentLimits {
            repeated_selections: 2000,
            copied_bytes,
            list_argument_bytes: usize::MAX,
        };
        assert!(schemaless_refusal(&document, &variables, limits(8)).is_none());
        let refused = schemaless_refusal(&document, &variables, limits(7)).expect("one step past");
        assert_eq!(refused.message, TOO_COMPLEX);
    }

    /// So a request carrying a value never spends the limit in what it may carry by
    /// being counted past what it carried, in a variable's JSON or in a document's text.
    #[test]
    fn no_value_counts_past_what_its_text_weighs() {
        for json in [
            "null",
            "true",
            "false",
            "0",
            "-1",
            "1.5e3",
            r#""""#,
            r#""é""#,
            r#""\u00e9""#,
            "[]",
            "[1]",
            "[1,2,3]",
            "[[],[]]",
            "{}",
            r#"{"a":1}"#,
            r#"{"a":{"b":[1,null]},"c":"d"}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(json).expect("it is JSON");
            let value = Value::from_json(value).expect("plain JSON");
            assert!(size_of(&value) <= json.len(), "{json}: {}", size_of(&value));
        }
        for literal in [
            "null",
            "1",
            "A",
            r#""""#,
            "[1 2 3]",
            r#"["a""b"]"#,
            "[[][]]",
            "{a:1 b:2}",
            "{a:{b:[1]}}",
        ] {
            let document = format!("{{ ...F }} fragment F on Q {{ a(x: {literal}) }}");
            let copied = copied(&document, 0);
            assert!(copied <= literal.len(), "{literal}: {copied}");
        }
    }

    /// Which is as close as the text comes: it is what a list's copies scale with.
    #[test]
    fn a_list_of_small_numbers_counts_what_its_text_weighs() {
        let value = Value::from_json(serde_json::json!([1, 2, 3])).expect("plain JSON");
        assert_eq!(size_of(&value), "[1,2,3]".len());
    }

    /// Each variable's size is read once, whatever refers to it: a read per reference
    /// would be the cost this counts, over its value's whole structure.
    #[test]
    fn a_variable_is_read_once_however_many_references_it_has() {
        let list = vec!["$v"; 2_000].join(",");
        let document = parse_query(format!("query($v: [Int]) {{ a(x: [{list}]) }}"))
            .expect("the document parses");
        let variables = Variables::from_value(
            Value::from_json(serde_json::json!({ "v": vec![1; 100_000] })).expect("plain JSON"),
        );
        let limits = DocumentLimits {
            repeated_selections: 2000,
            copied_bytes: usize::MAX,
            list_argument_bytes: usize::MAX,
        };
        let value = variables.get("v").expect("the request sent it");

        let started = thread_cpu_time();
        for _ in 0..64 {
            std::hint::black_box(size_of(std::hint::black_box(value)));
        }
        let walking = thread_cpu_time() - started;
        let started = thread_cpu_time();
        let mut cost = DocumentCost::new(&document, None, &variables, limits);
        cost.of_document(&document).expect("no limit to pass");
        let counting = thread_cpu_time() - started;

        assert!(
            counting < walking,
            "counted in {counting:?}, walked 64 times in {walking:?}"
        );
    }

    struct Listed;

    #[Object]
    impl Listed {
        async fn read(&self, x: Option<String>) -> usize {
            x.map_or(0, |x| x.len())
        }

        async fn more(&self) -> Vec<Listed> {
            Vec::new()
        }

        async fn one(&self) -> Option<Listed> {
            None
        }
    }

    struct Lists;

    #[Object]
    impl Lists {
        async fn rows(&self) -> Option<Vec<Option<Listed>>> {
            None
        }

        async fn row(&self) -> Listed {
            Listed
        }

        async fn grid(&self) -> Vec<Vec<Listed>> {
            Vec::new()
        }
    }

    /// The types of a schema whose root is [`Lists`].
    fn lists() -> Registry {
        let mut registry = Registry::default();
        let root = Lists::create_type_info(&mut registry);
        registry.query_type = MetaTypeName::concrete_typename(&root).to_owned();
        registry
    }

    /// What `document`, `$v` holding `size` bytes, answers under a bound of `bound`
    /// bytes a row.
    fn below_the_bound(document: &str, size: usize, bound: usize) -> bool {
        let document = parse_query(document).expect("the document parses");
        let limits = DocumentLimits {
            list_argument_bytes: bound,
            ..LIMITS
        };
        refusal(&document, None, &variables(size), limits, &lists()).is_none()
    }

    /// What a field a list stands above reads, through any nesting, wherever it is
    /// spread, against a bound read at its byte.
    #[test]
    fn what_a_lists_rows_copy_is_held_to_its_bound() {
        let reference = 1_000 + FIRST + NULL;
        for document in [
            "query($v: String) { rows { read(x: $v) } }",
            "query($v: String) { row { more { read(x: $v) } } }",
            "query($v: String) { rows { one { read(x: $v) } } }",
            "query($v: String) { grid { read(x: $v) } }",
            "query($v: String) { rows { ... on Listed { read(x: $v) } } }",
            "query($v: String) { rows { ... { read(x: $v) } } }",
            "query($v: String) { rows { ...F } } fragment F on Listed { read(x: $v) }",
            "query($v: String) { row { ...F } } fragment F on Listed { more { read(x: $v) } }",
            "query($v: String) { rows { ...F } } fragment F on Listed { ...G } \
             fragment G on Listed { read(x: $v) }",
        ] {
            assert!(below_the_bound(document, 1_000, reference), "{document}");
            assert!(
                !below_the_bound(document, 1_000, reference - 1),
                "{document}"
            );
        }
    }

    #[test]
    fn a_field_no_list_stands_above_reads_its_arguments_once() {
        for document in [
            "query($v: String) { row { read(x: $v) } }",
            "query($v: String) { row { one { read(x: $v) } } }",
            "query($v: String) { row { ...F } } fragment F on Listed { read(x: $v) }",
            "query($v: String) { row { read(x: $v) } rows { one { __typename } } }",
            "query($v: String) { rows @include(if: $v) { one { __typename } } }",
        ] {
            assert!(below_the_bound(document, 100_000, 1), "{document}");
        }
    }

    /// A spread read where a list stands above it and where none does is counted each
    /// way, so the one copying at each row is never read as the other.
    #[test]
    fn a_fragment_spread_above_and_below_a_list_is_counted_where_it_stands() {
        let document = "query($v: String) { row { ...F } rows { ...F } } \
                        fragment F on Listed { read(x: $v) }";
        assert!(!below_the_bound(document, 1_000, 1_000));
        let document = "query($v: String) { rows { ...F } row { ...F } } \
                        fragment F on Listed { read(x: $v) }";
        assert!(!below_the_bound(document, 1_000, 1_000));
    }

    /// A selection left out runs nothing, and `@skip` and `@include` are read once,
    /// before anything runs, so neither copies at a row.
    #[test]
    fn a_selection_left_out_and_its_directive_copy_nothing_at_a_row() {
        let large = "x".repeat(10_000);
        for document in [
            "query($v: String) { rows { read(x: $v) @skip(if: true) } }".to_owned(),
            "query($v: String) { rows { ... @include(if: false) { read(x: $v) } } }".to_owned(),
            format!(r#"query {{ rows {{ read(x: "{large}") @skip(if: true) }} }}"#),
        ] {
            assert!(below_the_bound(&document, 10_000, 100), "{document}");
        }
    }

    /// The other limits' sentence: a client has no reason to tell the checks apart.
    #[test]
    fn a_document_past_the_bound_is_refused_as_any_limit_refuses() {
        let document = parse_query("query($v: String) { rows { read(x: $v) } }").expect("parses");
        let limits = DocumentLimits {
            list_argument_bytes: 10,
            ..LIMITS
        };
        let refused =
            refusal(&document, None, &variables(100), limits, &lists()).expect("past the bound");
        assert_eq!(refused.message, TOO_COMPLEX);
        assert!(refused.locations.is_empty());
    }

    struct Query;

    #[Object]
    impl Query {
        async fn one(&self, x: Option<String>) -> usize {
            x.map_or(0, |x| x.len())
        }

        async fn echo(&self, values: Option<Vec<String>>) -> usize {
            values.map_or(0, |values| values.len())
        }
    }

    /// What `text` costs on this thread's CPU clock, against the document's weight: its
    /// parse, and everything after it, the count and, should it admit the document,
    /// async-graphql's run of it, whose cost no answer shows.
    async fn parsing_and_past_it(text: String) -> (Duration, Duration) {
        let schema = Schema::new(Query, EmptyMutation, EmptySubscription);
        let mut request = Request::new(text);
        let variables = std::mem::take(&mut request.variables);

        let started = thread_cpu_time();
        let document = request.parsed_query().expect("the document parses");
        let parsing = thread_cpu_time() - started;
        let started = thread_cpu_time();
        if schemaless_refusal(document, &variables, LIMITS).is_none() {
            drop(schema.execute(request).await);
        }
        (parsing, thread_cpu_time() - started)
    }

    /// `OverlappingFieldsCanBeMerged` looks each argument of a repeated key up in the
    /// other's: run, this costs about three times its parse here, and four times as
    /// much at twice the size.
    #[tokio::test]
    async fn a_repeated_key_costs_no_square_of_its_arguments() {
        let arguments: String = (0..8_000).map(|at| format!("a{at}: 1, ")).collect();
        let text = format!("{{ one({arguments}) one({arguments}) }}");
        let (parsing, past) = parsing_and_past_it(text).await;
        assert!(past < parsing, "parsed in {parsing:?}, then {past:?}");
    }

    /// `Context::var_value` scans the definitions at each reference, inside one poll of
    /// the resolver: run, this costs about three times its parse here, and four times
    /// as much at twice the size.
    #[tokio::test]
    async fn a_reference_costs_no_square_of_the_definitions() {
        let count = 5_000;
        let definitions: String = (0..count)
            .map(|at| format!(r#"$a{at}: String = "", "#))
            .collect();
        let each: String = (0..count).map(|at| format!("$a{at}, ")).collect();
        let last = "$z, ".repeat(count);
        let text =
            format!(r#"query({definitions}$z: String = "") {{ echo(values: [{each}{last}]) }}"#);
        let (parsing, past) = parsing_and_past_it(text).await;
        assert!(past < parsing, "parsed in {parsing:?}, then {past:?}");
    }
}
