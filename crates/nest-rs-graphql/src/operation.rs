//! What runs one GraphQL field a resolver declares: the [`Site`] its decorator
//! declares, whose chains each container composes once, dispatched through
//! the pipeline — a root field as a unit of its own, a field resolver inside
//! the root field that produced its parent.

use std::any::{Any, TypeId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, ready};

use async_graphql::parser::types::OperationType;
use async_graphql::{Context, Error as GraphqlError};
use futures_util::TryFutureExt as _;
use futures_util::future::BoxFuture;
use nest_rs_core::__private::{
    GlobalLayers, GuardStage, ResolvedLayer, Site, Tail, dispatch, new_unit_context,
    report_unresolved, settled,
};
use nest_rs_core::operation_log::{FiledOnce, Line, Unwinding};
use nest_rs_core::{
    Container, Failure, Handler, Outcome, Posture, Problem, ServedAddress, UnitContext,
    UnresolvedLayerError,
};
use nest_rs_core::{anyhow, code};
use nest_rs_guards::__private::{
    GlobalBucket, GuardSpec, NeededPrincipal, RefusalMark, SiteChainSources, carried_stage_idle,
    check_guards, compose_site_chain, validate_carried_unit,
};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::carried_request_chain;
use tracing::Instrument;

use crate::error::{AnsweredError, FieldFailure, Reply, answer, rendered};
use crate::field_spellings::spelled_by_schema;
use crate::view::{FieldRole, GraphqlView};

/// Which GraphQL site a field is — what decides whether its guard stage folds
/// the app-wide pool, and the role its field plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphqlSite {
    /// A `#[query]` / `#[mutation]` / `#[subscription]`: the app-wide pool runs
    /// here, each pooled guard's `check`, which the `/graphql` request's
    /// `check_http` never stands in for.
    Operation,
    /// A `#[field_resolver]`: its resolver's and method's guards run, the pool
    /// does not — the root field of the same request already ran it, and
    /// folding it again would run it once per parent.
    Field,
    /// An `#[entity]`, reached only through `_entities`, whose federation site
    /// runs the pool once per field; folding it here would run it once per
    /// representation.
    Entity,
    /// `_service` or `_entities`, which no resolver declares: `_entities`' whole
    /// chain is the pool, `_service`'s adds what else its request ran that
    /// identifies a caller.
    Federation,
}

impl GraphqlSite {
    /// Whether this site's guard stage folds the app-wide pool.
    ///
    /// An entity skips it only where the federation site runs it: a global
    /// pool can be absent, and skipping then would leave nothing to skip.
    fn bucket(self, container: &Container) -> GlobalBucket {
        match self {
            Self::Operation | Self::Federation => GlobalBucket::Fold,
            Self::Field => GlobalBucket::Skip,
            Self::Entity if crate::federation::gate_runs(container) => GlobalBucket::Skip,
            Self::Entity => GlobalBucket::Fold,
        }
    }

    /// The role this site's field plays.
    fn role(self) -> FieldRole {
        match self {
            Self::Operation => FieldRole::RootField,
            Self::Field => FieldRole::Nested,
            Self::Entity => FieldRole::Entity,
            Self::Federation => FieldRole::Federation,
        }
    }
}

/// What a root field folds of the app-wide layers: the exception filters and
/// the handler stage's interceptors, never the unit stage its `/graphql`
/// request already ran. A `#[field_resolver]`, no unit at all, folds none
/// ([`GlobalLayers::NONE`]).
pub const ROOT_FIELD: GlobalLayers = GlobalLayers {
    unit_stage: false,
    exception_filters: true,
    handler_stage: true,
};

/// The guards one field's site declares, as `#[operations]` collects them.
pub struct FieldGuards {
    /// `#[use_guards]` on the resolver.
    pub host: Vec<GuardSpec>,
    /// `#[use_guards]` beside the method, `#[force_guards]` included.
    pub method: Vec<GuardSpec>,
    /// `#[force_guards]`: the guards the method re-runs.
    pub force: Vec<TypeId>,
    /// Which site the field is.
    pub site: GraphqlSite,
    /// The field as a chain's log line and a refusal name it
    /// (`query listUsers`, `UsersResolver.posts`).
    pub label: &'static str,
    /// The posture withholds the ability from the field's reads: no mask shapes
    /// its answer.
    pub withheld: bool,
    /// The posture declares a class gate, which reads the ability a guard attached.
    pub class_gated: bool,
    /// What the field's `Ambient<T>` parameters read, by type.
    pub ambient: Vec<TypeId>,
}

/// The guard stage of the field `declared` describes, whose handler is
/// `handler`: the app-wide pool where the site folds it, the resolver's guards
/// and the method's, each once, each running its `check` on the field — `None`
/// when it runs none, needs no principal and has nothing to withhold.
///
/// A `#[public]` field, or a `#[field_resolver]`, which takes its root's posture,
/// reading as an `Ambient<P>` the principal a guard of its chain or its request
/// produces refuses a caller they admitted without it.
pub fn field_guard_stage(
    container: &Container,
    handler: &Handler,
    declared: FieldGuards,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    let FieldGuards {
        host,
        method,
        force,
        site,
        label,
        withheld,
        class_gated,
        ambient,
    } = declared;
    let sources = SiteChainSources {
        provider: host,
        method,
        force,
    };
    let chain = compose_site_chain(container, label, sources, site.bucket(container))?;
    let request_chain = || {
        carried_request_chain::<GraphqlView>(container, label)
            .map_err(|unresolved| unresolved.to_string())
    };
    validate_carried_unit(
        label,
        handler.posture(),
        class_gated,
        false,
        &chain,
        request_chain,
    )
    .map_err(anyhow::Error::msg)?;
    let public = handler.posture() == Posture::Public || site == GraphqlSite::Field;
    let needed = NeededPrincipal::carried(public, &ambient, &chain, request_chain)
        .map_err(anyhow::Error::msg)?;
    if needed.is_none()
        && carried_stage_idle(&chain, withheld, request_chain).map_err(anyhow::Error::msg)?
    {
        return Ok(None);
    }
    Ok(Some(Arc::new(FieldGuardStage {
        guards: chain,
        needed,
        denied: DENIED_OPERATION,
        withheld,
    })))
}

/// One field's guards, as the pipeline runs a site's guard stage: each one's
/// [`check`](Guard::check) on the field, read through [`GraphqlView`].
pub(crate) struct FieldGuardStage {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    needed: Option<NeededPrincipal>,
    denied: &'static str,
    withheld: bool,
}

/// The line a guard's denial of a resolver's field is filed as.
pub(crate) const DENIED_OPERATION: &str = "guard denied the operation";

/// The line a guard's denial of a federation root field is filed as.
pub(crate) const DENIED_FEDERATION_FIELD: &str = "guard denied the federation field";

impl FieldGuardStage {
    /// The stage running `guards`, filing a denial as `denied`, or `None` when
    /// there are none.
    pub(crate) fn of(
        guards: Vec<ResolvedLayer<dyn Guard>>,
        denied: &'static str,
    ) -> Option<Arc<dyn GuardStage>> {
        (!guards.is_empty()).then(|| {
            Arc::new(Self {
                guards,
                needed: None,
                denied,
                withheld: false,
            }) as Arc<dyn GuardStage>
        })
    }
}

impl GuardStage for FieldGuardStage {
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            check_guards(&self.guards, self.needed.as_ref(), None, self.withheld, cx)
                .await
                .map_err(|refused| {
                    let Some(view) = cx.switch_to::<GraphqlView>() else {
                        return Failure::from(refused.into_denial(self.denied, cx));
                    };
                    let fact = |said: String| view.first_filed(format!("{} {said}", view.field()));
                    let denial = refused.into_denial_once(self.denied, cx, fact);
                    Failure::from(denial)
                })
        })
    }
}

/// Where the field `ctx` resolves is served, as its lines name it: under the
/// operation it runs in, by the name `site` declares it under, or by the schema's
/// where it names no site, as a relation `#[expose]` emits.
pub fn served_field<'c>(ctx: &'c Context<'_>, site: Option<&'static Site>) -> ServedAddress<'c> {
    ServedAddress::field(
        operation_label(ctx.query_env.operation.node.ty),
        site.map_or_else(|| field_name(ctx), |site| site.handler().address()),
    )
}

/// The field `ctx` resolves, as the client wrote it in the document (`listUsers`,
/// never an alias).
pub fn field_name<'a>(ctx: &'a Context<'_>) -> &'a str {
    ctx.item.node.name.node.as_str()
}

/// Run one dispatched root field as its own unit of work — `#[operations]`
/// wraps every `#[query]`, `#[mutation]` and `#[entity]` in this — through
/// `site`'s chains, the rest of the field being `tail`: its argument pipes, the
/// call and the posture's mask.
///
/// The line names the field as its `mounted` line does, by `host`, `method`,
/// `kind` and `field` (`listUsers`, not `list_users`). It files `ok`
/// or `error` on return, `cancelled` when dropped first, and `panic` when it
/// unwinds — caught only to be named, then resumed. A sibling torn down by
/// that unwind files `cancelled`: one panic is one `panic` line.
pub fn run_field<'a, T, F, Fut>(
    site: &'static Site,
    ctx: &'a Context<'_>,
    kind: GraphqlSite,
    tail: F,
) -> impl Future<Output = Result<T, GraphqlError>> + Send + 'a
where
    T: Send + 'a,
    F: FnOnce() -> Fut + Send + 'a,
    Fut: Future<Output = Result<T, FieldFailure>> + Send + 'a,
{
    let operation = ctx.query_env.operation.node.ty;
    let role = match kind {
        GraphqlSite::Entity => "entity",
        _ => operation_label(operation),
    };
    let unit = FieldUnit::new(
        site,
        ctx.data_unchecked::<Container>(),
        operation,
        kind.role(),
    );
    in_field_unit(
        site.handler(),
        operation,
        role,
        field_name(ctx),
        unit.run(tail)
            .map_err(move |error| spelled_by_schema(ctx, error)),
    )
}

/// `unit`, the root field `handler` declares, playing `role` under the name
/// `field` in an `operation`, as a `graphql.field` unit of its own: its span,
/// its line, its correlation and the request's scope, its panic named then
/// resumed.
pub(crate) fn in_field_unit<'a, F, T, E>(
    handler: &'static Handler,
    operation: OperationType,
    role: &'static str,
    field: &'a str,
    unit: F,
) -> impl Future<Output = Result<T, E>> + Send + 'a
where
    E: AnsweredError,
    F: Future<Output = Result<T, E>> + Send + 'a,
{
    let correlation = match nest_rs_core::__private::current_correlation() {
        Some(request) => request.child(),
        None => nest_rs_core::Correlation::minted(None),
    };
    let span = nest_rs_core::operation_span!(
        crate::unit::FIELD,
        &correlation,
        // Not OTel's `graphql.operation.type`: `role` adds `entity` to its enum.
        // `graphql.document` is omitted: it carries the caller's literals.
        graphql.operation.role = role,
        graphql.field.name = field,
    );
    let line = FiledOnce::new(
        FieldLine {
            handler,
            kind: operation_label(operation),
            correlation: correlation.clone(),
            span: span.clone(),
            started: std::time::Instant::now(),
        },
        Unwinding::Shared,
    );
    // Combinators, never an `async fn` taking the field's future: a debug
    // build would hold every copy of it on the stack polling it
    // (rust-lang/rust#62958).
    let contained = nest_rs_core::with_request_scope(
        nest_rs_core::current_request_scope(),
        correlation,
        nest_rs_core::panic::contain(unit),
    );
    Filed {
        line: Some(line),
        unit: contained,
    }
    .instrument(span)
}

/// Run a field that is no unit of its own — a `#[field_resolver]` call, a
/// `#[subscription]`'s opening — through `site`'s chains, inside the unit that
/// carries it: no line, no span.
pub fn run_nested<'a, T, F, Fut>(
    site: &'static Site,
    ctx: &'a Context<'_>,
    kind: GraphqlSite,
    tail: F,
) -> impl Future<Output = Result<T, GraphqlError>> + Send + 'a
where
    T: Send + 'a,
    F: FnOnce() -> Fut + Send + 'a,
    Fut: Future<Output = Result<T, FieldFailure>> + Send + 'a,
{
    let operation = ctx.query_env.operation.node.ty;
    let mut unit = FieldUnit::new(
        site,
        ctx.data_unchecked::<Container>(),
        operation,
        kind.role(),
    );
    // Run once per parent: what it files repeating is one line per operation.
    unit.view = unit.view.filing_into(
        ctx.data_opt::<Arc<crate::filed_facts::FiledFacts>>()
            .cloned(),
    );
    unit.run(tail)
        .map_err(move |error| spelled_by_schema(ctx, error))
}

/// The operation type as a field's line names its role.
pub(crate) fn operation_label(operation: OperationType) -> &'static str {
    match operation {
        OperationType::Query => "query",
        OperationType::Mutation => "mutation",
        OperationType::Subscription => "subscription",
    }
}

/// One field to dispatch: its site, the container its chains are composed
/// against, and the view its layers read.
pub(crate) struct FieldUnit<'a> {
    site: &'static Site,
    container: &'a Container,
    view: GraphqlView,
}

impl<'a> FieldUnit<'a> {
    /// The field `site` declares, in an `operation`, playing `role`.
    pub(crate) fn new(
        site: &'static Site,
        container: &'a Container,
        operation: OperationType,
        role: FieldRole,
    ) -> Self {
        Self {
            site,
            container,
            view: GraphqlView::new(site.handler().address(), operation, role),
        }
    }

    /// The field through its site's chains, the rest of it `tail`, answered
    /// by the edge's one rule.
    pub(crate) async fn run<T, F, Fut>(self, tail: F) -> Result<T, GraphqlError>
    where
        T: Send,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, FieldFailure>> + Send,
    {
        let handler = self.site.handler();
        let chains = match self.site.chains(self.container) {
            Ok(chains) => chains,
            Err(refused) => return Err(refused_site(handler, &refused)),
        };
        let mut slot = None;
        let outcome = {
            let mut view = self.view;
            let mut tail = FieldTail {
                run: Some(tail),
                slot: &mut slot,
            };
            let mut cx = new_unit_context(handler, &mut view, None, None);
            dispatch(&mut cx, &chains, &mut tail).await
        };
        answered(handler, outcome, slot)
    }
}

/// What a site whose chains did not compose answers: the opaque answer, its
/// cause filed — the layer and the site when a declared layer did not resolve.
/// No field runs without a layer its site declares.
fn refused_site(handler: &Handler, refused: &anyhow::Error) -> GraphqlError {
    match refused.downcast_ref::<UnresolvedLayerError>() {
        Some(unresolved) => report_unresolved(unresolved),
        None => tracing::error!(
            target: nest_rs_core::target::LAYERS,
            site = handler.address(),
            error = %nest_rs_core::error_message(refused.as_ref()),
            "a GraphQL field's layers did not compose; it is refused",
        ),
    }
    rendered(&Problem::new(500, code::INTERNAL))
}

/// What the field `handler` declares came to, as the client reads it: the
/// value its tail wrote, or its failure answered by the edge's one rule.
fn answered<T>(
    handler: &Handler,
    outcome: Outcome,
    slot: Option<Result<T, Box<Reply>>>,
) -> Result<T, GraphqlError> {
    let failure = match outcome {
        Ok(_) => {
            return match slot {
                Some(Ok(value)) => Ok(value),
                Some(Err(reply)) => Err(reply.filed()),
                None => {
                    tracing::error!(
                        target: crate::TARGET,
                        site = handler.address(),
                        "a field settled without a value; it is refused",
                    );
                    Err(rendered(&Problem::new(500, code::INTERNAL)))
                }
            };
        }
        Err(failure) => failure,
    };
    if failure.find::<Denial>().is_some()
        && let Some(mark) = nest_rs_core::ambient::<RefusalMark>()
    {
        mark.mark();
    }
    Err(answer(&failure, slot.and_then(Result::err)))
}

/// A field's tail as the pipeline runs it: `run`, its value written into the
/// field's own slot; a failure carrying an answer only its type could say
/// goes out answered, which the exception filters still see.
struct FieldTail<'s, T, F> {
    run: Option<F>,
    slot: &'s mut Option<Result<T, Box<Reply>>>,
}

impl<T, F, Fut> Tail for FieldTail<'_, T, F>
where
    T: Send,
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = Result<T, FieldFailure>> + Send,
{
    fn call(&mut self, _cx: &mut UnitContext<'_>) -> impl Future<Output = Outcome> + Send {
        let run = self.run.take();
        let slot = &mut *self.slot;
        async move {
            let Some(run) = run else {
                unreachable!("a unit runs its tail once");
            };
            match run().await {
                Ok(value) => {
                    *slot = Some(Ok(value));
                    Ok(settled())
                }
                Err(FieldFailure {
                    failure,
                    reply: Some(reply),
                }) => {
                    *slot = Some(Err(reply));
                    Err(failure.answered(settled()))
                }
                Err(FieldFailure {
                    failure,
                    reply: None,
                }) => Err(failure),
            }
        }
    }
}

pin_project_lite::pin_project! {
    /// A root field's unit with its line, filed once it ends: `ok` or `error`
    /// by what it answered, `panic` when it unwound — the unwind then resumed.
    struct Filed<F> {
        line: Option<FiledOnce<FieldLine>>,
        #[pin]
        unit: F,
    }
}

impl<T, E, F> Future for Filed<F>
where
    E: AnsweredError,
    F: Future<Output = Result<Result<T, E>, Box<dyn Any + Send>>>,
{
    type Output = Result<T, E>;

    fn poll(self: Pin<&mut Self>, task: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let ended = ready!(this.unit.poll(task));
        let line = this.line.take();
        match ended {
            Ok(answered) => {
                if let Some(line) = line {
                    FieldLine::answered(line, &answered);
                }
                Poll::Ready(answered)
            }
            Err(unwound) => {
                if let Some(line) = line {
                    line.file(nest_rs_core::operation_log::PANIC);
                }
                std::panic::resume_unwind(unwound)
            }
        }
    }
}

/// One root field's `graphql.field` line, `cancelled` when dropped first.
///
/// The correlation is held: a `Drop` runs while the future is torn down, not
/// reliably inside the scope that future installed.
struct FieldLine {
    handler: &'static Handler,
    kind: &'static str,
    correlation: nest_rs_core::Correlation,
    span: tracing::Span,
    started: std::time::Instant,
}

impl FieldLine {
    /// File what the field answered: `ok`, or `error`, which its span records
    /// unless its caller's request earned it, as an HTTP `4xx` fails nothing on
    /// the server's span.
    fn answered<T, E: AnsweredError>(mut line: FiledOnce<Self>, answered: &Result<T, E>) {
        if answered
            .as_ref()
            .is_err_and(|error| !error.failed_the_server())
        {
            line.span = tracing::Span::none();
        }
        line.file(if answered.is_ok() {
            nest_rs_core::operation_log::OK
        } else {
            nest_rs_core::operation_log::ERROR
        });
    }
}

impl Line for FieldLine {
    fn file(&mut self, outcome: &'static str) {
        nest_rs_core::RequestContinuation::new(None, self.correlation.clone()).enter(|| {
            nest_rs_core::operation_line!(
                crate::unit::FIELD,
                span: &self.span,
                outcome: outcome,
                started: self.started,
                host = self.handler.host_name(),
                // A federation field dispatches to no method of its own.
                method = Some(self.handler.method()).filter(|method| !method.is_empty()),
                kind = self.kind,
                field = self.handler.address(),
            );
        });
    }
}

/// A `#[subscription]`'s answer, accepted only when it is a value — a stream.
/// async-graphql's subscription derive reads a fallible return by its spelling,
/// so a `Result` under another name is refused here.
#[diagnostic::on_unimplemented(
    message = "a `#[subscription]` answers a stream, and this one returns a `Result` spelled \
               another way",
    label = "a `Result` under another name",
    note = "write it `Result<impl Stream<Item = T>, E>`: async-graphql reads a fallible return by \
            its spelling — `Result` or `FieldResult` — and takes any other for the stream itself"
)]
pub trait IsStreamReturn {}

impl IsStreamReturn for nest_rs_core::__private::ValueAnswer {}

/// Accept a `#[subscription]`'s answer — see [`IsStreamReturn`].
pub fn answers_a_stream<K: IsStreamReturn>(_: K) {}

#[cfg(test)]
mod tests {
    use nest_rs_core::__private::{
        HandlerDeclaration, MetaLevels, SiteDeclaration, SiteLayers, declare_handler, declare_site,
    };
    use nest_rs_core::Posture;

    use super::*;

    struct Fields;

    static FIELD: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::FIELD,
        host: TypeId::of::<Fields>(),
        host_name: "Fields",
        method: "field",
        address: "field",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    static SITE: Site = declare_site(
        &FIELD,
        SiteDeclaration {
            layers: SiteLayers::default,
            global: ROOT_FIELD,
            guard_stage: None,
            class_gate: None,
        },
    );

    /// With no guard at the field or at the `/graphql` request, nothing can have
    /// attached what a bare `#[public]` withholds, so it keeps no stage.
    #[test]
    fn a_field_with_nothing_to_withhold_pays_for_no_stage() {
        let stage = field_guard_stage(
            &Container::default(),
            &FIELD,
            FieldGuards {
                host: Vec::new(),
                method: Vec::new(),
                force: Vec::new(),
                site: GraphqlSite::Operation,
                label: "query field",
                withheld: true,
                class_gated: false,
                ambient: Vec::new(),
            },
        )
        .expect("no guard to resolve");
        assert!(stage.is_none());
    }

    /// What a field's site adds to the future of a tail holding `N` bytes:
    /// every stage between the wrapper and the developer's method.
    fn added_by_the_site<const N: usize>(container: &Container) -> usize {
        let unit = FieldUnit {
            site: &SITE,
            container,
            view: GraphqlView::new("field", OperationType::Query, FieldRole::RootField),
        };
        let holding = || async {
            let held = [0u8; N];
            tokio::task::yield_now().await;
            Ok::<_, FieldFailure>(held.len())
        };
        let tail = size_of_val(&holding());
        size_of_val(&unit.run(holding)) - tail
    }

    /// A debug build reserves every copy of a future a frame holds on the
    /// thread polling it: a site copying its tail's would multiply the stack
    /// a field needs by the stages it runs.
    #[test]
    fn a_fields_site_holds_its_tails_future_once() {
        let container = Container::default();
        let light = added_by_the_site::<1024>(&container);
        let heavy = added_by_the_site::<16384>(&container);
        assert_eq!(
            light, heavy,
            "a field's site copies its tail's future: it adds {light} bytes to a 1 KiB one, \
             {heavy} to a 16 KiB one",
        );
    }

    #[tokio::test]
    async fn a_field_answers_the_value_its_tail_wrote() {
        let container = Container::default();
        let unit = FieldUnit {
            site: &SITE,
            container: &container,
            view: GraphqlView::new("field", OperationType::Query, FieldRole::RootField),
        };
        let answered = unit.run(|| async { Ok::<_, FieldFailure>(7) }).await;
        assert_eq!(answered.ok(), Some(7));
    }

    #[test]
    fn a_field_settled_without_a_value_is_refused_naming_its_site() {
        let logs = nest_rs_testing::LogCapture::install();

        let answer = answered::<i32>(&FIELD, Ok(settled()), None);

        assert!(answer.is_err());
        let line = logs.expect_one(
            crate::TARGET,
            "a field settled without a value; it is refused",
        );
        assert!(
            line.field("site").is_some_and(|site| site == "field"),
            "{:?}",
            line.fields,
        );
    }
}
