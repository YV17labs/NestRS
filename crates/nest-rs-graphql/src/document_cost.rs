//! What a document costs, counted before anything walks it.
//!
//! async-graphql (`7.2.1`) pays a fragment's whole expansion again at each of its
//! spreads, in `check_recursive_depth` and in every visitor `check_rules` inlines,
//! and only then scores the document: `...F1 ...F1` at each of `n` levels costs
//! `2^n` on the thread, which no deadline reaches, for a few bytes a level, and a
//! `__typename` or an unknown fragment at the bottom scores nothing at all. Its
//! rules recurse through a fragment no operation spreads, which its recursion bound
//! never reads, only to refuse it. And it builds an argument by copying a variable's
//! value at each reference, so `[$v, $v, …]` costs the references times the value,
//! which no score counts.
//!
//! So each fragment and each variable is read here once. A spread of a fragment
//! already read adds what it expands to, and an argument what it copies: the walks
//! ahead cost the document's text plus those two sums, each held to a limit. A
//! document spreading each fragment once adds nothing, so a field the schema scores
//! below one stays the visitor's to score. A fragment nothing reached is refused
//! here, as validation would.

use std::collections::HashMap;
use std::convert::Infallible;

use async_graphql::parser::types::{
    Directive, ExecutableDocument, Field, FragmentDefinition, Selection, SelectionSet,
};
use async_graphql::{Name, Pos, Positioned, ServerError, Value, Variables};

use crate::operation_units::RECURSIVE_DEPTH;

/// async-graphql's (`7.2.1`) refusal of a document past `max_complexity`, which a
/// document past a limit here answers word for word.
const TOO_COMPLEX: &str = "Query is too complex.";

/// What a document may add to its text.
#[derive(Clone, Copy)]
pub(crate) struct DocumentLimits {
    /// What spreading a fragment again may add, in selections.
    pub(crate) repeated_selections: usize,
    /// What its arguments may copy, in bytes, as its fragments expand them and its
    /// variables fill them.
    pub(crate) copied_bytes: usize,
}

/// The refusal of `document`, should it cost past `limits`, nest past what
/// async-graphql runs, or hold a fragment nothing reaches. A document holding no
/// fragment and given no variable does none of it: its walks cost its text, and it
/// pays for no count.
pub(crate) fn refusal(
    document: &ExecutableDocument,
    variables: &Variables,
    limits: DocumentLimits,
) -> Option<ServerError> {
    if document.fragments.is_empty() && variables.is_empty() {
        return None;
    }
    let mut cost = DocumentCost::new(document, variables, limits);
    cost.of_document(document).err().map(ServerError::from)
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
    copied: usize,
}

impl Weight {
    fn add(&mut self, other: Self) {
        self.selections = self.selections.saturating_add(other.selections);
        self.copied = self.copied.saturating_add(other.copied);
    }
}

/// One document's cost, as far as it was read.
struct DocumentCost<'d> {
    fragments: &'d HashMap<Name, Positioned<FragmentDefinition>>,
    limits: DocumentLimits,
    /// What each fragment read so far holds.
    read: HashMap<&'d str, Weight>,
    /// How many bytes each variable holds.
    sizes: HashMap<&'d str, usize>,
    /// What the spreads of a fragment already read added.
    repeated: usize,
    /// What the arguments read so far copy.
    copied: usize,
}

impl<'d> DocumentCost<'d> {
    fn new(
        document: &'d ExecutableDocument,
        variables: &'d Variables,
        limits: DocumentLimits,
    ) -> Self {
        Self {
            fragments: &document.fragments,
            limits,
            read: HashMap::with_capacity(document.fragments.len()),
            sizes: variables
                .iter()
                .map(|(name, value)| (name.as_str(), size_of(value)))
                .collect(),
            repeated: 0,
            copied: 0,
        }
    }

    /// Count every operation, async-graphql's walks reading each whichever one the
    /// request selects. The first fragment left unread, by its place in the text, is
    /// what the count stops at: every other walk stays on what an operation reaches.
    fn of_document(&mut self, document: &'d ExecutableDocument) -> Result<(), Past<'d>> {
        for (_, operation) in document.operations.iter() {
            self.of_arguments(&operation.node.directives, None)?;
            self.of_set(&operation.node.selection_set, 0)?;
        }
        if self.read.len() == self.fragments.len() {
            return Ok(());
        }
        let unread = self
            .fragments
            .iter()
            .filter(|(name, _)| !self.read.contains_key(name.as_str()))
            .min_by_key(|(_, fragment)| fragment.pos);
        match unread {
            Some((name, fragment)) => Err(Past::Unspread(name, fragment.pos)),
            None => Ok(()),
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
                    self.of_set(&field.node.selection_set, depth + 1)?,
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
            weight.add(Weight {
                selections: usize::from(field.is_some()),
                copied: self.of_arguments(selection.node.directives(), field)?,
            });
        }
        Ok(weight)
    }

    /// What a spread `depth` selection sets down holds. A spread of a fragment no
    /// document defines, or of one still being read, a cycle, is one selection:
    /// validation refuses both, after the walks they cost.
    fn of_spread(&mut self, name: &'d str, depth: usize) -> Result<Weight, Past<'d>> {
        if let Some(weight) = self.read.get(name).copied() {
            self.repeated = self.repeated.saturating_add(weight.selections);
            self.copied = self.copied.saturating_add(weight.copied);
            self.within_limits()?;
            return Ok(weight);
        }
        let one = Weight {
            selections: 1,
            copied: 0,
        };
        let Some(fragment) = self.fragments.get(name) else {
            return Ok(one);
        };
        self.read.insert(name, one);
        let mut weight = self.of_set(&fragment.node.selection_set, depth + 1)?;
        let copied = self.of_arguments(&fragment.node.directives, None)?;
        weight.copied = weight.copied.saturating_add(copied);
        self.read.insert(name, weight);
        Ok(weight)
    }

    /// What the arguments of `directives` and of `field` copy.
    fn of_arguments(
        &mut self,
        directives: &[Positioned<Directive>],
        field: Option<&Field>,
    ) -> Result<usize, Past<'d>> {
        let mut copied = 0_usize;
        let held = directives
            .iter()
            .flat_map(|directive| &directive.node.arguments);
        let named = field.into_iter().flat_map(|field| &field.arguments);
        for (_, value) in named.chain(held) {
            // Built as async-graphql builds it, since async-graphql re-exports only
            // the const `Value`, each reference standing in for what it copies.
            let Ok(written) = value.node.clone().into_const_with::<Infallible>(|name| {
                let size = self.sizes.get(name.as_str()).copied().unwrap_or(0);
                copied = copied.saturating_add(size);
                Ok(Value::Null)
            });
            copied = copied.saturating_add(size_of(&written));
        }
        self.copied = self.copied.saturating_add(copied);
        self.within_limits()?;
        Ok(copied)
    }

    fn within_limits(&self) -> Result<(), Past<'d>> {
        if self.repeated > self.limits.repeated_selections || self.copied > self.limits.copied_bytes
        {
            return Err(Past::Limit);
        }
        Ok(())
    }
}

/// About what copying `value` costs, a value's structure counting what its text does.
fn size_of(value: &Value) -> usize {
    match value {
        Value::Null | Value::Boolean(_) | Value::Number(_) => 8,
        Value::String(text) => text.len(),
        Value::Enum(name) => name.len(),
        Value::Binary(bytes) => bytes.len(),
        Value::List(items) => 8 + items.iter().map(size_of).sum::<usize>(),
        Value::Object(members) => {
            8 + members
                .iter()
                .map(|(key, member)| key.len() + size_of(member))
                .sum::<usize>()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use async_graphql::parser::parse_query;

    use super::*;

    const NO_COPIES_COUNTED: usize = usize::MAX;

    /// What [`size_of`] counts of a `null`, or of a list holding nothing.
    const NULL: usize = 8;

    /// What counting `document` under `repeated` selections stops at, held as what a
    /// test compares, or what its fragments' repeats added when it runs to its end.
    fn counted(document: &str, repeated: usize) -> Result<usize, String> {
        let document = parse_query(document).expect("the document parses");
        let variables = Variables::default();
        let limits = DocumentLimits {
            repeated_selections: repeated,
            copied_bytes: NO_COPIES_COUNTED,
        };
        let mut cost = DocumentCost::new(&document, &variables, limits);
        match cost.of_document(&document) {
            Ok(()) => Ok(cost.repeated),
            Err(past) => Err(format!("{past:?}")),
        }
    }

    /// What a count past its limit stops with.
    fn past_limit() -> Result<usize, String> {
        Err(format!("{:?}", Past::Limit))
    }

    /// What `document`'s arguments copy, given `$v` of `size` bytes.
    fn copied(document: &str, size: usize) -> usize {
        let document = parse_query(document).expect("the document parses");
        let variables = variables(size);
        let limits = DocumentLimits {
            repeated_selections: usize::MAX,
            copied_bytes: NO_COPIES_COUNTED,
        };
        let mut cost = DocumentCost::new(&document, &variables, limits);
        cost.of_document(&document).expect("nothing past a limit");
        cost.copied
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

    #[test]
    fn a_document_with_no_fragment_and_no_variable_is_never_counted() {
        let wide = format!("query {{ {} }}", "a(x: \"text\") ".repeat(100));
        let document = parse_query(wide).expect("the document parses");
        let none = DocumentLimits {
            repeated_selections: 0,
            copied_bytes: 0,
        };
        assert!(refusal(&document, &Variables::default(), none).is_none());
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
        let refused = refusal(&document, &Variables::default(), LIMITS).expect("two unreached");
        assert_eq!(refused.message, r#"Fragment "U" is never used"#);
        assert_eq!(refused.locations, [Pos { line: 2, column: 1 }]);
    }

    const LIMITS: DocumentLimits = DocumentLimits {
        repeated_selections: 2000,
        copied_bytes: 4 * 1024 * 1024,
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
            let refused =
                refusal(&document, &Variables::default(), LIMITS).expect("past the depth");
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
        let refused = refusal(&document, &Variables::default(), LIMITS).expect("past the limit");
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
        // What a reference copies, and the stand-in it leaves in the value.
        let reference = 1_000 + NULL;
        let document = "query($v: String) { ...F ...F ...F } fragment F on Q { a(x: $v) }";
        assert_eq!(copied(document, 1_000), 3 * reference);
        let text = format!(
            "query($v: String) {{ ...F ...F }} fragment F on Q {{ a(x: \"{}\") b(x: $v) }}",
            "y".repeat(500),
        );
        assert_eq!(copied(&text, 1_000), 2 * (500 + reference));
    }

    #[test]
    fn a_variable_nothing_gave_copies_nothing() {
        let document = "query($w: String) { a(x: [$w, $w]) }";
        assert_eq!(
            copied(document, 1_000),
            3 * NULL,
            "the list and its two nulls"
        );
    }

    #[test]
    fn the_copies_limit_admits_what_it_names_and_refuses_one_past() {
        let document = parse_query("query($v: String) { a(x: [$v, $v]) }").expect("it parses");
        let variables = variables(1_000);
        let limits = |copied_bytes| DocumentLimits {
            repeated_selections: 2000,
            copied_bytes,
        };
        let exactly = 2_000 + 3 * NULL;
        assert!(refusal(&document, &variables, limits(exactly)).is_none());
        let refused = refusal(&document, &variables, limits(exactly - 1)).expect("one byte past");
        assert_eq!(refused.message, TOO_COMPLEX);
    }

    /// Each variable's size is read once, whatever refers to it: a read per reference
    /// would be the cost this counts, over its value's whole structure.
    #[test]
    fn a_variable_is_read_once_however_many_references_it_has() {
        let list = vec!["$v"; 2_000].join(",");
        let document = parse_query(format!("query($v: [Int]) {{ a(x: [{list}]) }}"))
            .expect("the document parses");
        let variables = Variables::from_value(
            Value::from_json(serde_json::json!({ "v": vec![1; 1_000] })).expect("plain JSON"),
        );
        let limits = DocumentLimits {
            repeated_selections: 2000,
            copied_bytes: usize::MAX,
        };
        let mut cost = DocumentCost::new(&document, &variables, limits);
        cost.of_document(&document).expect("no limit to pass");
        assert_eq!(cost.sizes.len(), 1);
    }
}
