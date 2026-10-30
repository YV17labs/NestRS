//! [`ClientAuthenticationGuard`] — RFC 6749 §2.3's client authentication at
//! the token endpoint, refusing as §5.2 answers.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_authn::__private::{Verdict, authenticate};
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, Strategy};
use nest_rs_core::{
    Container, ContainerBuilder, Discoverable, Layer, Net, ProviderResidency, UnitContext,
};
use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::{
    HeaderMap, HeaderValue, HttpView, Response, ResponseError, async_trait, header,
};

use crate::error::{ClientRefusedError, TokenEndpointAuthMethod, TokenError};

/// The client-authentication guard of a token endpoint (RFC 6749 §2.3): runs
/// a [`Strategy`] on the request, as [`AuthnGuard`](nest_rs_authn::AuthnGuard)
/// runs one — within [`AUTHENTICATE_TIMEOUT`], a panic contained, the client
/// attached as the request's principal with its scopes, its tenant and its
/// `actor_id` — and refuses as §5.2 answers, through
/// [`TokenError`]'s rendering:
///
/// - a client refused over the `Authorization` header, whatever its scheme,
///   `401` with the `Basic` challenge, `invalid_client`;
/// - one that did not use the header, `400` `invalid_client`, no challenge —
///   credentials in the body (`client_secret_post`, §2.3.1's "NOT
///   RECOMMENDED") are not read, so they answer as none;
/// - a strategy that could not evaluate the client, `503` `server_error`,
///   with the `Retry-After` its store gave.
///
/// It reads the request's head alone, never its body, and refuses on every
/// route, `#[public]` included: every client authenticates at a token endpoint
/// (§3.2.1). Each refusal files one line on [`TARGET`](crate::TARGET), its
/// reason the strategy's and its cause naming the method attempted, never the
/// client.
///
/// [`ClientSecretBasicStrategy`](crate::ClientSecretBasicStrategy) is the
/// strategy for the clients [`OAuthServerConfig`](crate::OAuthServerConfig)
/// registers; a registry held elsewhere is a strategy of the app's own:
///
/// ```
/// use nest_rs_authn::{AuthError, Strategy, basic_credentials};
/// use nest_rs_core::{Ctx, injectable};
/// use nest_rs_http::{Request, async_trait, controller, routes};
/// use nest_rs_oauth_server::{
///     AccessTokenRequest, AccessTokenResponse, AuthenticatedClient, ClientAuthenticationGuard,
///     RegisteredClient, authenticate_against_registry,
/// };
///
/// #[injectable]
/// #[derive(Default)]
/// struct ClientCredentials;
///
/// #[async_trait]
/// impl Strategy for ClientCredentials {
///     type Principal = AuthenticatedClient<()>;
///
///     async fn authenticate(&self, req: &mut Request) -> Result<Self::Principal, AuthError> {
///         let (id, secret) = basic_credentials(req.headers()).ok_or(AuthError::MissingCredentials)?;
///         let clients = [RegisteredClient::new(
///             "billing",
///             "s3cret",
///             ["invoices:read"],
///             ["https://invoices.example.com"],
///             (),
///         )];
///         authenticate_against_registry(&clients, &id, &secret)
///     }
/// }
///
/// type ClientGuard = ClientAuthenticationGuard<ClientCredentials>;
///
/// #[controller(path = "/")]
/// struct TokenController;
///
/// #[routes]
/// impl TokenController {
///     #[post("/token")]
///     #[use_guards(ClientGuard)]
///     #[api(response = AccessTokenResponse)]
///     async fn token(
///         &self,
///         client: Ctx<AuthenticatedClient<()>>,
///         request: AccessTokenRequest,
///     ) -> nest_rs_http::Result<AccessTokenResponse> {
///         let scope = client.scope_for(&request.scope)?;
///         Ok(AccessTokenResponse::bearer("t", 3600).with_scope(scope))
///     }
/// }
/// ```
#[derive(Layer)]
pub struct ClientAuthenticationGuard<S: Strategy> {
    strategy: Arc<S>,
}

// Registered by hand rather than by `#[injectable]`, as `AuthnGuard` is: the
// net around its strategy is what the decorator cannot say.
impl<S: Strategy> Discoverable for ClientAuthenticationGuard<S> {
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
            "the client authentication guard",
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

impl<S: Strategy> ProviderResidency for ClientAuthenticationGuard<S> {
    const SINGLETON: bool = true;
}

impl<S: Strategy> ClientAuthenticationGuard<S> {
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
        let strategy = container.get::<S>().expect(
            "ClientAuthenticationGuard.strategy: no provider registered for this dependency",
        );
        Self::new(strategy)
    }
}

#[async_trait]
impl<S: Strategy> Guard for ClientAuthenticationGuard<S> {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let method = attempted(HttpView::of(cx)?.request().headers());
        match authenticate(&*self.strategy, cx).await? {
            Verdict::Admitted => Ok(()),
            Verdict::Unavailable(denial) => {
                let mut answer =
                    TokenError::Server(anyhow::Error::new(denial.clone())).as_response();
                if let Some(wait) = denial.retry_after_secs() {
                    answer
                        .headers_mut()
                        .insert(header::RETRY_AFTER, HeaderValue::from(wait));
                }
                refuse(cx, answer, denial)
            }
            Verdict::Refused(error) => {
                let answer = TokenError::InvalidClient(method).as_response();
                let denial = Denial::unauthorized(error.client_message())
                    .with_reason(error.reason())
                    .with_source(ClientRefusedError { method, error });
                refuse(cx, answer, denial)
            }
        }
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<S::Principal>())
    }

    /// On every route, `#[public]` included: `400` and `401`, a client refused
    /// (§5.2), and `503`, one the strategy could not evaluate.
    fn denial_statuses(&self, _public: bool) -> &'static [u16] {
        &[400, 401, 503]
    }

    /// Every refusal is filed on `nest_rs::oauth::server`, its reason the
    /// strategy's and its cause naming the method attempted.
    fn target(&self) -> &'static str {
        crate::TARGET
    }
}

/// HTTP is the only edge a token endpoint answers on; the declaration lets a
/// `#[controller]` or a `#[routes]` verb bind it.
impl<S: Strategy> GuardFor<HttpView> for ClientAuthenticationGuard<S> {}

/// The method the request attempted, read off its head alone (RFC 6749
/// §5.2): an `Authorization` header with any scheme, decodable or not, is
/// HTTP Basic's — the one this server reads there — and none is no client
/// authentication, whatever the body holds.
fn attempted(headers: &HeaderMap) -> TokenEndpointAuthMethod {
    if headers.contains_key(header::AUTHORIZATION) {
        TokenEndpointAuthMethod::ClientSecretBasic
    } else {
        TokenEndpointAuthMethod::None
    }
}

/// Refuse the unit with `answer`, §5.2's words, which the guard stage serves
/// in place of `denial`'s rendering.
fn refuse(cx: &mut UnitContext<'_>, answer: Response, denial: Denial) -> Result<(), Denial> {
    drop(HttpView::of(cx)?.respond(answer));
    Err(denial)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(authorization: Option<&'static str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = authorization {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn a_request_naming_any_scheme_attempted_basic() {
        for authorization in [
            "Basic Y2k6czNjcmV0",
            "Basic !!not-base64!!",
            "Bearer a-token",
            "Digest username=\"ci\"",
            "",
        ] {
            assert_eq!(
                attempted(&headers(Some(authorization))),
                TokenEndpointAuthMethod::ClientSecretBasic,
                "{authorization:?}"
            );
        }
    }

    #[test]
    fn a_request_without_the_header_attempted_none() {
        assert_eq!(attempted(&headers(None)), TokenEndpointAuthMethod::None);
    }

    #[test]
    fn it_declares_both_refusals_and_an_unevaluated_client_on_every_route() {
        struct Never;

        #[async_trait]
        impl Strategy for Never {
            type Principal = crate::AuthenticatedClient<()>;

            async fn authenticate(
                &self,
                _req: &mut nest_rs_http::Request,
            ) -> Result<Self::Principal, nest_rs_authn::AuthError> {
                Err(nest_rs_authn::AuthError::MissingCredentials)
            }
        }

        let guard = ClientAuthenticationGuard::new(Arc::new(Never));
        for public in [false, true] {
            assert_eq!(guard.denial_statuses(public), [400, 401, 503]);
        }
    }
}
