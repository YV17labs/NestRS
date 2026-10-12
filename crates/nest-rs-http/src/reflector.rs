//! [`Reflector`] — read what a request's route declared: its `#[meta(...)]`
//! values and its posture.
//!
//! Every guard reads it, global or per route. A route's shaper carries the
//! route's declaration on the request; a self-mounted endpoint (`/graphql`,
//! `/mcp`, a gateway) has none, so it finds no metadata and reads the posture
//! its mount stamped.

use std::any::Any;

use nest_rs_core::{Handler, Posture};
use poem::Request;

use crate::metadata::{HandlerMetadata, HandlerRef, Public};

/// Reads what the request's route declared, by type.
pub struct Reflector<'a> {
    req: &'a Request,
    handler: Option<&'static Handler>,
}

impl<'a> Reflector<'a> {
    /// Read the route the request is dispatched to.
    pub fn new(req: &'a Request) -> Self {
        Reflector {
            req,
            handler: req
                .extensions()
                .get::<HandlerRef>()
                .map(|declared| declared.0),
        }
    }
}

impl HandlerMetadata for Reflector<'_> {
    /// The value of type `M` the route's handler declares, else its host's;
    /// what a layer inserts into the request is never read here.
    fn get<M: Any + Send + Sync>(&self) -> Option<&M> {
        self.handler?.reflector().get::<M>()
    }

    fn is_public(&self) -> bool {
        self.posture() == Posture::Public
    }

    /// The route's declared posture; on a request no route declared, the
    /// posture its mount stamped, else `Public` under the [`Public`] marker,
    /// else `Gated`.
    fn posture(&self) -> Posture {
        if let Some(handler) = self.handler {
            return handler.posture();
        }
        let extensions = self.req.extensions();
        match extensions.get::<Posture>() {
            Some(stamped) => *stamped,
            None if extensions.get::<Public>().is_some() => Posture::Public,
            None => Posture::Gated,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, meta_value};

    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Throttle(u32);

    struct PostsController;

    fn levels() -> MetaLevels {
        MetaLevels {
            host: vec![meta_value(Throttle(100))],
            method: vec![meta_value(Throttle(10))],
        }
    }

    static LIST: Handler = declare_handler(HandlerDeclaration {
        unit: crate::unit::REQUEST,
        host: TypeId::of::<PostsController>(),
        host_name: "PostsController",
        method: "list",
        address: "/posts",
        posture: Posture::Public,
        meta: levels,
    });

    #[test]
    fn a_routed_request_reads_what_its_handler_declared() {
        let mut req = Request::default();
        req.extensions_mut().insert(HandlerRef(&LIST));
        let reflector = Reflector::new(&req);
        assert_eq!(reflector.get::<Throttle>(), Some(&Throttle(10)));
        assert_eq!(reflector.posture(), Posture::Public);
        assert!(reflector.is_public());
    }

    #[test]
    fn a_value_a_layer_inserts_never_shadows_the_declared_one() {
        let mut req = Request::default();
        req.extensions_mut().insert(HandlerRef(&LIST));
        req.extensions_mut().insert(Throttle(1));
        req.extensions_mut().insert(Posture::Gated);
        let reflector = Reflector::new(&req);
        assert_eq!(reflector.get::<Throttle>(), Some(&Throttle(10)));
        assert_eq!(reflector.posture(), Posture::Public);
    }

    #[test]
    fn a_request_no_route_declared_reads_no_metadata() {
        let mut req = Request::default();
        req.extensions_mut().insert(Throttle(1));
        assert_eq!(Reflector::new(&req).get::<Throttle>(), None);
    }
}
