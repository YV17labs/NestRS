//! What a `#[field_resolver]` answers of an `#[expose]`d entity's wire type, which
//! that entity's response mask shapes. A nested field names no posture, so the
//! expansion asks this probe: inherent methods, which method resolution prefers,
//! exist only for an answer holding a wire type's rows, and [`FieldAnswerFallback`]
//! leaves every other answer as it is. An answer is walked down to its rows, so what
//! holds them owes the mask no serde impl.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

use async_graphql::connection::{
    Connection, ConnectionNameType, CursorType, EdgeNameType, NodesFieldSwitcherSealed,
};
use async_graphql::{Context, Error, Json, ObjectType, OutputType};
use nest_rs_guards::Denial;

use crate::error::denial_to_graphql_error;

/// An answer holding an `#[expose]`d wire type's rows.
pub trait WireAnswer: Sized {
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

impl<A: WireAnswer> WireAnswer for Option<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_option(self, ctx)
    }
}

impl<A: WireAnswer> WireAnswer for Vec<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self, ctx)
    }
}

impl<A: WireAnswer> WireAnswer for VecDeque<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self.into(), ctx).map(Into::into)
    }
}

impl<A: WireAnswer> WireAnswer for LinkedList<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = A::masked_list(self.into_iter().collect(), ctx)?;
        Ok(rows.into_iter().collect())
    }
}

impl<A: WireAnswer + Ord> WireAnswer for BTreeSet<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = A::masked_list(self.into_iter().collect(), ctx)?;
        Ok(rows.into_iter().collect())
    }
}

impl<A: WireAnswer + Eq + Hash, S: BuildHasher + Default> WireAnswer for HashSet<A, S> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        let rows = A::masked_list(self.into_iter().collect(), ctx)?;
        Ok(rows.into_iter().collect())
    }
}

impl<A: WireAnswer> WireAnswer for Box<[A]> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self.into_vec(), ctx).map(Vec::into_boxed_slice)
    }
}

impl<A: WireAnswer + Clone> WireAnswer for Arc<[A]> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        A::masked_list(self.to_vec(), ctx).map(Into::into)
    }
}

/// Each row on its own: an array has no place for one a list would drop, so a row
/// the mask refuses refuses its selected columns.
impl<A: WireAnswer, const N: usize> WireAnswer for [A; N] {
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
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_iter()
            .map(|(key, row)| Ok((key, row.masked(ctx)?)))
            .collect()
    }
}

impl<A: WireAnswer> WireAnswer for Box<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        (*self).masked(ctx).map(Box::new)
    }
}

impl<A: WireAnswer + Clone> WireAnswer for Arc<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        Self::unwrap_or_clone(self).masked(ctx).map(Self::new)
    }
}

impl<A: WireAnswer + Clone> WireAnswer for Cow<'static, A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.into_owned().masked(ctx).map(Cow::Owned)
    }
}

impl<A: WireAnswer> WireAnswer for Json<A> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        self.0.masked(ctx).map(Json)
    }
}

impl<A: WireAnswer, E> WireAnswer for Result<A, E> {
    fn masked(self, ctx: &Context<'_>) -> Result<Self, Error> {
        match self {
            Ok(answer) => answer.masked(ctx).map(Ok),
            Err(err) => Ok(Err(err)),
        }
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
    /// The entity's mask.
    pub fn mask(&self) -> fn(&Context<'_>, T) -> Result<T, Error> {
        |ctx, answer| answer.masked(ctx)
    }
}

/// What [`FieldAnswer`] answers for a value holding no wire type's rows.
pub trait FieldAnswerFallback {
    /// The answer's own type.
    type Value;

    /// The answer as it is: a body's scalar or its own DTO names no entity.
    fn mask(&self) -> fn(&Context<'_>, Self::Value) -> Result<Self::Value, Error>;
}

impl<T> FieldAnswerFallback for FieldAnswer<'_, T> {
    type Value = T;

    fn mask(&self) -> fn(&Context<'_>, T) -> Result<T, Error> {
        |_, answer| Ok(answer)
    }
}
