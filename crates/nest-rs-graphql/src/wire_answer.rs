//! What a `#[field_resolver]` answers of an `#[expose]`d entity's wire type, which
//! that entity's response mask shapes. A nested field names no posture, so the
//! expansion asks this probe: inherent items, which name resolution prefers, exist
//! only for an answer holding a wire type's rows, and [`FieldAnswerFallback`] leaves
//! every other answer as it is. An answer is walked down to its rows, so what holds
//! them owes the mask no serde impl.
//!
//! A shape the walk cannot put a cut row back into refuses at compile time
//! ([`WireAnswer::READABLE`]), and what no impl can name, an object holding a row
//! among them, refuses the boot ([`check_field_answers`]): a row never passes unread
//! in silence.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};
use std::hash::{BuildHasher, Hash};
use std::sync::{Arc, Weak};

use async_graphql::connection::{
    Connection, ConnectionNameType, CursorType, EdgeNameType, NodesFieldSwitcherSealed,
};
use async_graphql::registry::{MetaType, MetaTypeName, Registry};
use async_graphql::{Context, Error, Json, ObjectType, OutputType};
use nest_rs_core::anyhow;
use nest_rs_guards::Denial;

use crate::error::denial_to_graphql_error;
use crate::resolver::{GraphqlFieldRegistration, ServedObjects};

/// An answer holding an `#[expose]`d wire type's rows.
pub trait WireAnswer: Sized {
    /// Evaluated at the field's expansion: a shape holding no place for the row the
    /// mask cuts panics here, a compile error naming it. Every wrapper forwards its
    /// row's, so the refusal holds however deep the shape sits.
    const READABLE: () = ();

    /// This answer, masked as a root field's answer is.
    ///
    /// # Errors
    ///
    /// The refusal of a selected column the grant withholds, or of a value the
    /// mask cannot reconcile with the entity.
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error>;

    /// An `Option` of this answer, masked: a wire type's whole, so a row it refuses
    /// can read `None`.
    ///
    /// # Errors
    ///
    /// As [`masked`](Self::masked).
    fn masked_option(answer: Option<Self>, ctx: &Context<'_>) -> Result<Option<Self>, Error> {
        answer.map(|inner| inner.masked(ctx)).transpose()
    }

    /// A list of this answer, masked: a wire type's whole, so a row it refuses drops.
    ///
    /// # Errors
    ///
    /// As [`masked`](Self::masked).
    fn masked_list(answer: Vec<Self>, ctx: &Context<'_>) -> Result<Vec<Self>, Error> {
        answer.into_iter().map(|inner| inner.masked(ctx)).collect()
    }
}

/// A wire type `#[expose(…, graphql)]` emits, naming the entity whose mask shapes
/// its rows. Never written by hand.
pub trait WireRow: WireAnswer {
    /// One page's rows masked in place by the selection of the connection `ctx`
    /// answers: whether each stays.
    ///
    /// # Errors
    ///
    /// As [`WireAnswer::masked`].
    fn masked_nodes(ctx: &Context<'_>, nodes: Vec<&mut Self>) -> Result<Vec<bool>, Error>;
}

/// A node holding no row stays; the rest are the row's to judge.
impl<A: WireRow> WireRow for Option<A> {
    fn masked_nodes(ctx: &Context<'_>, nodes: Vec<&mut Self>) -> Result<Vec<bool>, Error> {
        let holds: Vec<bool> = nodes.iter().map(|node| node.is_some()).collect();
        let rows = nodes.into_iter().filter_map(Option::as_mut).collect();
        let mut kept = A::masked_nodes(ctx, rows)?.into_iter();
        Ok(holds
            .into_iter()
            .map(|holds| !holds || kept.next().unwrap_or(false))
            .collect())
    }
}

impl<A: WireRow> WireRow for Box<A> {
    fn masked_nodes(ctx: &Context<'_>, nodes: Vec<&mut Self>) -> Result<Vec<bool>, Error> {
        A::masked_nodes(ctx, nodes.into_iter().map(|node| &mut **node).collect())
    }
}

impl<A: WireRow + Clone> WireRow for Arc<A> {
    fn masked_nodes(ctx: &Context<'_>, nodes: Vec<&mut Self>) -> Result<Vec<bool>, Error> {
        A::masked_nodes(ctx, nodes.into_iter().map(Arc::make_mut).collect())
    }
}

impl<A: WireRow> WireRow for Json<A> {
    fn masked_nodes(ctx: &Context<'_>, nodes: Vec<&mut Self>) -> Result<Vec<bool>, Error> {
        A::masked_nodes(ctx, nodes.into_iter().map(|node| &mut node.0).collect())
    }
}

impl<A: WireAnswer> WireAnswer for Option<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_option(self, ctx)
    }
}

impl<A: WireAnswer> WireAnswer for Vec<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self, ctx)
    }
}

impl<A: WireAnswer> WireAnswer for VecDeque<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self.into(), ctx).map(Into::into)
    }
}

impl<A: WireAnswer> WireAnswer for LinkedList<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = A::masked_list(self.into_iter().collect(), ctx)?;
        Ok(rows.into_iter().collect())
    }
}

impl<A: WireAnswer + Ord> WireAnswer for BTreeSet<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = A::masked_list(self.into_iter().collect(), ctx)?;
        Ok(rows.into_iter().collect())
    }
}

impl<A: WireAnswer + Eq + Hash, S: BuildHasher + Default> WireAnswer for HashSet<A, S> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = A::masked_list(self.into_iter().collect(), ctx)?;
        Ok(rows.into_iter().collect())
    }
}

impl<A: WireAnswer> WireAnswer for Box<[A]> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self.into_vec(), ctx).map(Vec::into_boxed_slice)
    }
}

impl<A: WireAnswer + Clone> WireAnswer for Arc<[A]> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self.to_vec(), ctx).map(Into::into)
    }
}

/// Each row on its own: an array has no place for one a list would drop, so a row
/// the mask refuses refuses its selected columns.
impl<A: WireAnswer, const N: usize> WireAnswer for [A; N] {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = self
            .into_iter()
            .map(|row| row.masked(ctx))
            .collect::<Result<Vec<A>, Error>>()?;
        match rows.try_into() {
            Ok(rows) => Ok(rows),
            Err(_) => Err(denial_to_graphql_error(Denial::internal(
                "an array's rows changed in number",
            ))),
        }
    }
}

/// A map's values are rows of an answer with no selection, each masked on its own.
impl<K: Ord, A: WireAnswer> WireAnswer for BTreeMap<K, A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_iter()
            .map(|(key, row)| Ok((key, row.masked(ctx)?)))
            .collect()
    }
}

/// A map's values are rows of an answer with no selection, each masked on its own.
impl<K, A, S> WireAnswer for HashMap<K, A, S>
where
    K: Eq + Hash,
    A: WireAnswer,
    S: BuildHasher + Default,
{
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_iter()
            .map(|(key, row)| Ok((key, row.masked(ctx)?)))
            .collect()
    }
}

impl<A: WireAnswer> WireAnswer for Box<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        (*self).masked(ctx).map(Box::new)
    }
}

impl<A: WireAnswer + Clone> WireAnswer for Arc<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        Self::unwrap_or_clone(self).masked(ctx).map(Self::new)
    }
}

impl<A: WireAnswer + Clone> WireAnswer for Cow<'static, A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_owned().masked(ctx).map(Cow::Owned)
    }
}

impl<A: WireAnswer> WireAnswer for Json<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.0.masked(ctx).map(Json)
    }
}

impl<A: WireAnswer, E> WireAnswer for Result<A, E> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        match self {
            Ok(answer) => answer.masked(ctx).map(Ok),
            Err(err) => Ok(Err(err)),
        }
    }
}

/// async-graphql serves it behind its own `tokio-sync` feature, an app's to turn on.
impl<A: WireAnswer> WireAnswer for tokio::sync::Mutex<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_inner().masked(ctx).map(Self::new)
    }
}

/// async-graphql serves it behind its own `tokio-sync` feature, an app's to turn on.
impl<A: WireAnswer> WireAnswer for tokio::sync::RwLock<A> {
    const READABLE: () = A::READABLE;

    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_inner().masked(ctx).map(Self::new)
    }
}

/// Holds no place for the row the mask cuts.
impl<A: WireAnswer> WireAnswer for &'static A {
    const READABLE: () = panic!(
        "a `#[field_resolver]` answering a wire type's row behind a `&'static` borrow holds no \
         place for the row the mask cuts, so it would ship what a grant withholds: answer the \
         row by value, or behind an `Arc`"
    );

    fn masked(self, _ctx: &Context<'_>) -> Result<Self, Error> {
        let () = Self::READABLE;
        Ok(self)
    }
}

/// Holds no place for the rows the mask cuts.
impl<A: WireAnswer> WireAnswer for &'static [A] {
    const READABLE: () = panic!(
        "a `#[field_resolver]` answering a wire type's rows behind a `&'static` slice holds no \
         place for the rows the mask cuts, so it would ship what a grant withholds: answer them \
         in a `Vec`, or behind an `Arc<[_]>`"
    );

    fn masked(self, _ctx: &Context<'_>) -> Result<Self, Error> {
        let () = Self::READABLE;
        Ok(self)
    }
}

/// Nothing would hold the row the mask cuts: a `Weak` of it reads as gone.
impl<A: WireAnswer> WireAnswer for Weak<A> {
    const READABLE: () = panic!(
        "a `#[field_resolver]` answering a wire type's row behind a `Weak` leaves nothing to \
         hold the row the mask cuts, so it would ship what a grant withholds: answer the row \
         by value, or behind an `Arc`"
    );

    fn masked(self, _ctx: &Context<'_>) -> Result<Self, Error> {
        let () = Self::READABLE;
        Ok(self)
    }
}

/// Its rows masked in place, a refused one leaving with its edge; its flags, each
/// kept edge's cursor and what else it holds are the body's.
impl<K, R, Fields, EdgeFields, Name, EdgeName, Nodes> WireAnswer
    for Connection<K, R, Fields, EdgeFields, Name, EdgeName, Nodes>
where
    K: CursorType + Send + Sync,
    R: WireRow + OutputType,
    Fields: ObjectType,
    EdgeFields: ObjectType,
    Name: ConnectionNameType,
    EdgeName: EdgeNameType,
    Nodes: NodesFieldSwitcherSealed,
{
    const READABLE: () = R::READABLE;

    fn masked(mut self, ctx: &Context<'_>) -> Result<Self, Error> {
        let nodes = self.edges.iter_mut().map(|edge| &mut edge.node).collect();
        let mut kept = R::masked_nodes(ctx, nodes)?.into_iter();
        self.edges.retain(|_| kept.next().unwrap_or(false));
        Ok(self)
    }
}

/// A field resolver's answer, borrowed so the compiler can be asked what it holds.
pub struct FieldAnswer<'a, T>(pub &'a T);

impl<T: WireAnswer> FieldAnswer<'_, T> {
    /// What the boot reads of this answer: nothing, since the mask walks it down to
    /// its rows. Evaluating it refuses a shape the walk cannot read.
    pub const UNMASKED: Option<fn(&mut Registry) -> String> = {
        let () = T::READABLE;
        None
    };

    /// The entity's mask.
    pub fn mask(&self) -> fn(&Context<'_>, T) -> Result<T, Error> {
        |ctx, answer| answer.masked(ctx)
    }
}

/// What [`FieldAnswer`] answers for a value no walk reaches a wire type's rows in.
pub trait FieldAnswerFallback {
    /// The answer's own type.
    type Value;

    /// How the boot reads what the answer's type holds, which no mask walks.
    const UNMASKED: Option<fn(&mut Registry) -> String>;

    /// The answer as it is: a body's scalar or its own DTO names no entity.
    fn mask(&self) -> fn(&Context<'_>, Self::Value) -> Result<Self::Value, Error>;
}

impl<T: OutputType> FieldAnswerFallback for FieldAnswer<'_, T> {
    type Value = T;

    const UNMASKED: Option<fn(&mut Registry) -> String> = Some(T::create_type_info);

    fn mask(&self) -> fn(&Context<'_>, T) -> Result<T, Error> {
        |_, answer| Ok(answer)
    }
}

/// A wire type `#[expose(…, graphql)]` emits, as the boot tells it among the objects
/// an answer holds: a schema names no entity. It serves nothing, so it needs no
/// module gate.
pub struct GraphqlWireType {
    /// The wire type, as `std::any::type_name` spells it.
    pub rust_type: fn() -> &'static str,
}

inventory::collect!(GraphqlWireType);

/// Refuse every served `#[field_resolver]` whose answer holds a wire type's row where
/// no mask reads it: an object's column, a union's member, a flattened object, a
/// `Connection` of those. The mask walks a type to its rows, and an object of a body's
/// own names no entity and hands its columns to no walk, so the row would ship what a
/// grant withholds, to any caller, on no line a reviewer reads.
pub(crate) fn check_field_answers(served: &ServedObjects) -> anyhow::Result<()> {
    let wire: HashSet<&str> = inventory::iter::<GraphqlWireType>()
        .map(|wire| (wire.rust_type)())
        .collect();
    let resolved: HashSet<(&str, &str)> = inventory::iter::<GraphqlFieldRegistration>()
        .map(|field| ((field.parent)(), field.field))
        .collect();
    let mut refused: Vec<String> = inventory::iter::<GraphqlFieldRegistration>()
        .filter_map(|field| {
            let parent = served.serving((field.parent)(), field.field)?;
            let unread = unread_row(field.unmasked?, &wire, &resolved)?;
            Some(format!("`{parent}.{}` holds {unread}", field.field))
        })
        .collect();
    if !refused.is_empty() {
        refused.sort_unstable();
        anyhow::bail!(
            "a field's answer holds an `#[expose]`d wire type's row where no mask reads it, so \
             it would ship what a grant withholds: {}. Serve that column from a \
             `#[field_resolver]`, which masks what it answers, or answer the wire type itself",
            refused.join("; ")
        );
    }
    Ok(())
}

/// The first wire type's row `answer`'s type holds on a link no mask reads, as a refusal
/// says it: every column of an object not a wire type itself, but one a
/// `#[field_resolver]` serves, whose own answer the boot reads.
fn unread_row(
    answer: fn(&mut Registry) -> String,
    wire: &HashSet<&str>,
    resolved: &HashSet<(&str, &str)>,
) -> Option<String> {
    let mut registry = Registry::default();
    let answered = answer(&mut registry);
    let is_wire = |ty: &MetaType| ty.rust_typename().is_some_and(|name| wire.contains(name));

    let mut seen = HashSet::new();
    let mut pending = vec![(MetaTypeName::concrete_typename(&answered), String::new())];
    while let Some((name, at)) = pending.pop() {
        if !seen.insert(name) {
            continue;
        }
        let Some(ty) = registry.types.get(name) else {
            continue;
        };
        if is_wire(ty) {
            return Some(format!("`{name}`{at}"));
        }
        for (held, column) in links_of(ty) {
            if column
                .zip(ty.rust_typename())
                .is_some_and(|(column, rust)| resolved.contains(&(rust, column)))
            {
                continue;
            }
            let at = column.map_or_else(
                || format!(" at `{name}`"),
                |column| format!(" at `{name}.{column}`"),
            );
            pending.push((held, at));
        }
    }

    // A flattened object and a merged one copy a row's columns and leave no link to
    // it, yet each registers it: a wire type nothing links is one of those.
    let linked: HashSet<&str> = registry
        .types
        .values()
        .flat_map(links_of)
        .map(|(held, _)| held)
        .collect();
    registry
        .types
        .iter()
        .find(|(name, ty)| is_wire(ty) && !linked.contains(name.as_str()))
        .map(|(name, _)| format!("`{name}`"))
}

/// The types `ty` links to, each beside the column holding it: its fields', and a
/// union's or an interface's members, which no column holds.
fn links_of(ty: &MetaType) -> impl Iterator<Item = (&str, Option<&str>)> {
    let columns = ty.fields().into_iter().flatten().map(|(column, field)| {
        (
            MetaTypeName::concrete_typename(&field.ty),
            Some(column.as_str()),
        )
    });
    let members = ty
        .possible_types()
        .into_iter()
        .flatten()
        .map(|member| (member.as_str(), None));
    columns.chain(members)
}
