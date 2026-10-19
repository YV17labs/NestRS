//! One message on a gateway's socket, run as a unit through the pipeline: the
//! site `#[messages]` declares for its event, composed by the boot's wiring and
//! once at mount, with its data pipes and its handler as the tail, its answer
//! rendered as a frame once it is final.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use nest_rs_core::__private::{
    ResolvedLayer, Site, Tail, UnitChains, caught_problem, compose_chain, dedup_bucket, dispatch,
    log_effective_chain, new_unit_context, refuse_site, report_unresolved, resolve_global_layers,
    resolve_scoped,
};
use nest_rs_core::layer_chain::LayerSite;
use nest_rs_core::{Container, Failure, Handler, Outcome, Peer, Settled, UnitContext};
use nest_rs_guards::Denial;
use nest_rs_pipes::{ArgumentKind, GlobalPipe, PipeSpec, PipeSpecs};

use crate::envelope::{Rendered, WsReply, caught_rendered, chain_rendered};
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
        let global = resolve_global_layers::<PipeSpecs>(container);
        let host = resolve_scoped(container, host, LayerSite::Host, &site)
            .inspect_err(report_unresolved)?;
        let method = resolve_scoped(container, method, LayerSite::Method, &site)
            .inspect_err(report_unresolved)?;
        let chain = compose_chain::<dyn GlobalPipe>(dedup_bucket(global), host, method, &[], &site);
        log_effective_chain(&site, "pipes", &chain);
        Ok(Self(chain))
    }
}

/// One message a gateway declares, as its boot check composes it.
pub struct DeclaredMessage {
    /// The message's site.
    pub site: &'static Site,
    /// `#[use_pipes]` beside the message.
    pub pipes: Vec<PipeSpec>,
}

/// Each of `messages` composes on `container` — its site, and its data pipes
/// over the gateway's `host_pipes` — or the first that does not refuses,
/// naming its site.
pub(crate) fn compose_declared(
    container: &Container,
    host_pipes: &[PipeSpec],
    messages: &[DeclaredMessage],
) -> nest_rs_core::anyhow::Result<()> {
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

/// A gateway's messages by event, mounted once by `#[messages]`.
#[derive(Default)]
pub struct MessageTable {
    /// `None` for an event whose site did not compose: every message of it is
    /// refused, opaquely.
    by_event: HashMap<&'static str, Option<Message>>,
}

impl MessageTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
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
}

impl Answered {
    /// The unit failed, answered with `reply`.
    pub(crate) fn failed(reply: WsReply) -> Self {
        Self {
            reply,
            failed: true,
        }
    }

    /// The opaque failure, its cause said where it was met.
    pub(crate) fn opaque() -> Self {
        Self::failed(WsReply::error(nest_rs_core::OPAQUE_CLIENT_MESSAGE))
    }
}

/// Run `data`, arrived on `client` for `event`, through `message`'s chains,
/// then render what the unit came to, filing the line its answer owes.
pub(crate) async fn answer<G: Gateway>(
    gateway: &G,
    message: &Message,
    client: &WsClient,
    event: &'static str,
    data: serde_json::Value,
    deadline: Option<tokio::time::Instant>,
) -> Answered {
    let mut view = WsView::new(client.clone(), event, data);
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

/// The answer `outcome` comes to: the reply the view holds, or, for a failure
/// nobody answered, the edge's one rule. The line the answer owes is filed
/// here, once, as it goes out.
fn rendered(view: WsView, outcome: Outcome, event: &str) -> Answered {
    let failed = outcome.is_err();
    let (reply, line) = match (outcome, view.into_answer()) {
        (Ok(_), (Some(reply), line)) => (reply, line),
        (Ok(_), (None, _)) => {
            tracing::error!(
                target: crate::TARGET,
                event,
                "a message settled without a reply; answered opaquely",
            );
            return Answered::opaque();
        }
        (Err(failure), (Some(reply), line)) if failure.is_answered() => (reply, line),
        (Err(failure), _) => {
            let Rendered { frame, line } = failure_rendered(&failure);
            (WsReply::Error(frame), line)
        }
    };
    if let Some(line) = line {
        line.file(event);
    }
    Answered { reply, failed }
}

/// What a failure the view holds no answer to renders: an exception filter's
/// problem, a refusal, else the vocabulary its chain carries.
fn failure_rendered(failure: &Failure) -> Rendered {
    if let Some(problem) = caught_problem(failure) {
        return caught_rendered(failure.error(), problem);
    }
    if let Some(denial) = failure.find::<Denial>() {
        return Rendered {
            frame: denial_to_ws_error(denial.clone()),
            line: None,
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
                    Err(failure.answered(view.answer_failed(rendered)))
                }
            }
        }
    }

    fn answer_failure(&mut self, cx: &mut UnitContext<'_>, failure: &Failure) -> Option<Settled> {
        let rendered = failure_rendered(failure);
        WsView::of(cx).ok().map(|view| view.answer_failed(rendered))
    }
}

/// What a message whose site did not compose answers: the opaque refusal, its
/// cause the boot's refusal.
pub(crate) fn refused() -> Answered {
    Answered::failed(WsReply::Error(denial_to_ws_error(Denial::internal(
        "the message's site did not compose",
    ))))
}
