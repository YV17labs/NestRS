//! Per-route guard that runs a [`Strategy`] and attaches the principal.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::__private::TenantId;
use nest_rs_core::trace_context::field;
use nest_rs_core::{
    AmbientValue, Container, ContainerBuilder, Discoverable, Layer, Net, Posture,
    ProviderResidency, Unanswered, UnitContext, within,
};
use nest_rs_guards::{Denial, GrantedScopes, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::{HttpView, RejectedCredential, async_trait};

use crate::PrincipalIdentity;
use crate::error::{AuthError, UNAVAILABLE, UnansweredError};
use crate::strategy::{AUTHENTICATE_TIMEOUT, Strategy};

/// The edge this guard authenticates on: `http` whichever edge the request is
/// for, since a route, GraphQL, MCP and the WS upgrade all arrive as HTTP.
const TRANSPORT: &str = "http";

/// Why a credential went unevaluated, the `reason` of its denial's line beside
/// what the strategy itself answers ([`AuthError::reason`]).
mod reason {
    pub(super) const STRATEGY_PANICKED: &str = "strategy_panicked";
    pub(super) const STRATEGY_TIMEOUT: &str = "strategy_timeout";
}

/// The authentication guard: runs a [`Strategy`] on the request, attaches the
/// resulting principal — its granted scopes and the tenant it acts within
/// beside it — for the rest of the request, and records `actor_id` on the
/// span. Generic over the
/// strategy `S`. Bind it via `#[use_guards]` / `use_guards_global`; on a
/// `#[public]` route it authenticates opportunistically but never rejects, and
/// on an in-band endpoint (`/graphql`) it admits an anonymous request while
/// still refusing a credential that does not verify.
///
/// It waits for the strategy no longer than [`AUTHENTICATE_TIMEOUT`]: a
/// strategy silent past it, or one that panics, is denied on every route,
/// `#[public]` included. So the boot refuses every budget the strategy's code
/// can reach at or past it (`nest_rs_core::BudgetPastNetError`).
pub struct AuthnGuard<S: Strategy> {
    strategy: Arc<S>,
}

// Registered by hand rather than by `#[injectable]` for what the decorator
// cannot say: the net around its strategy, declared by each app that runs it.
impl<S: Strategy> Discoverable for AuthnGuard<S> {
    fn dependencies() -> Vec<TypeId> {
        vec![TypeId::of::<S>()]
    }

    fn dependency_names() -> Vec<&'static str> {
        vec![std::any::type_name::<S>()]
    }

    fn injected() -> Vec<TypeId> {
        Self::dependencies()
    }

    fn injected_names() -> Vec<&'static str> {
        Self::dependency_names()
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let guard = Self::from_container(&builder.snapshot());
        builder.provide(guard).provide_meta(Net::around::<S>(
            "the authentication guard",
            AUTHENTICATE_TIMEOUT,
        ))
    }

    // A double arms no net: the strategy runs under none.
    fn __nestrs_register_overridden(builder: ContainerBuilder) -> ContainerBuilder {
        nest_rs_core::__private::claim_own_override::<Self>(
            builder,
            nest_rs_core::ProviderScope::Singleton,
        )
    }
}

impl<S: Strategy> ProviderResidency for AuthnGuard<S> {
    const SINGLETON: bool = true;
}

impl<S: Strategy> AuthnGuard<S> {
    /// Construct with an already-resolved strategy (container or tests). A
    /// guard built by hand and seeded declares no net: the boot holds budgets
    /// under the net of a guard the container builds.
    pub fn new(strategy: Arc<S>) -> Self {
        Self { strategy }
    }

    /// Construct this provider by resolving its strategy from the container —
    /// what the register phase calls.
    fn from_container(container: &Container) -> Self {
        #[expect(
            clippy::expect_used,
            reason = "the register phase builds a provider only once its `dependencies` are registered"
        )]
        let strategy = container
            .get::<S>()
            .expect("AuthnGuard.strategy: no provider registered for this dependency");
        Self::new(strategy)
    }
}

impl<S: Strategy> Layer for AuthnGuard<S> {}

/// Layer-System impl — registers globally via `use_guards_global`.
///
/// On a `#[public]` route the guard authenticates opportunistically: it attaches
/// the principal when a token verifies and otherwise continues anonymously (a
/// rejected credential logged at `warn`). On `Posture::InBand` it admits a
/// request carrying no credential, left to each operation's own gate, and
/// refuses one whose credential does not verify. [`AuthError::Unavailable`], a
/// strategy silent past [`AUTHENTICATE_TIMEOUT`] and one that panics are not
/// absorbed: the credential was never evaluated, so the request fails closed
/// with a 503.
#[async_trait]
impl<S: Strategy> Guard for AuthnGuard<S> {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let strategy = std::any::type_name::<S>();
        let posture = cx.reflector().posture();
        let is_public = posture == Posture::Public;
        match authenticate(&*self.strategy, cx).await? {
            Verdict::Admitted => Ok(()),
            // The credential was never evaluated: fail closed on every route,
            // `#[public]` included, or an outage downgrades every session to anonymous.
            Verdict::Unavailable(denial) => Err(denial),
            Verdict::Refused(AuthError::MissingCredentials) if is_public => {
                tracing::debug!(target: crate::TARGET, strategy, "anonymous request on a public route");
                Ok(())
            }
            Verdict::Refused(AuthError::MissingCredentials) if posture == Posture::InBand => {
                tracing::debug!(target: crate::TARGET, strategy, "anonymous request on an in-band endpoint");
                Ok(())
            }
            Verdict::Refused(error) if is_public => {
                tracing::warn!(
                    target: crate::TARGET,
                    strategy,
                    reason = error.reason(),
                    error = %nest_rs_core::error_message(&error),
                    "rejected credential on a public route — continuing as anonymous",
                );
                // Recorded so a handler that does read a principal answers the
                // 401 this credential earned, not a 500: on the request, which
                // shapes the edge's answer, never attached as an identity.
                HttpView::of(cx)?
                    .request_mut()
                    .extensions_mut()
                    .insert(RejectedCredential::of::<S::Principal>(
                        error.client_message(),
                        error.error_code(),
                    ));
                Ok(())
            }
            Verdict::Refused(error) => {
                let denial = match error.error_code() {
                    Some(code) => Denial::invalid_credential(error.client_message(), code),
                    None => Denial::unauthorized(error.client_message()),
                };
                Err(denial.with_reason(error.reason()).with_source(error))
            }
        }
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<S::Principal>())
    }

    /// `503` on every route, `#[public]` included: a credential the strategy
    /// could not evaluate. `401` off a `#[public]` route: one that is missing
    /// or refused.
    fn denial_statuses(&self, public: bool) -> &'static [u16] {
        if public { &[503] } else { &[401, 503] }
    }

    /// Every refusal is filed on `nest_rs::authn`, the strategy named by the
    /// guard's type and the credential's fault by the reason.
    fn target(&self) -> &'static str {
        crate::TARGET
    }
}

/// What running a [`Strategy`] on a request decided.
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "each guard over a strategy matches every verdict, so one added fails its build rather than falling into a wildcard"
)]
pub enum Verdict {
    /// The strategy authenticated the caller: its principal is attached, with
    /// the scopes it was granted and the tenant it acts within, and its
    /// `actor_id` recorded.
    Admitted,
    /// The strategy refused the credential, or found none; nothing is filed.
    Refused(AuthError),
    /// Nothing was decided about the credential — what the strategy asks did
    /// not answer, it was silent past [`AUTHENTICATE_TIMEOUT`], or it panicked —
    /// the `503` the request is refused with, its reason and cause on it for
    /// the edge to file (a panic is filed on its own line besides).
    Unavailable(Denial),
}

/// Run `strategy` on the request `cx` dispatches, as every guard over a
/// [`Strategy`] runs it: within [`AUTHENTICATE_TIMEOUT`], a panic contained,
/// and on success the principal attached for the rest of the request — its
/// granted scopes and its tenant beside it — and its `actor_id` recorded on
/// the span and in the ambient context.
///
/// The JWT strategy answers at the first poll — against a static key, or a
/// JWK Set already held — so the common path pays no timer. `Err` is a unit
/// the HTTP edge did not dispatch.
#[doc(hidden)]
pub async fn authenticate<S: Strategy>(
    strategy: &S,
    cx: &mut UnitContext<'_>,
) -> Result<Verdict, Denial> {
    let name = std::any::type_name::<S>();
    let request = HttpView::of(cx)?.request_mut();
    let outcome = match within(AUTHENTICATE_TIMEOUT, strategy.authenticate(request)).await {
        Ok(outcome) => outcome,
        Err(unanswered) => {
            let (reason, cause) = match unanswered {
                Unanswered::Panicked(payload) => {
                    nest_rs_core::contained_panic!(
                        target: crate::TARGET,
                        payload.as_ref(),
                        "strategy panicked; denying (fail-closed)",
                        strategy = name,
                        transport = TRANSPORT,
                    );
                    (
                        reason::STRATEGY_PANICKED,
                        UnansweredError::Panicked { strategy: name },
                    )
                }
                _ => (
                    reason::STRATEGY_TIMEOUT,
                    UnansweredError::Silent {
                        strategy: name,
                        waited_ms: u64::try_from(AUTHENTICATE_TIMEOUT.as_millis())
                            .unwrap_or(u64::MAX),
                    },
                ),
            };
            let denial = Denial::unavailable(None, UNAVAILABLE)
                .with_reason(reason)
                .with_source(cause);
            return Ok(Verdict::Unavailable(denial));
        }
    };
    match outcome {
        Ok(principal) => {
            attach(cx, principal, name);
            Ok(Verdict::Admitted)
        }
        Err(error @ AuthError::Unavailable { .. }) => {
            let denial = Denial::unavailable(error.retry_after_secs(), error.client_message())
                .with_reason(error.reason())
                .with_source(error);
            Ok(Verdict::Unavailable(denial))
        }
        Err(error) => Ok(Verdict::Refused(error)),
    }
}

/// Make `principal` the request's: its `actor_id` on the span and in the
/// ambient context, its granted scopes and its tenant beside it.
fn attach<P: PrincipalIdentity + AmbientValue>(
    cx: &mut UnitContext<'_>,
    principal: P,
    strategy: &'static str,
) {
    // The field is declared by the kernel's `operation_span!`, never by the
    // observability crate an app may not install.
    if let Some(actor_id) = principal.actor_id() {
        tracing::Span::current().record(field::ACTOR_ID, actor_id.as_str());
        // …and into the ambient context for the layers below. Write-once.
        nest_rs_core::__private::set_actor_id(&actor_id);
    }
    // No `actor_id` field: every line carries it off the ambient context.
    tracing::debug!(target: crate::TARGET, strategy, "authenticated");
    // A principal that is not scope-aware attaches nothing, and the absence is
    // what turns scope gating off.
    if let Some(scopes) = principal.scopes() {
        cx.attach(GrantedScopes::new(scopes));
    }
    // The tenant comes off the verified principal and nothing else.
    if let Some(tenant) = principal.tenant_id().and_then(TenantId::new) {
        cx.attach(tenant);
    }
    cx.attach(principal);
}

/// HTTP is the only edge this guard checks, and enough for all: the GraphQL
/// POST, the `/mcp` request and the WS upgrade are HTTP requests, and what it
/// attaches there is ambient for every operation they carry. The declaration
/// lets a `#[controller]` or a `#[gateway]` struct bind it.
impl<S: Strategy> GuardFor<HttpView> for AuthnGuard<S> {}

#[cfg(test)]
mod tests {
    use nest_rs_http::Request;

    use super::*;

    #[derive(Clone)]
    struct Anyone;

    impl AmbientValue for Anyone {}

    impl PrincipalIdentity for Anyone {
        fn actor_id(&self) -> Option<String> {
            None
        }
    }

    struct Admits;

    #[async_trait]
    impl Strategy for Admits {
        type Principal = Anyone;

        async fn authenticate(&self, _req: &mut Request) -> Result<Anyone, AuthError> {
            Ok(Anyone)
        }
    }

    #[test]
    fn it_declares_an_unevaluated_credential_everywhere_and_a_refused_one_off_a_public_route() {
        let guard = AuthnGuard::new(Arc::new(Admits));
        assert_eq!(guard.denial_statuses(false), [401, 503]);
        assert_eq!(guard.denial_statuses(true), [503]);
    }
}
