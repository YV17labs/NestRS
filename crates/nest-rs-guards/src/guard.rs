//! The [`Guard`] trait — one contract every edge checks a unit through — and
//! [`GuardFor`], the declaration a decorator binds a guard on.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use nest_rs_core::{Layer, UnitContext, UnitView};

use crate::denial::Denial;

/// Where in a guard chain this guard belongs — **declared**, never inferred
/// from type names. Boot-time chain validation reads it to refuse a chain
/// whose authorization guard is listed before the authentication guard it
/// depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardPhase {
    /// Establishes *who* the caller is (attaches a principal).
    Authentication,
    /// Decides *what* the caller may do (consumes a principal).
    Authorization,
    /// Neither — throttling, feature gates, custom checks.
    Other,
}

/// A principal type a guard attaches to a unit — or expects an earlier guard to
/// have attached — described by `TypeId` plus a human-readable name for boot
/// errors.
#[derive(Clone, Copy, Debug)]
pub struct PrincipalClaim {
    /// The principal's `TypeId`, the key it is attached under.
    pub type_id: TypeId,
    /// The principal's type name, for boot diagnostics.
    pub type_name: &'static str,
}

impl PrincipalClaim {
    /// Describe the principal type `T`.
    pub fn of<T: 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
        }
    }
}

/// The future a guard's check returns, as `#[async_trait]` writes an override.
type Check<'fut> = Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>;

/// What an entry a guard does not override answers: admitted, with nothing
/// allocated — a box of a zero-sized future holds no memory.
struct Admit;

impl Future for Admit {
    type Output = Result<(), Denial>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(Ok(()))
    }
}

/// A guard: decides whether a unit of work may run, before anything an app or
/// a package adds around it.
///
/// Two entries, each admitting by default: [`check_http`](Self::check_http)
/// for an HTTP request — a route, a WebSocket upgrade, the request of an
/// in-band endpoint (`/graphql`, `/mcp`) — and [`check`](Self::check) for a
/// unit an in-band edge carries: a GraphQL field, a WebSocket message, an MCP
/// operation, or a later edge's unit. Override the one(s) where the guard has
/// work, then declare each edge it checks with [`GuardFor`]: a decorator binds
/// a guard only on the edges it declares, so a guard bound where its check is
/// the default is a compile error rather than an entry that admits everything.
///
/// The posture a site declares — `#[public]`, `#[authorize]`, an in-band
/// endpoint — is read through [`UnitContext::reflector`], and what a guard
/// makes known for the rest of the unit is attached with
/// [`UnitContext::attach`]: ambient once the whole stage passed, never on a
/// refused unit.
///
/// The entries are written out as `#[async_trait]` writes an override, so an
/// implementation is an ordinary `async fn` under `#[async_trait]`.
pub trait Guard: Layer {
    /// An HTTP request. Read it — head and body — through the HTTP edge's view
    /// of `cx` (`HttpView::of(cx)`); a guard that reads the body sets it back.
    /// The default admits.
    fn check_http<'s, 'c, 'u, 'fut>(&'s self, cx: &'c mut UnitContext<'u>) -> Check<'fut>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut,
    {
        let _ = cx;
        Box::pin(Admit)
    }

    /// A unit an in-band edge carries; [`UnitContext::edge`] and
    /// [`UnitContext::switch_to`] say which. The default admits.
    fn check<'s, 'c, 'u, 'fut>(&'s self, cx: &'c mut UnitContext<'u>) -> Check<'fut>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut,
    {
        let _ = cx;
        Box::pin(Admit)
    }

    /// The chain phase this guard declares. Boot-time validation refuses a
    /// chain listing an [`Authentication`](GuardPhase::Authentication) guard
    /// after an [`Authorization`](GuardPhase::Authorization) one.
    fn phase(&self) -> GuardPhase {
        GuardPhase::Other
    }

    /// The principal type this guard attaches on success (an authentication
    /// guard's claims), if any. Read by boot-time chain validation.
    fn produced_principal(&self) -> Option<PrincipalClaim> {
        None
    }

    /// The principal type this guard expects an earlier guard to have
    /// attached (an authorization guard's actor), if any. Boot-time chain
    /// validation fails the boot when an earlier guard attaches a **different**
    /// type — the mismatch that would otherwise answer `500` on every request.
    fn expected_principal(&self) -> Option<PrincipalClaim> {
        None
    }
}

/// Declares that a guard checks the units whose view is `V` — the bound every
/// decorator emits for the guards bound at its site.
///
/// `V` is an edge's view: the HTTP edge's for a `#[controller]`, a `#[routes]`
/// verb and a `#[gateway]` (whose guards run on the upgrade, an HTTP `GET`),
/// GraphQL's for a `#[resolver]`, WebSocket's for a `#[subscribe_message]`,
/// MCP's for an `#[mcp]` host or a `#[tool]`. A crate whose guard serves
/// several edges declares each one behind that edge's feature, one line each.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not declare that it checks `{V}` units",
    label = "bound at a site whose units are `{V}`",
    note = "`check_http` and `check` admit by default, so a guard bound here without its check \
            would pass every unit silently",
    note = "override `Guard::check_http` (an HTTP request) or `Guard::check` (every other edge's \
            unit), then declare `impl GuardFor<{V}> for {Self} {{}}`"
)]
pub trait GuardFor<V: UnitView>: Guard {}

// Written out, not `#[async_trait]`: the macro would box each already-boxed
// inner future a second time.
impl<T: Guard + ?Sized> Guard for Arc<T> {
    fn check_http<'s, 'c, 'u, 'fut>(&'s self, cx: &'c mut UnitContext<'u>) -> Check<'fut>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut,
    {
        (**self).check_http(cx)
    }

    fn check<'s, 'c, 'u, 'fut>(&'s self, cx: &'c mut UnitContext<'u>) -> Check<'fut>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut,
    {
        (**self).check(cx)
    }

    fn phase(&self) -> GuardPhase {
        (**self).phase()
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        (**self).produced_principal()
    }

    fn expected_principal(&self) -> Option<PrincipalClaim> {
        (**self).expected_principal()
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use async_trait::async_trait;
    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, declare_unit};
    use nest_rs_core::operation_log::Kind;
    use nest_rs_core::{Handler, Posture};

    use super::*;
    use crate::testing::run_counting;

    struct Socket;

    impl UnitView for Socket {}

    struct Host;

    static MESSAGE: Handler = declare_handler(HandlerDeclaration {
        unit: declare_unit("nest-rs-ws", "nest_rs::ws", Kind::Server, "ws.message"),
        host: TypeId::of::<Host>(),
        host_name: "Host",
        method: "send",
        address: "send",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// Overrides neither entry.
    struct Neither;

    impl Layer for Neither {}
    impl Guard for Neither {}

    /// Overrides both, as an app writes them.
    struct Both;

    impl Layer for Both {}

    #[async_trait]
    impl Guard for Both {
        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            let _ = cx.reflector().posture();
            Err(Denial::unauthorized("refused at the request"))
        }

        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            match cx.switch_to::<Socket>() {
                Some(_) => Err(Denial::forbidden("refused on the socket")),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn a_guard_overriding_neither_entry_admits_without_allocating() {
        let mut view = Socket;
        let mut cx = nest_rs_core::__private::new_unit_context(&MESSAGE, &mut view, None, None);
        let guard: Arc<dyn Guard> = Arc::new(Neither);

        let (http, made) = run_counting(|| guard.as_ref().check_http(&mut cx));
        assert!(http.is_ok());
        assert_eq!(made, 0, "the default HTTP entry allocates nothing");

        let (unit, made) = run_counting(|| guard.as_ref().check(&mut cx));
        assert!(unit.is_ok());
        assert_eq!(made, 0, "the default unit entry allocates nothing");
    }

    #[test]
    fn an_async_trait_override_of_each_entry_is_the_one_that_runs() {
        let mut view = Socket;
        let mut cx = nest_rs_core::__private::new_unit_context(&MESSAGE, &mut view, None, None);
        let guard: Arc<dyn Guard> = Arc::new(Both);

        let (http, _) = run_counting(|| guard.check_http(&mut cx));
        assert_eq!(http.map_err(|d| d.http_status()), Err(401));
        let (unit, _) = run_counting(|| guard.check(&mut cx));
        assert_eq!(unit.map_err(|d| d.http_status()), Err(403));
    }
}
