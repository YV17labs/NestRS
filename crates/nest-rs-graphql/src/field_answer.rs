//! A root field's answer, resolved under the unit that built it. async-graphql
//! resolves what a root field returned once the field's own future returned, so
//! what judged the rows it built — the ability its own guards attached, which its
//! request may not carry — is merged back over that resolution
//! ([`AnswerContext`]), a subscription's around each poll and each item, and
//! a batch below it reads through loaders of the answer's own. A
//! refusal a serializer meets there — a row inside a `Json` scalar, which
//! async-graphql (`7.2.1`) writes itself and answers `null` for — is the field's
//! answer, filed naming it.

use std::borrow::Cow;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use async_graphql::futures_util::Stream;
use async_graphql::parser::types::{Field, OperationType};
use async_graphql::registry::Registry;
use async_graphql::{ContextSelectionSet, Data, OutputType, Positioned, ServerResult, Value};
use nest_rs_core::__private::{AnswerContext, RefusedAnswer, Site, answering};

use crate::loader::LoaderSeeds;
use crate::root_fields::file_below_root;

/// What a root field answers, held with what its unit judged it under: its
/// schema is the value's.
pub struct FieldAnswer<T> {
    value: T,
    unit: AnswerContext,
    site: &'static Site,
}

impl<T> FieldAnswer<T> {
    /// `value`, answered by the root field `site` declares, captured inside its
    /// unit.
    pub fn new(value: T, site: &'static Site) -> Self {
        Self {
            value,
            unit: AnswerContext::capture(),
            site,
        }
    }

    /// Loaders of the answer's own where its unit holds what its request does not
    /// — an ability the field's own guards attached: the request's are shared by
    /// every root field reading through them, whose units may hold others. `None`
    /// where the request's values judge the answer, on a subscription, whose
    /// operation holds one root field (`operation_units` refuses another before
    /// it opens), and where an extension handed the resolution data of its own,
    /// which this cannot extend.
    fn loaders(&self, ctx: &ContextSelectionSet<'_>) -> Option<Data> {
        if self.unit.is_empty()
            || ctx.execute_data.is_some()
            || ctx.query_env.operation.node.ty == OperationType::Subscription
        {
            return None;
        }
        LoaderSeeds::seed_own(ctx)
    }
}

impl<T: OutputType> OutputType for FieldAnswer<T> {
    fn type_name() -> Cow<'static, str> {
        T::type_name()
    }

    fn qualified_type_name() -> String {
        T::qualified_type_name()
    }

    fn introspection_type_name(&self) -> Cow<'static, str> {
        self.value.introspection_type_name()
    }

    fn create_type_info(registry: &mut Registry) -> String {
        T::create_type_info(registry)
    }

    async fn resolve(
        &self,
        ctx: &ContextSelectionSet<'_>,
        field: &Positioned<Field>,
    ) -> ServerResult<Value> {
        let loaders = self.loaders(ctx);
        let own = loaders.as_ref().map(|loaders| {
            ctx.query_env
                .create_context(ctx.schema_env, ctx.path_node, ctx.item, Some(loaders))
        });
        let resolving = own.as_ref().unwrap_or(ctx);
        let (resolved, refused) = Claimed {
            inner: self.unit.scope(self.value.resolve(resolving, field)),
            refused: None,
        }
        .await;
        let Some(refused) = refused else {
            return resolved;
        };
        file_below_root(ctx, &refused, Some(self.site));
        let answered = crate::problem_error(&refused.refusal().problem());
        Err(ctx.set_error_path(answered.into_server_error(field.pos)))
    }
}

pin_project_lite::pin_project! {
    /// What a `#[subscription]` streams: each poll under what its opening's unit
    /// judged its rows by, each item a [`FieldAnswer`] resolved under it too.
    pub struct FieldAnswers<S> {
        #[pin]
        stream: S,
        unit: AnswerContext,
        site: &'static Site,
    }
}

impl<S> FieldAnswers<S> {
    /// `stream`, opened by the subscription `site` declares, captured inside its
    /// opening's unit.
    pub fn new(stream: S, site: &'static Site) -> Self {
        Self {
            stream,
            unit: AnswerContext::capture(),
            site,
        }
    }
}

/// The stream a `#[subscription]` answers by a type it names, read for what it
/// yields where the field's posture filters its items: the filtered stream's
/// own type has no name for the field to answer.
#[diagnostic::on_unimplemented(
    message = "a `#[subscription]` answers a stream, and `{Self}` is none",
    label = "no stream",
    note = "answer `impl Stream<Item = T>`, a stream type such as `BoxStream<'static, T>`, or one \
            of those inside `Result<…>`: async-graphql reads a fallible return by its spelling — \
            `Result` or `FieldResult` — and takes any other for the stream itself"
)]
pub trait AnsweredStream {
    /// What the stream yields.
    type Item;
}

#[diagnostic::do_not_recommend]
impl<S: Stream> AnsweredStream for S {
    type Item = S::Item;
}

impl<S: Stream> Stream for FieldAnswers<S> {
    type Item = FieldAnswer<S::Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let unit = &*this.unit;
        let site = *this.site;
        unit.enter(|| this.stream.poll_next(cx)).map(|item| {
            item.map(|value| FieldAnswer {
                value,
                unit: unit.clone(),
                site,
            })
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

pin_project_lite::pin_project! {
    /// `inner`, each poll run as an edge's serializer runs: the first refusal a
    /// serializer meets there is handed back beside its output.
    struct Claimed<F> {
        #[pin]
        inner: F,
        refused: Option<RefusedAnswer>,
    }
}

impl<F: Future> Future for Claimed<F> {
    type Output = (F::Output, Option<RefusedAnswer>);

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let inner = this.inner;
        let (polled, refused) = answering(move || inner.poll(cx));
        if this.refused.is_none() {
            *this.refused = refused;
        }
        polled.map(|output| (output, this.refused.take()))
    }
}

/// A field's answer `R`, asked whether it is a `Result` under another name:
/// async-graphql reads a fallible answer by its spelling and takes any other
/// for the field's own type, which such a `Result` is only where its error is
/// one async-graphql answers itself ([`AliasedError`]). Inherent for a
/// `Result`, [`AliasedAnswerFallback`] for every other type.
pub struct AliasedAnswer<R>(PhantomData<fn() -> R>);

impl<R> AliasedAnswer<R> {
    /// The question, asked of `R`.
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<R> Default for AliasedAnswer<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, E> AliasedAnswer<Result<T, E>> {
    /// A `Result` whose error async-graphql answers. Bound on the method, never
    /// on the impl: an impl's unmet bound hands the call to the fallback.
    pub fn spelled(&self)
    where
        E: AliasedError,
    {
    }
}

/// What [`AliasedAnswer::spelled`] says of an answer that is no `Result`.
pub trait AliasedAnswerFallback {
    /// A value: nothing to spell.
    fn spelled(&self) {}
}

impl<R> AliasedAnswerFallback for AliasedAnswer<R> {}

/// Where an `#[operations]` block asks each answer it reads as a value
/// ([`AliasedAnswer`]), `M` a type of the block's own. An impl's future proves
/// `Send` by its body, which rustc therefore checks with the impl, ahead of the
/// impls the GraphQL derive writes after it: the sentence leads the bounds
/// those then miss.
pub trait AliasedAnswers<M> {
    /// Asks each answer.
    fn asked() -> impl Future<Output = ()> + Send;
}

/// The error of a `Result` a field answers under another name: the whole
/// `Result` is then the field's GraphQL type, so its error is async-graphql's
/// to answer.
#[diagnostic::on_unimplemented(
    message = "a field answering a `Result` under another name answers it as its GraphQL type, \
               which no `Result` failing with `{Self}` is",
    label = "a `Result` under another name",
    note = "write it `Result<…>` or `FieldResult<…>`: async-graphql reads a fallible answer by \
            that spelling, and the field then answers the error by its own type"
)]
pub trait AliasedError {}

#[diagnostic::do_not_recommend]
impl<E: Into<async_graphql::Error> + Send + Sync + Clone> AliasedError for E {}
