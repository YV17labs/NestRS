//! Per-route guard that runs a [`Strategy`] and attaches the principal.

use std::any::TypeId;
use std::future::{Future as _, poll_fn};
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;

use nest_rs_core::trace_context::field;
use nest_rs_core::{Container, ContainerBuilder, Discoverable, Layer, Net, ProviderResidency};
use nest_rs_guards::{Denial, GrantedScopes, Guard, GuardPhase, PrincipalClaim};
use nest_rs_http::HandlerMetadata;
use nest_rs_http::{Reflector, RejectedCredential, async_trait};
use poem::Request;

use crate::error::{AuthError, UNAVAILABLE};
use crate::strategy::{AUTHENTICATE_TIMEOUT, Strategy};

/// The edge this guard authenticates on, as the `transport` field its lines
/// share with every other guard's names it.
///
/// It is `http` whichever edge the request is for: a route, the GraphQL POST,
/// the MCP POST and the WebSocket upgrade are all HTTP requests, and
/// `check_http` is the one entry that sees any of them. The line's trace
/// context joins it to that request's operation line, which carries the route.
const TRANSPORT: &str = "http";

/// The authentication guard: runs a [`Strategy`] on the request, attaches the
/// resulting principal, and records `actor_id` on the span. Generic over the
/// strategy `S`. Bind it via `#[use_guards]` / `use_guards_global`; on a
/// `#[public]` route it authenticates opportunistically but never rejects.
///
/// It waits for the strategy no longer than [`AUTHENTICATE_TIMEOUT`]: a
/// strategy silent past it is denied on every route, `#[public]` included. So
/// the boot refuses every budget the strategy's code can reach at or past it
/// (`nest_rs_core::BudgetPastNetError`).
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
    /// what the register phase calls, not by hand.
    pub fn from_container(container: &Container) -> Self {
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
    /// not answer in time, already said here at `warn`.
    ///
    /// The strategy's call is polled once bare before the bound is armed, as
    /// the HTTP edge arms its request timeout: the JWT strategy verifies locally
    /// and answers on that first poll, so the common path pays neither the clock
    /// read nor the timer entry a bound costs.
    async fn authenticate(
        &self,
        strategy: &'static str,
        req: &mut Request,
    ) -> Option<Result<S::Principal, AuthError>> {
        let mut authenticate = pin!(self.strategy.authenticate(req));
        if let Poll::Ready(outcome) =
            poll_fn(|cx| Poll::Ready(authenticate.as_mut().poll(cx))).await
        {
            return Some(outcome);
        }
        match tokio::time::timeout(AUTHENTICATE_TIMEOUT, authenticate).await {
            Ok(outcome) => Some(outcome),
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

/// Layer-System impl — registers globally via
/// `App::builder().use_guards_global([guard::<AuthnGuard>(), ...])` and is the
/// canonical path. `check_graphql` and `check_ws_message` keep the no-op
/// defaults because the GraphQL POST and WS upgrade are both HTTP requests
/// this `check_http` covers at the connection edge.
///
/// On a `#[public]` route, the guard authenticates opportunistically and does
/// not reject a *credential* failure: it attaches the principal when a token
/// verifies (so a downstream policy guard sees who is calling) and otherwise
/// continues anonymously — a rejected credential logged at `warn`, a plain
/// anonymous call at `debug`. Visitor-rule policy belongs in the authorization
/// layer, not in `AuthnGuard`. The one failure `#[public]` does **not** absorb
/// is [`AuthError::Unavailable`]: an unreachable identity store or provider
/// means the credential was never evaluated, so the request fails closed with a
/// 503 — carrying the provider's `Retry-After` when it gave one — rather than
/// being served as anonymous. A strategy that does not answer within
/// [`AUTHENTICATE_TIMEOUT`] left it unevaluated too, and gets the same answer.
#[async_trait]
impl<S: Strategy> Guard for AuthnGuard<S> {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let strategy = std::any::type_name::<S>();
        let is_public = Reflector::new(req).is_public();
        // Nothing was decided about the credential, so neither "authenticated"
        // nor "anonymous" is a true answer — the same position as the
        // unreachable store below, and the same denial on every route. The line
        // naming the strategy was filed where the bound passed.
        let Some(outcome) = self.authenticate(strategy, req).await else {
            return Err(Denial::unavailable(None, UNAVAILABLE));
        };
        match outcome {
            Ok(principal) => {
                // Record the audit identity on the request span so every
                // downstream event — denials included — inherits who is
                // calling. The field is declared by `operation_span!`, in the
                // kernel, unconditionally — never by the observability crate,
                // which an app may not install.
                if let Some(actor_id) = crate::PrincipalIdentity::actor_id(&principal) {
                    tracing::Span::current().record(field::ACTOR_ID, actor_id.as_str());
                    // …and into the ambient context, so a service, the data
                    // layer or a queue producer below this guard can read it
                    // without the handler threading it down. Write-once.
                    nest_rs_core::set_actor_id(&actor_id);
                }
                // One event either way, and it names no actor: every line the
                // console renders carries `actor_id` off the ambient context the
                // branch above just wrote, so spelling it here would print it
                // twice on the one line that proves who was resolved.
                tracing::debug!(target: crate::TARGET, strategy, "authenticated");
                // Publish what the credential was granted, so the authorization
                // layer can withhold the rules it does not reach. A principal
                // that is not scope-aware publishes nothing at all — the
                // absence is what tells the ability layer scope gating does not
                // apply, and is why a non-OAuth app is untouched by any of this.
                if let Some(scopes) = crate::PrincipalIdentity::scopes(&principal) {
                    req.extensions_mut().insert(GrantedScopes::new(scopes));
                }
                req.extensions_mut().insert(principal);
                Ok(())
            }
            // The store could not be reached, so the presented credential was
            // never evaluated — neither "authenticated" nor "anonymous" is a
            // true answer. Fail closed on every route, `#[public]` included:
            // admitting the caller as anonymous would silently downgrade every
            // authenticated session for the duration of the outage.
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
            // A public route admits the anonymous caller, but a credential that
            // was *presented and rejected* is a security event: a forged or
            // expired token probing a public endpoint must leave a queryable
            // trace, not a `debug` line.
            Err(AuthError::MissingCredentials) if is_public => {
                tracing::debug!(target: crate::TARGET, strategy, "anonymous request on a public route");
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
                // Anonymous is only an answer while nothing downstream needs a
                // principal. Record the rejection so a handler that *does* read
                // one (`Ctx<Claims>`) answers the 401 this credential earned,
                // instead of a 500 that hides a forged-credential attempt.
                req.extensions_mut().insert(RejectedCredential {
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
}

/// HTTP is the only edge this guard checks, and it is enough for all of them:
/// the GraphQL POST, the `/mcp` request and the WS upgrade are HTTP requests
/// `check_http` covers at the connection edge — and so is its one bound,
/// [`AUTHENTICATE_TIMEOUT`], which every edge therefore shares. The marker is
/// what lets a `#[controller]` or a `#[gateway]` struct bind it.
impl<S: Strategy> nest_rs_guards::HttpGuard for AuthnGuard<S> {}
