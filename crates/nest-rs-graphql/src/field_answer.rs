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
