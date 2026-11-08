//! One message on a gateway's socket, run as a unit through the pipeline: the
//! site `#[messages]` declares for its event, composed by the boot's wiring and
//! once at mount, with its data pipes and its handler as the tail, its answer
//! rendered as a frame once it is final.

use std::any::TypeId;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use nest_rs_core::__private::{
    GlobalLayers, HandlerDeclaration, MetaLevels, ResolvedLayer, Site, SiteDeclaration, SiteLayers,
    Tail, UnitChains, caught_problem, compose_chain, declare_handler, declare_site, dedup_bucket,
    dispatch, log_effective_chain, new_unit_context, refuse_site, report_unresolved,
    resolve_global_layers, resolve_scoped,
};
use nest_rs_core::LayerSite;
use nest_rs_core::{
    Container, Failure, Handler, Outcome, Peer, Posture, Settled, UnitContext, operation_log,
};
use nest_rs_guards::Denial;
use nest_rs_pipes::__private::PipeSpec;
use nest_rs_pipes::{ArgumentKind, GlobalPipe};

use crate::envelope::{Rendered, WsError, WsReply, chain_rendered, problem_rendered};
use crate::gateway::Gateway;
use crate::guard::denial_to_ws_error;
use crate::server::WsClient;
use crate::view::WsView;

/// What a message's site is named in boot logs and refusals.
pub(crate) fn message_site(handler: &Handler) -> String {
    format!("{}::{}", handler.host_name(), handler.method())
}

/// The data pipes one message runs over its `data`, composed once at mount: the
/// app's pool, then the gateway's `#[use_pipes]`, then the message's, each once.
struct MessagePipes(Vec<ResolvedLayer<dyn GlobalPipe>>);

impl MessagePipes {
    /// The chain the message `handler` declares composes on `container`.
    fn compose(
        container: &Container,
        handler: &Handler,
        host: &[PipeSpec],
        method: &[PipeSpec],
    ) -> nest_rs_core::anyhow::Result<Self> {
        let site = message_site(handler);
        let global = resolve_global_layers::<dyn GlobalPipe>(container);
        let host = resolve_scoped(container, host, LayerSite::Host, &site)
            .inspect_err(report_unresolved)?;
        let method = resolve_scoped(container, method, LayerSite::Method, &site)
            .inspect_err(report_unresolved)?;
        let chain = compose_chain::<dyn GlobalPipe>(dedup_bucket(global), host, method, &[], &site);
        log_effective_chain(&site, "pipes", &chain);
        Ok(Self(chain))
    }
}

/// What the edge answers itself, a frame no handler reads — an event none
/// serves, a frame no envelope holds — one site, at no event of its own.
static UNSERVED: Handler = declare_handler(HandlerDeclaration {
    unit: crate::unit::MESSAGE,
    host: TypeId::of::<crate::WsModule>(),
    host_name: "WsModule",
    method: "",
    address: operation_log::OTHER,
    posture: Posture::Public,
    meta: MetaLevels::default,
});

/// That site, folding what HTTP's fallback does: the unit stage and the app's
/// exception filters around what the edge answers.
static UNSERVED_SITE: Site = declare_site(
    &UNSERVED,
    SiteDeclaration {
        layers: SiteLayers::default,
        global: GlobalLayers {
            unit_stage: true,
            exception_filters: true,
            handler_stage: false,
        },
        guard_stage: None,
        class_gate: None,
    },
);

/// One message a gateway declares, as its boot check composes it.
pub struct DeclaredMessage {
    /// The message's site.
    pub site: &'static Site,
    /// `#[use_pipes]` beside the message.
    pub pipes: Vec<PipeSpec>,
}

/// Each of `messages` composes on `container` — its site, and its data pipes
/// over the gateway's `host_pipes` — or the first that does not refuses,
/// naming its site; so does the site of what no handler reads.
pub(crate) fn compose_declared(
    container: &Container,
    host_pipes: &[PipeSpec],
    messages: &[DeclaredMessage],
) -> nest_rs_core::anyhow::Result<()> {
    UNSERVED_SITE.chains(container)?;
    for message in messages {
        Message::compose(container, message.site, host_pipes, &message.pipes)?;
    }
    Ok(())
}

/// One message a gateway serves, mounted.
pub(crate) struct Message {
    pub(crate) handler: &'static Handler,
    pub(crate) chains: Arc<UnitChains>,
    pipes: MessagePipes,
}

impl Message {
    /// The message `site` declares, its chains and its data pipes — the
    /// gateway's `host_pipes`, then its own `method_pipes` — composed on
    /// `container`.
    fn compose(
        container: &Container,
        site: &'static Site,
        host_pipes: &[PipeSpec],
        method_pipes: &[PipeSpec],
    ) -> nest_rs_core::anyhow::Result<Self> {
        let handler = site.handler();
        Ok(Self {
            handler,
            chains: site.chains(container)?,
            pipes: MessagePipes::compose(container, handler, host_pipes, method_pipes)?,
        })
    }
}

/// A gateway's messages by event, mounted once by `#[messages]`, beside the
/// site of what no handler reads.
pub struct MessageTable {
    /// `None` for an event whose site did not compose: every message of it is
    /// refused, opaquely.
    by_event: HashMap<&'static str, Option<Message>>,
    /// `None` when the site did not compose: every such frame is refused, opaquely.
    unserved: Option<Arc<UnitChains>>,
}

impl MessageTable {
    /// A table of no message, the site of what no handler reads composed on
    /// `container`: one that does not compose is refused through `container`,
    /// for the boot to fail on.
    pub fn new(container: &Container) -> Self {
        let unserved = match UNSERVED_SITE.chains(container) {
            Ok(chains) => Some(chains),
            Err(refused) => {
                refuse_site(container, refused);
                None
            }
        };
        Self {
            by_event: HashMap::new(),
            unserved,
        }
    }

    /// Mount `event`'s message: its `site`'s chains and its data pipes, the
    /// gateway's `host_pipes` and the message's `method_pipes`, composed on
    /// `container`. One that does not compose is refused through `container`,
    /// for the boot to fail on, and denies every message.
    pub fn mount(
        &mut self,
        container: &Container,
        event: &'static str,
        site: &'static Site,
        host_pipes: &[PipeSpec],
        method_pipes: &[PipeSpec],
    ) {
        let message = match Message::compose(container, site, host_pipes, method_pipes) {
            Ok(message) => Some(message),
            Err(refused) => {
                refuse_site(container, refused);
                None
            }
        };
        self.by_event.insert(event, message);
    }

    /// `event` as a handler declares it, and its message — `None` for a site
    /// that did not compose — or `None` for an event no handler serves, which
    /// only the client named.
    pub(crate) fn get(&self, event: &str) -> Option<(&'static str, Option<&Message>)> {
        self.by_event
            .get_key_value(event)
            .map(|(&event, message)| (event, message.as_ref()))
    }
}

/// What a message's unit answered, and whether it failed.
pub(crate) struct Answered {
    /// What the client is sent.
    pub(crate) reply: WsReply,
    /// Whether the unit failed: a failed unit never commits.
    pub(crate) failed: bool,
    /// Whether it failed by the server's own fault, whatever `reply` says of it.
    server_failed: bool,
}

impl Answered {
    /// The server failed the unit, answered with `reply`.
    pub(crate) fn server_failed(reply: WsReply) -> Self {
        Self {
            reply,
            failed: true,
            server_failed: true,
        }
    }

    /// The opaque failure, its cause said where it was met.
    pub(crate) fn opaque() -> Self {
        Self::server_failed(WsReply::Error(WsError::internal()))
    }

    /// Whether the unit failed by what its client's own frame earned.
    pub(crate) fn refused_its_client(&self) -> bool {
        self.failed && !self.server_failed
    }
}

/// Run `data`, arrived on `client` for `event` at the gateway mounted at
/// `path`, through `message`'s chains, then render what the unit came to,
/// filing what its answer withholds.
pub(crate) async fn answer<G: Gateway>(
    gateway: &G,
    message: &Message,
    client: &WsClient,
    event: &'static str,
    path: &Arc<str>,
    data: serde_json::Value,
    deadline: Option<tokio::time::Instant>,
) -> Answered {
    let mut view = WsView::new(client.clone(), event, Some(path), data);
    let outcome = {
        let peer = Peer::new(None, Some(client.id()));
        let mut cx = new_unit_context(message.handler, &mut view, Some(peer), deadline);
        let mut tail = MessageTail {
            gateway,
            client,
            event,
            pipes: &message.pipes,
        };
        dispatch(&mut cx, &message.chains, &mut tail).await
    };
    rendered(view, outcome, event)
}

/// Run what the edge answers itself, `refusal`, to a frame no handler reads,
/// through the one site of it — the unit stage and the app's exception filters,
/// no guard and no handler stage — under `_OTHER`, then render what the unit
/// came to as a message's is.
pub(crate) async fn unserved(
    messages: &MessageTable,
    client: &WsClient,
    data: serde_json::Value,
    path: &Arc<str>,
    refusal: &WsError,
    deadline: Option<tokio::time::Instant>,
) -> Answered {
    let Some(chains) = &messages.unserved else {
        return refused();
    };
    let mut view = WsView::new(client.clone(), operation_log::OTHER, Some(path), data);
    let outcome = {
        let peer = Peer::new(None, Some(client.id()));
        let mut cx = new_unit_context(&UNSERVED, &mut view, Some(peer), deadline);
        dispatch(&mut cx, chains, &mut UnservedTail(refusal)).await
    };
    rendered(view, outcome, operation_log::OTHER)
}

/// The answer `outcome` comes to: the reply the view holds, or, for a failure
/// nobody answered, the edge's one rule. Who failed is read off what the edge
/// rendered for the failure that reached the end, else off what it would have —
/// a failure a layer raised in place of the one rendered says it for itself —
/// never off a reply a layer wrote, which answers for nobody. What the answer
/// withholds is filed here, once, as it goes out.
fn rendered(view: WsView, outcome: Outcome, event: &str) -> Answered {
    let failed = outcome.is_err();
    let rendered_class = outcome
        .as_ref()
        .err()
        .and_then(|failure| view.server_failed(failure));
    let (answer, withheld, path) = view.into_answer();
    let (reply, withheld, server_failed) = match (outcome, answer) {
        (Ok(_), Some(reply)) => (reply, withheld, false),
        (Ok(_), None) => {
            tracing::error!(
                target: crate::TARGET,
                path = path.as_deref(),
                event,
                "a message settled without a reply; answered opaquely",
            );
            return Answered::opaque();
        }
        (Err(failure), Some(reply)) if failure.is_answered() => {
            let server_failed =
                rendered_class.unwrap_or_else(|| failure_rendered(&failure).frame.server_failed);
            (reply, withheld, server_failed)
        }
        (Err(failure), _) => {
            let Rendered { frame, withheld } = failure_rendered(&failure);
            let server_failed = frame.server_failed;
            (WsReply::Error(frame), withheld, server_failed)
        }
    };
    if let Some(withheld) = withheld {
        crate::envelope::file(withheld, path.as_deref(), event);
    }
    Answered {
        reply,
        failed,
        server_failed,
    }
}

/// What a failure the view holds no answer to renders: an exception filter's
/// problem, a refusal, else the vocabulary its chain carries.
fn failure_rendered(failure: &Failure) -> Rendered {
    if let Some(problem) = caught_problem(failure) {
        return problem_rendered(failure.error(), problem);
    }
    if let Some(denial) = failure.find::<Denial>() {
        return Rendered {
            frame: denial_to_ws_error(denial.clone()),
            withheld: None,
        };
    }
    chain_rendered(failure.error())
}

/// What a message runs last: its data pipes over `data`, then its handler's
/// own call, written into the message's view. An error frame fails the unit,
/// answered: the exception filters still see it.
struct MessageTail<'a, G> {
    gateway: &'a G,
    client: &'a WsClient,
    event: &'static str,
    pipes: &'a MessagePipes,
}

impl<G: Gateway> Tail for MessageTail<'_, G> {
    fn call(&mut self, cx: &mut UnitContext<'_>) -> impl Future<Output = Outcome> + Send {
        let (gateway, client, event, pipes) = (self.gateway, self.client, self.event, self.pipes);
        async move {
            let mut data = WsView::of(cx)?.take_data();
            let folded = pipes
                .0
                .iter()
                .try_for_each(|entry| entry.layer.transform(cx, ArgumentKind::Data, &mut data));
            if let Err(refused) = folded {
                let failure = Failure::from(refused.clone());
                let reply = WsReply::pipe_error(event, "data", refused);
                return Err(failure.answered(WsView::of(cx)?.respond(reply)));
            }
            let handled = gateway.__nestrs_handle(client, event, data).await;
            let view = WsView::of(cx)?;
            match handled {
                Ok(WsReply::Error(frame)) => {
                    let failure = Failure::from(frame.clone());
                    Err(failure.answered(view.respond(WsReply::Error(frame))))
                }
                Ok(reply) => Ok(view.respond(reply)),
                Err(failed) => {
                    let (rendered, failure) = failed.into_parts();
                    let settled = view.answer_failed(rendered, &failure);
                    Err(failure.answered(settled))
                }
            }
        }
    }

    fn answer_failure(&mut self, cx: &mut UnitContext<'_>, failure: &Failure) -> Option<Settled> {
        let rendered = failure_rendered(failure);
        WsView::of(cx)
            .ok()
            .map(|view| view.answer_failed(rendered, failure))
    }
}

/// What a frame no handler reads runs last: the edge's own refusal, written
/// into its view. It fails the unit, answered: the exception filters still see
/// it, as they see a route's `404`.
struct UnservedTail<'a>(&'a WsError);

impl Tail for UnservedTail<'_> {
    fn call(&mut self, cx: &mut UnitContext<'_>) -> impl Future<Output = Outcome> + Send {
        let refusal = self.0;
        async move {
            let failure = Failure::from(refusal.clone());
            Err(failure.answered(WsView::of(cx)?.respond(WsReply::Error(refusal.clone()))))
        }
    }

    fn answer_failure(&mut self, cx: &mut UnitContext<'_>, failure: &Failure) -> Option<Settled> {
        let rendered = failure_rendered(failure);
        WsView::of(cx)
            .ok()
            .map(|view| view.answer_failed(rendered, failure))
    }
}

/// What a message whose site did not compose answers: the opaque refusal, its
/// cause the boot's refusal.
pub(crate) fn refused() -> Answered {
    Answered::server_failed(WsReply::Error(denial_to_ws_error(Denial::internal(
        "the message's site did not compose",
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit that could not begin and a site that did not compose answer the
    /// one opaque frame, failing the unit, the server's own failure.
    #[test]
    fn what_the_message_cannot_run_answers_opaquely() {
        for answered in [Answered::opaque(), refused()] {
            assert!(answered.failed, "a failed unit never commits");
            assert!(!answered.refused_its_client(), "no client earned it");
            let WsReply::Error(frame) = answered.reply else {
                panic!("an error frame");
            };
            crate::testing::assert_opaque(&frame);
        }
    }

    /// A unit that settled with no reply in its view answers opaquely, its line
    /// naming the message by the coordinates its `mounted` line gives it.
    #[test]
    fn a_message_settled_without_a_reply_names_it_at_its_gateway() {
        let logs = nest_rs_testing::LogCapture::install();
        let path: Arc<str> = Arc::from("/chat");
        let view = WsView::new(
            WsClient::for_test(),
            "send",
            Some(&path),
            serde_json::Value::Null,
        );

        let answered = rendered(view, Ok(nest_rs_core::__private::settled()), "send");

        assert!(answered.failed && !answered.refused_its_client());
        let line = logs.expect_one(
            crate::TARGET,
            "a message settled without a reply; answered opaquely",
        );
        for (coordinate, value) in [("path", "/chat"), ("event", "send")] {
            assert_eq!(line.field(coordinate).as_deref(), Some(value), "{line:#?}");
        }
    }
}
