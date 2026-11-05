//! [`ClientAuthenticationGuard`] — RFC 6749 §2.3's client authentication at
//! the token endpoint, refusing as §5.2 answers.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_authn::__private::{Verdict, authenticate, check_unit};
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, Strategy};
use nest_rs_core::{
    ChallengeScheme, Container, ContainerBuilder, DenialDocument, Discoverable, Layer, Net,
    ProviderResidency, UnitContext, async_trait,
};
use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_http::{HeaderMap, HeaderValue, HttpView, Response, ResponseError, header};

use crate::error::{ClientRefusedError, TokenEndpointAuthMethod, TokenError};
use crate::strategy::ClientSecretBasicStrategy;

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
/// [`ClientSecretBasicStrategy`] is the strategy for the clients
/// [`OAuthServerConfig`](crate::OAuthServerConfig) registers, behind
/// [`ClientGuard`]; a registry held elsewhere is a strategy of the app's own:
///
/// ```
/// use nest_rs_authn::{AuthError, Strategy, basic_credentials};
/// use nest_rs_core::{Ambient, async_trait, injectable};
/// use nest_rs_http::{Request, controller, routes};
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
///     async fn token(
///         &self,
///         client: Ambient<AuthenticatedClient<()>>,
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

    /// A unit reaches this check from the pool or a subgraph's `_service`, its
    /// request's `check_http` having authenticated it, and a request running no
    /// guard (`McpAccess::Public`) carries a gated unit no principal reached.
    fn check<'s, 'c, 'u, 'fut>(
        &'s self,
        cx: &'c mut UnitContext<'u>,
    ) -> Pin<Box<dyn Future<Output = Result<(), Denial>> + Send + 'fut>>
    where
        's: 'fut,
        'c: 'fut,
        'u: 'fut,
        Self: 'fut,
    {
        check_unit::<S::Principal>(cx)
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

    /// §5.2's words: [`TokenError`]'s body, and the `Basic` challenge §2.3.1
    /// makes the method every authorization server reads.
    fn denial_document(&self) -> DenialDocument {
        DenialDocument::answering(ChallengeScheme::Basic, |generator| {
            generator.subschema_for::<TokenError>()
        })
    }

    /// Every refusal is filed on `nest_rs::oauth::server`, its reason the
    /// strategy's and its cause naming the method attempted.
    fn target(&self) -> &'static str {
        crate::TARGET
    }
}

/// The guard of a token endpoint whose clients
/// [`OAuthServerConfig<P>`](crate::OAuthServerConfig) registers: RFC 6749
/// §2.3.1's HTTP Basic pair checked against the registry, provided by
/// [`OAuthServerModule::<P>`](crate::OAuthServerModule) and bound with
/// `#[use_guards(ClientGuard<P>)]`.
pub type ClientGuard<P> = ClientAuthenticationGuard<ClientSecretBasicStrategy<P>>;

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
    use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler, declare_unit};
    use nest_rs_core::operation_log::Kind;
    use nest_rs_core::{Handler, Posture, UnitView};

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
        let guard = ClientAuthenticationGuard::new(Arc::new(Never));
        for public in [false, true] {
            assert_eq!(guard.denial_statuses(public), [400, 401, 503]);
        }
    }

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

    struct Operation;

    impl UnitView for Operation {}

    static OPERATION: Handler = declare_handler(HandlerDeclaration {
        unit: declare_unit("nest-rs-mcp", "nest_rs::mcp", Kind::Server, "mcp.operation"),
        host: TypeId::of::<Operation>(),
        host_name: "Operation",
        method: "token",
        address: "token",
        posture: Posture::Gated,
        meta: MetaLevels::default,
    });

    /// The guard's `check` on a gated unit, the future's size read before it runs.
    async fn checked() -> (usize, Result<(), Denial>) {
        let guard = ClientAuthenticationGuard::new(Arc::new(Never));
        let mut view = Operation;
        let mut cx = nest_rs_core::__private::new_unit_context(&OPERATION, &mut view, None, None);
        let check = guard.check(&mut cx);
        (std::mem::size_of_val(&*check), check.await)
    }

    /// A zero-sized future boxes into no allocation, on every unit the pool
    /// admits.
    #[tokio::test]
    async fn it_admits_a_gated_unit_its_client_reached_without_allocating() {
        let client = crate::AuthenticatedClient {
            client_id: "ci".to_owned(),
            scopes: Vec::new(),
            resources: Vec::new(),
            payload: (),
        };
        let (size, admitted) = nest_rs_core::with_ambient(Arc::new(client), checked()).await;
        assert_eq!(size, 0);
        assert!(admitted.is_ok());
    }

    /// A request running no guard, `McpAccess::Public`'s, carries units the pool
    /// still folds.
    #[tokio::test]
    async fn it_refuses_a_gated_unit_no_client_reached() {
        let (_, refused) = checked().await;
        assert_eq!(refused.expect_err("no client").http_status(), 401);
    }
}
