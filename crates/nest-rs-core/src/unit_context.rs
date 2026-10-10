//! [`UnitContext`], what a layer knows about the unit it runs around, with the
//! edge's [`UnitView`] of it and the [`Peer`] on its other end.

use std::any::Any;
use std::fmt;
use std::net::IpAddr;

use tokio::time::Instant;

use crate::handler::{Handler, Reflector};
use crate::operation_log::{Edge, Unit};

/// Everything a layer knows about the unit it runs around: which unit, on which
/// edge, dispatched to which [`Handler`], for which [`Peer`], by when, and the
/// edge's own view of it.
///
/// The edge builds it on its stack for each unit; it is never boxed nor
/// stored. It is `Send` and not `Sync`: what reads the view mutably takes it
/// by `&mut`.
pub struct UnitContext<'a> {
    handler: &'a Handler,
    view: &'a mut (dyn UnitView + 'static),
    peer: Option<Peer>,
    deadline: Option<Instant>,
}

impl<'a> UnitContext<'a> {
    /// The unit of work running.
    pub fn unit(&self) -> Unit {
        self.handler.unit()
    }

    /// The framework edge dispatching the unit; `None` off the framework
    /// edges.
    pub fn edge(&self) -> Option<Edge> {
        self.unit().edge()
    }

    /// The dispatch site the unit runs: its method and its host.
    pub fn handler(&self) -> &'a Handler {
        self.handler
    }

    /// Reads what the site declared.
    pub fn reflector(&self) -> Reflector<'a> {
        self.handler.reflector()
    }

    /// Who is on the other end, as far as the edge can tell.
    pub fn peer(&self) -> Option<Peer> {
        self.peer
    }

    /// When the unit must end by, when its edge bounds it.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// The edge's view of the unit, when it is a `V`; `None` for another
    /// edge's view.
    ///
    /// ```
    /// use nest_rs_core::{UnitContext, UnitView};
    ///
    /// struct SocketView {
    ///     event: &'static str,
    /// }
    /// impl UnitView for SocketView {}
    ///
    /// struct JobView;
    /// impl UnitView for JobView {}
    ///
    /// fn event(cx: &UnitContext<'_>) -> Option<&'static str> {
    ///     cx.switch_to::<SocketView>().map(|view| view.event)
    /// }
    /// # use std::any::TypeId;
    /// # use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, declare_unit, new_unit_context};
    /// # use nest_rs_core::{Handler, Posture, operation_log::Kind};
    /// # struct ChatGateway;
    /// # static SEND: Handler = declare_handler(HandlerDeclaration {
    /// #     unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
    /// #     host: TypeId::of::<ChatGateway>(),
    /// #     host_name: "ChatGateway",
    /// #     method: "send",
    /// #     address: "send",
    /// #     posture: Posture::Gated,
    /// #     meta: MetaLevels::default,
    /// # });
    /// # let mut view = SocketView { event: "send" };
    /// # let cx = new_unit_context(&SEND, &mut view, None, None);
    ///
    /// assert_eq!(event(&cx), Some("send"));
    /// assert!(cx.switch_to::<JobView>().is_none());
    /// ```
    pub fn switch_to<V: UnitView>(&self) -> Option<&V> {
        let view: &dyn Any = &*self.view;
        view.downcast_ref()
    }

    /// The edge's view of the unit, mutably, when it is a `V`.
    pub fn switch_to_mut<V: UnitView>(&mut self) -> Option<&mut V> {
        let view: &mut dyn Any = &mut *self.view;
        view.downcast_mut()
    }
}

impl fmt::Debug for UnitContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnitContext")
            .field("handler", self.handler)
            .field("peer", &self.peer)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

/// An edge's typed view of the unit it dispatches, reached through
/// [`UnitContext::switch_to`]; each edge crate implements it on its own view.
pub trait UnitView: Any + Send {}

/// Who is on the other end of a unit, as far as its edge can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Peer {
    /// The client's address, as the edge resolved it through its trusted
    /// proxies; personal data, so no line or span carries it.
    pub address: Option<IpAddr>,
    /// The connection the unit arrived on, on an edge that holds one (a
    /// socket's id).
    pub connection: Option<u64>,
}

impl Peer {
    /// A peer an edge resolved.
    pub const fn new(address: Option<IpAddr>, connection: Option<u64>) -> Self {
        Self {
            address,
            connection,
        }
    }
}

/// The context of one unit, built by the edge dispatching it.
pub fn new_unit_context<'a>(
    handler: &'a Handler,
    view: &'a mut (dyn UnitView + 'static),
    peer: Option<Peer>,
    deadline: Option<Instant>,
) -> UnitContext<'a> {
    UnitContext {
        handler,
        view,
        peer,
        deadline,
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::handler::{HandlerDeclaration, MetaLevels, Posture, declare_handler};
    use crate::operation_log::{Edge, Kind, Unit};

    const MESSAGE: Unit = crate::operation_log::__private::declare_unit(
        "nest-rs-ws",
        "nest_rs::ws",
        Kind::Server,
        "ws.message",
    );

    struct ChatGateway;

    static SEND: Handler = declare_handler(HandlerDeclaration {
        unit: MESSAGE,
        host: TypeId::of::<ChatGateway>(),
        host_name: "ChatGateway",
        method: "send",
        address: "send",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    struct SocketView {
        event: &'static str,
    }

    impl UnitView for SocketView {}

    struct JobView;

    impl UnitView for JobView {}

    #[test]
    fn a_context_answers_its_unit_edge_handler_peer_and_deadline() {
        let mut view = SocketView { event: "send" };
        let peer = Peer::new(Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), Some(7));
        let deadline = tokio::time::Instant::now();
        let cx = new_unit_context(&SEND, &mut view, Some(peer), Some(deadline));

        assert_eq!(cx.unit(), MESSAGE);
        assert_eq!(cx.edge(), Some(Edge::Ws));
        assert!(std::ptr::eq(cx.handler(), &SEND));
        assert_eq!(cx.reflector().posture(), Posture::Gated);
        assert_eq!(cx.peer(), Some(peer));
        assert_eq!(cx.deadline(), Some(deadline));
        assert_eq!(
            cx.peer().and_then(|peer| peer.address),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(cx.peer().and_then(|peer| peer.connection), Some(7));
    }

    #[test]
    fn switching_reaches_the_units_own_view_and_no_other() {
        let mut view = SocketView { event: "send" };
        let mut cx = new_unit_context(&SEND, &mut view, None, None);

        assert_eq!(
            cx.switch_to::<SocketView>().map(|view| view.event),
            Some("send")
        );
        assert!(cx.switch_to::<JobView>().is_none());

        if let Some(view) = cx.switch_to_mut::<SocketView>() {
            view.event = "edited";
        }
        assert!(cx.switch_to_mut::<JobView>().is_none());
        assert_eq!(
            cx.switch_to::<SocketView>().map(|view| view.event),
            Some("edited")
        );
        assert_eq!((cx.peer(), cx.deadline()), (None, None));
    }

    #[test]
    fn a_context_held_across_an_await_keeps_the_future_send() {
        fn send<T: Send>(_: &T) {}

        let mut view = SocketView { event: "send" };
        let mut cx = new_unit_context(&SEND, &mut view, None, None);
        send(&cx);

        let held = async {
            let cx: &mut UnitContext<'_> = &mut cx;
            tokio::task::yield_now().await;
            cx.switch_to_mut::<SocketView>().map(|view| view.event)
        };
        send(&held);
    }
}
