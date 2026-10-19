//! Per-route guard that runs a [`Strategy`] and attaches the principal.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::__private::TenantId;
use nest_rs_core::trace_context::field;
use nest_rs_core::{
    Container, ContainerBuilder, Discoverable, Layer, Net, Posture, ProviderResidency, Unanswered,
    UnitContext, within,
};
use nest_rs_guards::{Denial, GrantedScopes, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::{HttpView, RejectedCredential, Request, async_trait};

use crate::error::{AuthError, UNAVAILABLE};
use crate::strategy::{AUTHENTICATE_TIMEOUT, Strategy};

/// The edge this guard authenticates on: `http` whichever edge the request is
/// for, since a route, GraphQL, MCP and the WS upgrade all arrive as HTTP.
const TRANSPORT: &str = "http";

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

    /// Run the strategy on `req`, waiting no longer than
    /// [`AUTHENTICATE_TIMEOUT`] for its answer. `None` is a strategy that did
    /// not answer in time, or that panicked, already said here — at `warn`, at
    /// `error`.
    ///
    /// The JWT strategy answers at the first poll — against a static key, or a
    /// JWK Set already held — so the common path pays no timer.
    async fn authenticate(
        &self,
        strategy: &'static str,
        req: &mut Request,
    ) -> Option<Result<S::Principal, AuthError>> {
        let authenticate = async { self.strategy.authenticate(req).await };
        match within(AUTHENTICATE_TIMEOUT, authenticate).await {
            Ok(outcome) => Some(outcome),
            Err(Unanswered::Panicked(payload)) => {
                nest_rs_core::contained_panic!(
                    target: crate::TARGET,
                    payload.as_ref(),
                    "strategy panicked; denying (fail-closed)",
                    strategy,
                    transport = TRANSPORT,
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    target: crate::TARGET,
                    strategy,
                    transport = TRANSPORT,
                    waited_ms = u64::try_from(AUTHENTICATE_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
                    "strategy did not answer within the guard's timeout; denying (fail-closed)",
                );
                None
            }
        }
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
        let outcome = self
            .authenticate(strategy, HttpView::of(cx)?.request_mut())
            .await;
        // Nothing was decided about the credential: the same 503 as an
        // unreachable store, on every route. `authenticate` already said why.
        let Some(outcome) = outcome else {
            return Err(Denial::unavailable(None, UNAVAILABLE));
        };
        match outcome {
            Ok(principal) => {
                // Record the audit identity on the request span. The field is
                // declared by the kernel's `operation_span!`, never by the
                // observability crate an app may not install.
                if let Some(actor_id) = crate::PrincipalIdentity::actor_id(&principal) {
                    tracing::Span::current().record(field::ACTOR_ID, actor_id.as_str());
                    // …and into the ambient context for the layers below. Write-once.
                    nest_rs_core::__private::set_actor_id(&actor_id);
                }
                // No `actor_id` field: every line carries it off the ambient context.
                tracing::debug!(target: crate::TARGET, strategy, "authenticated");
                // A principal that is not scope-aware attaches nothing, and the
                // absence is what turns scope gating off.
                if let Some(scopes) = crate::PrincipalIdentity::scopes(&principal) {
                    cx.attach(GrantedScopes::new(scopes));
                }
                // The tenant comes off the verified principal and nothing else.
                if let Some(tenant) =
                    crate::PrincipalIdentity::tenant_id(&principal).and_then(TenantId::new)
                {
                    cx.attach(tenant);
                }
                cx.attach(principal);
                Ok(())
            }
            // The credential was never evaluated: fail closed on every route,
            // `#[public]` included, or an outage downgrades every session to anonymous.
            Err(error @ AuthError::Unavailable { .. }) => {
                tracing::error!(
                    target: crate::TARGET,
                    strategy,
                    reason = error.reason(),
                    error = %nest_rs_core::error_message(&error),
                    "authentication unavailable — what the strategy asks did not answer",
                );
                Err(Denial::unavailable(
                    error.retry_after_secs(),
                    error.client_message(),
                ))
            }
            Err(AuthError::MissingCredentials) if is_public => {
                tracing::debug!(target: crate::TARGET, strategy, "anonymous request on a public route");
                Ok(())
            }
            Err(AuthError::MissingCredentials) if posture == Posture::InBand => {
                tracing::debug!(target: crate::TARGET, strategy, "anonymous request on an in-band endpoint");
                Ok(())
            }
            Err(error) if is_public => {
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
                    .insert(RejectedCredential {
                        principal: std::any::TypeId::of::<S::Principal>(),
                        client_message: error.client_message().to_owned(),
                        bearer_error: error.error_code(),
                    });
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    target: crate::TARGET,
                    strategy,
                    reason = error.reason(),
                    error = %nest_rs_core::error_message(&error),
                    "authentication failed",
                );
                Err(match error.error_code() {
                    Some(code) => Denial::invalid_credential(error.client_message(), code),
                    None => Denial::unauthorized(error.client_message()),
                })
            }
        }
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<S::Principal>())
    }

    /// Every refusal is filed here, on `nest_rs::authn`, with the strategy and
    /// its reason.
    fn files_its_denials(&self) -> bool {
        true
    }
}

/// HTTP is the only edge this guard checks, and enough for all: the GraphQL
/// POST, the `/mcp` request and the WS upgrade are HTTP requests, and what it
/// attaches there is ambient for every operation they carry. The declaration
/// lets a `#[controller]` or a `#[gateway]` struct bind it.
impl<S: Strategy> GuardFor<HttpView> for AuthnGuard<S> {}
