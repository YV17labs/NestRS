//! What one MCP operation *is*, and the sites it runs through on core's
//! pipeline: every request's own [`REQUEST`] site, and the site `#[tools]`
//! declares for each tool and prompt, run inside it.

use std::any::TypeId;
use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use nest_rs_core::__private::{
    GlobalLayers, GuardStage, HandlerDeclaration, MetaLevels, ResolvedLayer, Site, SiteDeclaration,
    SiteLayers, Tail, compose_chain, declare_handler, declare_site, dedup_bucket, dispatch,
    log_effective_chain, new_unit_context, report_unresolved, resolve_scoped, settled,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{
    Container, Deadline, Failure, Handler, Outcome, Posture, UnitContext, UnresolvedLayerError,
    anyhow,
};
use nest_rs_guards::__private::{RefusalMark, Refused, failure_chain, resolve_global_guards};
use nest_rs_guards::{Denial, Guard, GuardSpec};
use rmcp::model::{CallToolRequestMethod, ConstString, GetPromptRequestMethod};

use crate::McpError;
use crate::error::{OperationFailure, Reply, answer, opaque_answer};
use crate::module::McpModule;
use crate::view::McpView;

/// The container serving the current MCP operation, read off the ambient request
/// scope; `None` outside a dispatch nested under the HTTP transport edge.
pub fn current_container() -> Option<Container> {
    crate::scope::current_scope().map(|scope| scope.root().clone())
}

/// Which router an operation belongs to — the two roles `#[tools]` serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpOperationKind {
    /// A `#[tool]` method, reached by `tools/call`.
    Tool,
    /// A `#[prompt]` method, reached by `prompts/get`.
    Prompt,
}

impl McpOperationKind {
    /// The lowercase word this role is logged and labelled under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Prompt => "prompt",
        }
    }

    /// The JSON-RPC method that reaches this role's router.
    pub fn method(self) -> &'static str {
        match self {
            Self::Tool => CallToolRequestMethod::VALUE,
            Self::Prompt => GetPromptRequestMethod::VALUE,
        }
    }
}

impl fmt::Display for McpOperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What every request's own site folds of the app-wide layers: the unit
/// stage, once around whatever the request reaches.
const REQUEST_LAYERS: GlobalLayers = GlobalLayers {
    unit_stage: true,
    exception_filters: false,
    handler_stage: false,
};

/// What a tool's or a prompt's site folds of the app-wide layers: its
/// exception filters and handler-stage interceptors, never the unit stage the
/// operation's own request site ran around it.
pub const OPERATION_LAYERS: GlobalLayers = GlobalLayers {
    unit_stage: false,
    exception_filters: true,
    handler_stage: true,
};

/// The declaration of every request's own site, behind the guards at the
/// endpoint's request.
static REQUEST_HANDLER: Handler = declare_handler(HandlerDeclaration {
    unit: crate::unit::OPERATION,
    host: TypeId::of::<McpModule>(),
    host_name: "McpModule",
    method: "",
    address: "*",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

/// The site every MCP request runs as — the unit stage alone; a tool call or
/// a prompt fetch then reaches its own site inside it.
pub(crate) static REQUEST: Site = declare_site(
    &REQUEST_HANDLER,
    SiteDeclaration {
        layers: SiteLayers::default,
        global: REQUEST_LAYERS,
        guard_stage: None,
        class_gate: None,
    },
);

/// The guards one tool's or prompt's site declares, as `#[tools]` collects
/// them.
pub struct OperationGuards {
    /// `#[use_guards]` on the host.
    pub host: Vec<GuardSpec>,
    /// `#[use_guards]` beside the method, `#[force_guards]` included.
    pub method: Vec<GuardSpec>,
    /// `#[force_guards]`: the guards the method re-runs.
    pub force: Vec<TypeId>,
    /// Which router serves the operation.
    pub kind: McpOperationKind,
    /// The host's type, as a denial's line names it.
    pub host_type: &'static str,
}

/// The guard stage of the operation `handler` declares: the app-wide pool, the
/// host's guards and the method's, each once, each running its
/// [`check`](Guard::check) on the operation — `None` when it runs none.
///
/// The pool is part of it, as on a GraphQL root field: the endpoint's request
/// runs `check_http`, never `check`, so leaving it out here would leave a
/// global guard overriding only `check` never consulted.
pub fn operation_guard_stage(
    container: &Container,
    handler: &Handler,
    declared: OperationGuards,
) -> anyhow::Result<Option<Arc<dyn GuardStage>>> {
    let label = format!(
        "{} {}::{}",
        declared.kind,
        handler.host_name(),
        handler.method()
    );
    let host = resolve_scoped(container, &declared.host, LayerSite::Host, &label)?;
    let method = resolve_scoped(container, &declared.method, LayerSite::Method, &label)?;
    let chain = compose_chain::<dyn Guard>(
        dedup_bucket(resolve_global_guards(container)),
        host,
        method,
        &declared.force,
        &label,
    );
    log_effective_chain(&label, "guards", &chain);
    if chain.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(OperationGuardStage {
        guards: chain,
        kind: declared.kind,
        host: declared.host_type,
    })))
}

/// One operation's guards, as the pipeline runs a site's guard stage: each
/// one's [`check`](Guard::check) on the operation, read through [`McpView`].
struct OperationGuardStage {
    guards: Vec<ResolvedLayer<dyn Guard>>,
    kind: McpOperationKind,
    host: &'static str,
}

impl GuardStage for OperationGuardStage {
    fn check<'a>(&'a self, cx: &'a mut UnitContext<'_>) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            for entry in &self.guards {
                // `as_ref()`: dispatch on the erased guard — the `Guard for
                // Arc<T>` blanket would nest a second boxed future per check.
                let guard = entry.layer.as_ref();
                if let Err(denial) = guard.check(cx).await {
                    let operation = cx.handler().address();
                    let denial = Refused::by(entry.name, guard, denial)
                        .into_denial(|guard, denial| self.file(guard, operation, denial));
                    return Err(Failure::from(denial));
                }
            }
            Ok(())
        })
    }
}

impl OperationGuardStage {
    /// The edge's structural floor under a denial `guard` raised on
    /// `operation`; host and operation stay separate fields for an incident
    /// query.
    fn file(&self, guard: &'static str, operation: &str, denial: &Denial) {
        tracing::warn!(
            target: nest_rs_core::target::LAYERS,
            guard,
            host = self.host,
            kind = self.kind.as_str(),
            operation,
            status = denial.http_status(),
            error = failure_chain(denial),
            "guard denied the operation",
        );
    }
}

/// Run one tool call or prompt fetch through the site `#[tools]` declared for
/// it — `#[tools]` wraps every operation in this — the rest of it being
/// `tail`: its `Scoped<T>` parameters, its pipes, the call and the posture's
/// mask, answering the operation's value.
///
/// It runs inside the operation's own unit, whose site ran the unit stage. A
/// site whose chains do not compose is refused opaquely, its cause filed: no
/// operation runs without a layer it declares.
pub async fn run_operation<T, F, Fut>(
    site: &'static Site,
    view: McpView,
    tail: F,
) -> Result<T, McpError>
where
    T: Send,
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = Result<T, OperationFailure>> + Send,
{
    // `tail` is a closure: the operation's future is built in `dispatch`'s
    // own frame, held once (rust-lang/rust#62958).
    let handler = site.handler();
    // Composed against an empty container where no app is ambient: a site
    // declaring nothing still runs, one declaring a layer is refused.
    let container = current_container().unwrap_or_default();
    let chains = match site.chains(&container) {
        Ok(chains) => chains,
        Err(refused) => return Err(refused_site(handler, &refused)),
    };
    let deadline = nest_rs_core::ambient::<Deadline>().map(|deadline| deadline.0);
    let mut slot = None;
    let outcome = {
        let mut view = view;
        let mut tail = OperationTail {
            run: Some(tail),
            slot: &mut slot,
        };
        let mut cx = new_unit_context(handler, &mut view, None, deadline);
        dispatch(&mut cx, &chains, &mut tail).await
    };
    answered(handler, outcome, slot)
}

/// The view of the operation `name` of `host`, served by `kind`'s router.
pub fn operation_view(kind: McpOperationKind, name: &'static str, host: &'static str) -> McpView {
    McpView::declared(kind, name, host)
}

/// What a site whose chains did not compose answers: the opaque answer, its
/// cause filed — the layer and the site when a declared layer did not resolve.
fn refused_site(handler: &Handler, refused: &anyhow::Error) -> McpError {
    match refused.downcast_ref::<UnresolvedLayerError>() {
        Some(unresolved) => report_unresolved(unresolved),
        None => tracing::error!(
            target: nest_rs_core::target::LAYERS,
            site = handler.address(),
            error = %nest_rs_core::error_message(refused.as_ref()),
            "an MCP operation's layers did not compose; it is refused",
        ),
    }
    opaque_answer()
}

/// What the unit of the operation `handler` declares came to, as the client
/// reads it: the value its tail wrote, or its failure answered by the edge's
/// one rule.
fn answered<T>(
    handler: &Handler,
    outcome: Outcome,
    slot: Option<Result<T, Box<Reply>>>,
) -> Result<T, McpError> {
    match (outcome, slot) {
        (Ok(_), Some(Ok(value))) => Ok(value),
        (Ok(_), Some(Err(reply))) => Err(reply.filed()),
        (Ok(_), None) => {
            tracing::error!(
                target: crate::TARGET,
                site = handler.address(),
                "an MCP operation settled without a value; it is refused",
            );
            Err(opaque_answer())
        }
        (Err(failure), slot) => {
            if failure.find::<Denial>().is_some()
                && let Some(mark) = nest_rs_core::ambient::<RefusalMark>()
            {
                mark.mark();
            }
            Err(answer(&failure, slot.and_then(Result::err)))
        }
    }
}

/// An operation's tail as the pipeline runs it: `run`, its value written into
/// the operation's own slot; a failure carrying an answer only its type could
/// say goes out answered, which the exception filters still see.
struct OperationTail<'s, T, F> {
    run: Option<F>,
    slot: &'s mut Option<Result<T, Box<Reply>>>,
}

impl<T, F, Fut> Tail for OperationTail<'_, T, F>
where
    T: Send,
    F: FnOnce() -> Fut + Send,
    Fut: Future<Output = Result<T, OperationFailure>> + Send,
{
    async fn call(&mut self, _: &mut UnitContext<'_>) -> Outcome {
        let Some(run) = self.run.take() else {
            unreachable!("a unit runs its tail once");
        };
        match run().await {
            Ok(value) => {
                *self.slot = Some(Ok(value));
                Ok(settled())
            }
            Err(OperationFailure {
                failure,
                reply: Some(reply),
            }) => {
                *self.slot = Some(Err(reply));
                Err(failure.answered(settled()))
            }
            Err(OperationFailure {
                failure,
                reply: None,
            }) => Err(failure),
        }
    }
}

/// Whether an operation's description holds no prose — `str::trim` leaves
/// nothing of it. `const` so `#[tools]` can assert a non-literal description
/// (`include_str!`, a constant) at build time, which `str::trim` cannot.
pub const fn description_is_blank(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        // A `&str` is valid UTF-8, so the lead byte says how many continuation
        // bytes follow, and each carries six bits.
        let lead = bytes[at];
        let (mut code, width) = match lead {
            0x00..=0x7F => (lead as u32, 1),
            0xC0..=0xDF => ((lead & 0x1F) as u32, 2),
            0xE0..=0xEF => ((lead & 0x0F) as u32, 3),
            _ => ((lead & 0x07) as u32, 4),
        };
        let mut next = 1;
        while next < width {
            code = (code << 6) | (bytes[at + next] & 0x3F) as u32;
            next += 1;
        }
        match char::from_u32(code) {
            Some(character) if character.is_whitespace() => at += width,
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `str::trim` is the oracle.
    #[test]
    fn a_description_is_blank_exactly_when_trim_leaves_nothing() {
        for character in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            let alone = character.to_string();
            assert_eq!(
                description_is_blank(&alone),
                alone.trim().is_empty(),
                "{:?}",
                character,
            );
        }
        for text in [
            "",
            " ",
            "\t\n\r",
            "\u{3000}\u{a0}",
            " x ",
            "\u{3000}é\u{3000}",
            "😀",
        ] {
            assert_eq!(
                description_is_blank(text),
                text.trim().is_empty(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_check_runs_in_a_constant() {
        const {
            assert!(description_is_blank(concat!(" ", "\u{3000}")));
            assert!(!description_is_blank(concat!(" Counts ", "items.")));
        }
    }

    struct Tools;

    static PING: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::OPERATION,
        host: TypeId::of::<Tools>(),
        host_name: "Tools",
        method: "ping",
        address: "ping",
        posture: Posture::Public,
        meta: MetaLevels::default,
    });

    static SITE: Site = declare_site(
        &PING,
        SiteDeclaration {
            layers: SiteLayers::default,
            global: OPERATION_LAYERS,
            guard_stage: None,
            class_gate: None,
        },
    );

    /// What an operation's site adds to the future of a tail holding `N`
    /// bytes: every stage between the wrapper and the developer's method.
    fn added_by_the_site<const N: usize>() -> usize {
        let holding = || async {
            let held = [0u8; N];
            tokio::task::yield_now().await;
            Ok::<_, OperationFailure>(held.len())
        };
        let tail = size_of_val(&holding());
        let view = operation_view(McpOperationKind::Tool, "ping", "Tools");
        size_of_val(&run_operation(&SITE, view, holding)) - tail
    }

    /// A debug build reserves every copy of a future a frame holds on the
    /// thread polling it: a site copying its tail's would multiply the stack
    /// an operation needs by the stages it runs.
    #[test]
    fn an_operations_site_holds_its_tails_future_once() {
        let light = added_by_the_site::<1024>();
        let heavy = added_by_the_site::<16384>();
        assert_eq!(
            light, heavy,
            "an operation's site copies its tail's future: it adds {light} bytes to a 1 KiB \
             one, {heavy} to a 16 KiB one",
        );
    }

    #[tokio::test]
    async fn an_operation_answers_the_value_its_tail_wrote() {
        let view = operation_view(McpOperationKind::Tool, "ping", "Tools");
        let answered =
            run_operation(&SITE, view, || async { Ok::<_, OperationFailure>("pong") }).await;
        assert_eq!(answered.ok(), Some("pong"));
    }

    #[test]
    fn an_operation_kind_names_the_method_reaching_its_router() {
        assert_eq!(McpOperationKind::Tool.method(), "tools/call");
        assert_eq!(McpOperationKind::Prompt.method(), "prompts/get");
    }
}
