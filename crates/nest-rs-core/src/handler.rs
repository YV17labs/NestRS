//! [`Handler`], one dispatch site as its decorators declared it, the
//! [`Posture`] it declares, and the [`Reflector`] a layer reads it through.

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::OnceLock;

use crate::operation_log::Unit;

/// One dispatch site as its decorators declared it: built once, immutable, and
/// shared by every unit the site serves, so no layer can overwrite what was
/// declared.
///
/// A layer reaches it through
/// [`UnitContext::handler`](crate::UnitContext::handler) — the method and its
/// host in one value — and reads its metadata through
/// [`reflector`](Self::reflector).
pub struct Handler {
    unit: Unit,
    host: TypeId,
    host_name: &'static str,
    method: &'static str,
    address: &'static str,
    posture: Posture,
    meta: fn() -> MetaLevels,
    levels: OnceLock<MetaLevels>,
}

impl Handler {
    /// The unit of work this site runs as.
    pub fn unit(&self) -> Unit {
        self.unit
    }

    /// The host provider's type: a controller, resolver, gateway, MCP host,
    /// processor, scheduled or listener host, or the mounting module's type
    /// for a self-mount.
    pub fn host(&self) -> TypeId {
        self.host
    }

    /// The host's type name, as its decorator wrote it.
    pub fn host_name(&self) -> &'static str {
        self.host_name
    }

    /// The Rust method dispatched to; empty for a framework endpoint (a
    /// self-mount, the fallback).
    pub fn method(&self) -> &'static str {
        self.method
    }

    /// What the decorator declared a client addresses: the route template as
    /// written (without the app's prefix or version), the GraphQL field, the
    /// WS event, the MCP tool or prompt, the queue, the job, the event type,
    /// the mount path.
    pub fn address(&self) -> &'static str {
        self.address
    }

    /// Who the site's declaration says may call it.
    pub fn posture(&self) -> Posture {
        self.posture
    }

    /// Reads what was declared on this site.
    pub fn reflector(&self) -> Reflector<'_> {
        Reflector { handler: self }
    }

    /// The site's metadata, built by its declaration on the first read and
    /// kept for the process.
    fn levels(&self) -> &MetaLevels {
        self.levels.get_or_init(self.meta)
    }
}

impl fmt::Debug for Handler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handler")
            .field("unit", &self.unit.name())
            .field("host", &self.host_name)
            .field("method", &self.method)
            .field("address", &self.address)
            .field("posture", &self.posture)
            .finish_non_exhaustive()
    }
}

/// Who a site's declaration says may call it: what `#[public]` and
/// `#[authorize]` compile to, read by a guard instead of a marker it could be
/// handed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Posture {
    /// `#[public]`: an authentication guard admits an anonymous caller.
    Public,
    /// `#[authorize(..)]`, or a site gated by its guards alone.
    Gated,
    /// An endpoint whose operations each declare their own posture
    /// (`/graphql`): an authentication guard admits an anonymous request,
    /// still verifying a presented credential.
    InBand,
    /// No caller: a queue attempt, a scheduled tick, an event listener.
    System,
}

/// Reads what was declared on a [`Handler`]: its `#[meta]` values by type and
/// its [`Posture`].
///
/// A value declared on the method overrides one of the same type declared on
/// its host.
#[derive(Clone, Copy, Debug)]
pub struct Reflector<'a> {
    handler: &'a Handler,
}

impl<'a> Reflector<'a> {
    /// The `M` declared on the method, else on its host.
    pub fn get<M: Any + Send + Sync>(&self) -> Option<&'a M> {
        self.get_all().next()
    }

    /// Every `M` declared, the method's first, then the host's.
    pub fn get_all<M: Any + Send + Sync>(&self) -> impl Iterator<Item = &'a M> + use<'a, M> {
        let levels = self.handler.levels();
        levels
            .method
            .iter()
            .chain(&levels.host)
            .filter_map(|value| value.downcast_ref::<M>())
    }

    /// The site's [`Posture`].
    pub fn posture(&self) -> Posture {
        self.handler.posture
    }

    /// Whether the site is `#[public]`.
    pub fn is_public(&self) -> bool {
        self.posture() == Posture::Public
    }
}

/// A site's `#[meta]` values, the host's and the method's, as its declaration
/// builds them.
#[derive(Default)]
pub struct MetaLevels {
    /// Declared on the host: the struct, or the impl block a worker pair
    /// decorates.
    pub host: Vec<Box<dyn Any + Send + Sync>>,
    /// Declared on the dispatched method.
    pub method: Vec<Box<dyn Any + Send + Sync>>,
}

/// What a decorator declares about one dispatch site, written into a `static`.
pub struct HandlerDeclaration {
    /// The unit the site runs as.
    pub unit: Unit,
    /// The host provider's type.
    pub host: TypeId,
    /// The host's type name, a literal: `type_name` is not `const`.
    pub host_name: &'static str,
    /// The dispatched method's name.
    pub method: &'static str,
    /// What a client addresses.
    pub address: &'static str,
    /// What `#[public]` and `#[authorize]` declared.
    pub posture: Posture,
    /// Builds the site's metadata, once, on the first read.
    pub meta: fn() -> MetaLevels,
}

/// The [`Handler`] a declaration describes, in a `static`.
pub const fn declare_handler(declaration: HandlerDeclaration) -> Handler {
    let HandlerDeclaration {
        unit,
        host,
        host_name,
        method,
        address,
        posture,
        meta,
    } = declaration;
    Handler {
        unit,
        host,
        host_name,
        method,
        address,
        posture,
        meta,
        levels: OnceLock::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::operation_log::{Edge, Kind, Unit};

    const REQUEST: Unit = crate::operation_log::__private::declare_unit(
        "nest-rs-http",
        "nest_rs::http",
        Kind::Server,
        "http.request",
    );

    #[derive(Debug, PartialEq)]
    struct Throttle(u32);

    #[derive(Debug, PartialEq)]
    struct Audit(&'static str);

    struct PostsController;

    fn posts_levels() -> MetaLevels {
        MetaLevels {
            host: vec![Box::new(Throttle(100)), Box::new(Audit("posts"))],
            method: vec![Box::new(Throttle(10))],
        }
    }

    static CREATE: Handler = declare_handler(HandlerDeclaration {
        unit: REQUEST,
        host: TypeId::of::<PostsController>(),
        host_name: "PostsController",
        method: "create",
        address: "/posts",
        posture: Posture::Gated,
        meta: posts_levels,
    });

    #[test]
    fn a_handler_answers_what_its_site_declared() {
        assert_eq!(CREATE.unit(), REQUEST);
        assert_eq!(CREATE.unit().edge(), Some(Edge::Http));
        assert_eq!(CREATE.host(), TypeId::of::<PostsController>());
        assert_eq!(CREATE.host_name(), "PostsController");
        assert_eq!(CREATE.method(), "create");
        assert_eq!(CREATE.address(), "/posts");
        assert_eq!(CREATE.posture(), Posture::Gated);
    }

    #[test]
    fn the_reflector_reads_the_methods_value_over_the_hosts() {
        let reflector = CREATE.reflector();
        assert_eq!(reflector.get::<Throttle>(), Some(&Throttle(10)));
        assert_eq!(reflector.get::<Audit>(), Some(&Audit("posts")));
        assert_eq!(reflector.get::<u8>(), None);
    }

    #[test]
    fn the_reflector_yields_every_value_the_methods_first() {
        let all: Vec<&Throttle> = CREATE.reflector().get_all::<Throttle>().collect();
        assert_eq!(all, [&Throttle(10), &Throttle(100)]);
        assert_eq!(CREATE.reflector().get_all::<u8>().count(), 0);
    }

    #[test]
    fn the_reflector_reads_the_posture_declared() {
        assert_eq!(CREATE.reflector().posture(), Posture::Gated);
        assert!(!CREATE.reflector().is_public());

        static HEALTH: Handler = declare_handler(HandlerDeclaration {
            unit: REQUEST,
            host: TypeId::of::<PostsController>(),
            host_name: "PostsController",
            method: "health",
            address: "/posts/health",
            posture: Posture::Public,
            meta: MetaLevels::default,
        });
        assert!(HEALTH.reflector().is_public());
        assert_eq!(HEALTH.reflector().get::<Throttle>(), None);
    }

    #[test]
    fn the_metadata_is_built_once_on_first_read() {
        static BUILT: AtomicUsize = AtomicUsize::new(0);
        fn counted() -> MetaLevels {
            BUILT.fetch_add(1, Ordering::SeqCst);
            MetaLevels {
                host: Vec::new(),
                method: vec![Box::new(Audit("counted"))],
            }
        }
        static COUNTED: Handler = declare_handler(HandlerDeclaration {
            unit: REQUEST,
            host: TypeId::of::<PostsController>(),
            host_name: "PostsController",
            method: "counted",
            address: "/counted",
            posture: Posture::Gated,
            meta: counted,
        });

        assert_eq!(BUILT.load(Ordering::SeqCst), 0, "declaring builds nothing");
        for _ in 0..3 {
            assert_eq!(COUNTED.reflector().get::<Audit>(), Some(&Audit("counted")));
        }
        assert_eq!(BUILT.load(Ordering::SeqCst), 1);
    }
}
