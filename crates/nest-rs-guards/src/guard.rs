//! The [`Guard`] trait — one contract every edge checks a unit through — and
//! [`GuardFor`], the declaration a decorator binds a guard on.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::{DenialDocument, Layer, UnitContext, UnitView};

use crate::denial::Denial;

/// Where in a guard chain this guard belongs — **declared**, never inferred
/// from type names. Boot-time chain validation reads it to refuse a chain
/// whose authorization guard is listed before the authentication guard it
/// depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub struct PrincipalClaim {
    /// The principal's `TypeId`, the key it is attached under.
    pub type_id: TypeId,
    /// The principal's type name, for boot diagnostics.
    pub type_name: &'static str,
    /// An expected principal whose absence the guard decides itself
    /// ([`or_visitor`](Self::or_visitor)).
    pub or_visitor: bool,
}

impl PrincipalClaim {
    /// Describe the principal type `T`.
    pub fn of<T: 'static>() -> Self {
        Self {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            or_visitor: false,
        }
    }

    /// An expected principal the guard does without: it reads it where it is,
    /// and decides a caller with none itself, as an authorization guard builds a
    /// visitor's ability. Nothing attaching it is no wiring defect, and is said
    /// at `warn` rather than refusing the boot.
    #[must_use]
    pub fn or_visitor(self) -> Self {
        Self {
            or_visitor: true,
            ..self
        }
    }
}

/// The future a guard's check returns, as `#[async_trait]` writes an override.
type Check<'fut> = Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>;

/// A guard: decides whether a unit of work may run, before anything an app or
/// a package adds around it.
///
/// One decision, written once: [`check`](Self::check) runs on every unit the
/// guard is bound at — a GraphQL field, a WebSocket message, an MCP operation,
/// a later edge's unit — and on an HTTP request too, since
/// [`check_http`](Self::check_http) runs it unless the guard overrides it. A
/// guard overrides `check_http` only where the request differs from the unit:
/// it reads the request itself, answers in its protocol's own words, or what is
/// missing at the request means something else than at the unit. `check` has
/// no default, so no guard admits a unit it never decided: one whose decision
/// is the request's alone, made in `check_http`, admits the units that request
/// carries in one visible `Ok(())`. Then declare each edge it checks with
/// [`GuardFor`]: a decorator binds a guard only on the edges it declares.
///
/// A refusal is a [`Denial`], filed by the edge that ran the guard — one line,
/// on the guard's [`target`](Self::target), carrying the denial's
/// [`reason`](Denial::reason) — so a guard writes no log line of its own for
/// it.
///
/// The posture a site declares — `#[public]`, `#[authorize]`, an in-band
/// endpoint — is read through [`UnitContext::reflector`], and what a guard
/// makes known for the rest of the unit is attached with
/// [`UnitContext::attach`]: ambient once the whole stage passed, never on a
/// refused unit. What an earlier guard must have attached is read with
/// [`UnitContext::require`], and a principal declared through
/// [`expected_principal`](Self::expected_principal) answers by cause: the boot
/// refuses a chain nothing in which attaches it, so its absence at a unit is a
/// caller the guard attaching it admitted without one, the caller's `401`. Any
/// other value's absence is a `500`.
///
/// The entries are written out as `#[async_trait]` writes an override, so an
/// implementation is an ordinary `async fn` under `#[async_trait]` — one
/// closing every unit while the deployment is in maintenance:
///
/// ```
/// use nest_rs_core::{Layer, UnitContext, async_trait};
/// use nest_rs_guards::{Denial, Guard};
///
/// struct Maintenance {
///     closed: bool,
/// }
/// # impl Layer for Maintenance {}
///
/// #[async_trait]
/// impl Guard for Maintenance {
///     async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
///         if self.closed {
///             return Err(Denial::forbidden("closed for maintenance"));
///         }
///         Ok(())
///     }
/// }
/// ```
pub trait Guard: Layer {
    /// An HTTP request — a route, a WebSocket upgrade, the request of an
    /// in-band endpoint (`/graphql`, `/mcp`). Read it — head and body — through
    /// the HTTP edge's view of `cx` (`HttpView::of(cx)`); a guard that reads the
    /// body sets it back. The default runs [`check`](Self::check), the guard's
    /// one decision.
    fn check_http<'s, 'c, 'u, 'fut>(&'s self, cx: &'c mut UnitContext<'u>) -> Check<'fut>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut,
    {
        self.check(cx)
    }

    /// The guard's decision on a unit; [`UnitContext::edge`] and
    /// [`UnitContext::switch_to`] say which. It has no default: a guard
    /// admitting a unit says so.
    fn check<'s, 'c, 'u, 'fut>(&'s self, cx: &'c mut UnitContext<'u>) -> Check<'fut>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut;

    /// The chain phase this guard declares. Boot-time validation refuses a
    /// chain listing an [`Authentication`](GuardPhase::Authentication) guard
    /// after an [`Authorization`](GuardPhase::Authorization) one, and only those
    /// two cover a unit: under [`Other`](GuardPhase::Other) guards alone, the
    /// default, a unit says `#[public]` or refuses the boot.
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
    /// type, or when nothing in the chain attaches it, unless the claim says the
    /// guard does without it ([`PrincipalClaim::or_visitor`]) — what would
    /// otherwise answer `500` on every request.
    fn expected_principal(&self) -> Option<PrincipalClaim> {
        None
    }

    /// Whether this guard attaches the caller's ability, which a class gate
    /// reads: the boot refuses a gate no guard answering `true` reaches, in its
    /// chain or at its request. `nest_rs::authz::AbilityGuard` does, and a guard
    /// delegating to it answers as it does; nothing else builds that ability.
    #[doc(hidden)]
    fn __attaches_ability(&self) -> bool {
        false
    }

    /// The statuses [`check_http`](Self::check_http) can refuse a request
    /// with on a route that is `#[public]` (`public`) or not, each one a
    /// [`Denial::http_status`] or the status of the error a refusing guard
    /// answers in its protocol's own words, which the HTTP edge serves in place
    /// of the denial's rendering: what a route's document lists for this guard.
    /// A `500` is not declared — [`Denial::internal`] is a wiring defect, and
    /// every route documents it.
    ///
    /// The default is `401` and `403` on a route that is not `#[public]`, and
    /// nothing on one that is. A guard refusing with anything else says so,
    /// read off its `Err` paths — one answering `403` to a caller outside the
    /// office network, on every route:
    ///
    /// ```
    /// # use nest_rs_core::{Layer, UnitContext, async_trait};
    /// # use nest_rs_guards::{Denial, Guard};
    /// struct OfficeOnly;
    /// # impl Layer for OfficeOnly {}
    ///
    /// #[async_trait]
    /// impl Guard for OfficeOnly {
    ///     # async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
    ///     #     Err(Denial::forbidden("outside the office network"))
    ///     # }
    ///     fn denial_statuses(&self, _public: bool) -> &'static [u16] {
    ///         &[403]
    ///     }
    /// }
    /// ```
    fn denial_statuses(&self, public: bool) -> &'static [u16] {
        if public { &[] } else { &[401, 403] }
    }

    /// How a route's document states this guard's
    /// [`denial_statuses`](Self::denial_statuses). The default,
    /// [`DenialDocument::PROBLEM`], is what a [`Denial`] renders: a problem
    /// document, and a `Bearer` challenge on a `401`. A guard answering in its
    /// protocol's own words says so, as a client refused at a token endpoint is
    /// answered RFC 6749 §5.2's body and a `Basic` challenge.
    fn denial_document(&self) -> DenialDocument {
        DenialDocument::PROBLEM
    }

    /// The target the edge files this guard's denials on: the target constant
    /// of the crate owning the concern the guard decides. The default is
    /// `nest_rs::layers`.
    fn target(&self) -> &'static str {
        nest_rs_core::target::LAYERS
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
    note = "a guard's `check` decides the units it declares; bound where its units are another \
            edge's, it would run a decision written for a view they do not have",
    note = "write the guard's decision for `{V}` units in `Guard::check` (overriding \
            `Guard::check_http` only where the HTTP request differs from the unit), then declare \
            `impl GuardFor<{V}> for {Self} {{}}`"
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

    fn __attaches_ability(&self) -> bool {
        (**self).__attaches_ability()
    }

    fn denial_statuses(&self, public: bool) -> &'static [u16] {
        (**self).denial_statuses(public)
    }

    fn denial_document(&self) -> DenialDocument {
        (**self).denial_document()
    }

    fn target(&self) -> &'static str {
        (**self).target()
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

    /// Refuses a caller outside the office network, wherever it is bound.
    struct OfficeOnly;

    impl Layer for OfficeOnly {}

    #[async_trait]
    impl Guard for OfficeOnly {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Err(Denial::forbidden("outside the office network"))
        }

        fn denial_statuses(&self, _public: bool) -> &'static [u16] {
            &[403]
        }
    }

    #[test]
    fn a_guard_declaring_no_statuses_answers_an_authentication_guard_s_off_a_public_route() {
        let guard: Arc<dyn Guard> = Arc::new(OneDecision);
        assert_eq!(guard.denial_statuses(false), [401, 403]);
        assert!(
            guard.denial_statuses(true).is_empty(),
            "a guard that says nothing claims no refusal on a `#[public]` route",
        );
    }

    #[test]
    fn an_arc_answers_the_statuses_its_guard_declares() {
        let guard = Arc::new(OfficeOnly);
        assert_eq!(Guard::denial_statuses(&guard, false), [403]);
        assert_eq!(Guard::denial_statuses(&guard, true), [403]);
    }

    /// Writes its decision once, in the unit entry.
    struct OneDecision;

    impl Layer for OneDecision {}

    #[async_trait]
    impl Guard for OneDecision {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Err(Denial::forbidden("refused wherever it is bound"))
        }
    }

    #[tokio::test]
    async fn a_guard_writing_one_decision_refuses_an_http_request_with_it() {
        let mut view = Socket;
        let mut cx = nest_rs_core::__private::new_unit_context(&MESSAGE, &mut view, None, None);
        let guard: Arc<dyn Guard> = Arc::new(OneDecision);

        let http = guard.as_ref().check_http(&mut cx).await;
        assert_eq!(
            http.map_err(|denial| denial.http_status()),
            Err(403),
            "an HTTP request runs the one decision a guard writes, never an admit-all default",
        );
    }

    /// What an earlier guard attaches.
    struct Author;

    impl nest_rs_core::AmbientValue for Author {}

    /// Requires what no guard before it attached.
    struct RequiresAuthor;

    impl Layer for RequiresAuthor {}

    #[async_trait]
    impl Guard for RequiresAuthor {
        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            cx.require::<Author>()?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_missing_precondition_refuses_with_a_500_naming_the_value() {
        let mut view = Socket;
        let mut cx = nest_rs_core::__private::new_unit_context(&MESSAGE, &mut view, None, None);
        let guard: Arc<dyn Guard> = Arc::new(RequiresAuthor);

        let unit = guard.as_ref().check(&mut cx).await;
        let denial = unit.expect_err("an unwired guard fails closed");
        assert_eq!(denial.http_status(), 500);
        assert_eq!(denial.reason(), Some(crate::denial::MISSING_AMBIENT));
        let chain = crate::denial::failure_chain(&denial).unwrap_or_default();
        assert!(chain.contains("no `Author` is ambient"), "{chain}");

        cx.attach(Author);
        let unit = guard.as_ref().check(&mut cx).await;
        assert!(unit.is_ok(), "attached earlier in the stage, it is there");
    }

    #[tokio::test]
    async fn an_async_trait_override_of_each_entry_is_the_one_that_runs() {
        let mut view = Socket;
        let mut cx = nest_rs_core::__private::new_unit_context(&MESSAGE, &mut view, None, None);
        let guard: Arc<dyn Guard> = Arc::new(Both);

        let http = guard.check_http(&mut cx).await;
        assert_eq!(http.map_err(|d| d.http_status()), Err(401));
        let unit = guard.check(&mut cx).await;
        assert_eq!(unit.map_err(|d| d.http_status()), Err(403));
    }
}
