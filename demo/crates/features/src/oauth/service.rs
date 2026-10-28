use std::sync::Arc;

use nest_rs::authn::{AuthError, JwtService};
use nest_rs::core::injectable;
use nest_rs::oauth::client::AuthorizationRedirect;
use nest_rs::oauth::server::{AccessTokenResponse, TokenError, authenticate_against_registry};
use nest_rs::social::{SocialProfile, SocialRegistry};
use uuid::Uuid;

use super::config::{ClientPayload, OAuthConfig};
use super::scope::{granted_scopes, role_from_db};
use crate::authz::constants;
use crate::users::{SocialIdentity, UsersService};
use crate::{Claims, Role, Subject};

#[derive(Debug, Clone)]
pub struct Caller {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

pub type AuthenticatedClient = nest_rs::oauth::server::AuthenticatedClient<ClientPayload>;

impl nest_rs::core::AmbientValue for Caller {}

impl nest_rs::authn::PrincipalIdentity for Caller {
    fn actor_id(&self) -> Option<String> {
        Some(self.user_id.to_string())
    }
}

#[injectable]
pub struct OAuthService {
    #[inject]
    jwt_svc: Arc<JwtService>,
    #[inject]
    providers: Arc<SocialRegistry>,
    #[inject]
    users_svc: Arc<UsersService>,
    #[inject]
    config: Arc<OAuthConfig>,
}

impl OAuthService {
    pub fn new(
        jwt_svc: Arc<JwtService>,
        providers: Arc<SocialRegistry>,
        users_svc: Arc<UsersService>,
        config: Arc<OAuthConfig>,
    ) -> Self {
        Self {
            jwt_svc,
            providers,
            users_svc,
            config,
        }
    }

    pub fn issue(
        &self,
        resource: Option<&str>,
        user_id: Uuid,
        org_id: Uuid,
        roles: Vec<Role>,
    ) -> Result<AccessTokenResponse, TokenError> {
        issue_with_jwt(
            &self.jwt_svc,
            self.sign_in_audience(resource)?,
            Claims {
                sub: Subject::User(user_id),
                client_id: None,
                org_id,
                roles,
                scopes: constants::all(),
                exp: self.jwt_svc.expiry(),
            },
        )
    }

    pub async fn grant_password(
        &self,
        email: &str,
        password: &str,
        resource: Option<&str>,
    ) -> Result<AccessTokenResponse, TokenError> {
        self.sign_in_audience(resource)?;
        let user = self
            .users_svc
            .authenticate(email, password)
            .await
            .map_err(token_error_from_auth)?;
        let roles = vec![role_from_db(user.role)];
        self.issue(resource, user.id, user.org_id, roles)
    }

    pub fn grant_client_credentials(
        &self,
        grant_type: &str,
        scope: Option<&str>,
        resource: &[String],
        client: &AuthenticatedClient,
    ) -> Result<AccessTokenResponse, TokenError> {
        grant_client_credentials_with_jwt(&self.jwt_svc, grant_type, scope, resource, client)
    }

    fn sign_in_audience(&self, resource: Option<&str>) -> Result<&str, TokenError> {
        let resources = &self.config.sign_in_resources;
        match resource {
            None => resources.first(),
            Some(requested) => resources.iter().find(|served| *served == requested),
        }
        .map(String::as_str)
        .ok_or_else(|| refused(TokenError::InvalidTarget, "invalid_target"))
    }

    pub fn authorize(&self, provider: &str) -> Option<Result<AuthorizationRedirect, AuthError>> {
        let provider = self.providers.get(provider)?;
        Some(provider.authorize(&self.jwt_svc))
    }

    pub async fn resolve_caller(
        &self,
        provider: &str,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<Caller, AuthError> {
        let provider = self
            .providers
            .get(provider)
            .ok_or_else(|| AuthError::failed("unknown social provider"))?;

        let tokens = provider
            .exchange(&self.jwt_svc, transaction, state, code)
            .await?;
        let profile: SocialProfile = provider.profile(&tokens).await?;

        let user = self
            .users_svc
            .resolve_social_identity(&social_identity(profile), self.config.default_org_id)
            .await?;

        Ok(Caller {
            user_id: user.id,
            org_id: user.org_id,
            roles: vec![role_from_db(user.role)],
        })
    }

    pub fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<AuthenticatedClient, AuthError> {
        authenticate_against_registry(&self.config.clients, client_id, client_secret)
    }
}

fn social_identity(profile: SocialProfile) -> SocialIdentity {
    SocialIdentity {
        provider: profile.provider,
        subject: profile.subject,
        email: profile.email,
        email_verified: profile.email_verified,
        name: profile.name,
    }
}

pub(crate) fn issue_with_jwt(
    jwt_svc: &JwtService,
    audience: &str,
    claims: Claims,
) -> Result<AccessTokenResponse, TokenError> {
    let access_token = jwt_svc
        .sign_for(audience, &claims)
        .map_err(|e| TokenError::Sign(e.into()))?;
    tracing::debug!(
        target: crate::oauth::TARGET,
        audience,
        sub = %claims.sub,
        org_id = %claims.org_id,
        roles = ?claims.roles,
        scopes = ?claims.scopes,
        "issued access token"
    );
    Ok(AccessTokenResponse::bearer(access_token, jwt_svc.ttl_secs()).with_scope(claims.scopes))
}

pub(crate) fn grant_client_credentials_with_jwt(
    jwt_svc: &JwtService,
    grant_type: &str,
    scope: Option<&str>,
    resource: &[String],
    client: &AuthenticatedClient,
) -> Result<AccessTokenResponse, TokenError> {
    if grant_type != "client_credentials" {
        return Err(refused(
            TokenError::UnsupportedGrant,
            "unsupported_grant_type",
        ));
    }
    let scopes =
        granted_scopes(scope, &client.scopes).map_err(|error| refused(error, "invalid_scope"))?;
    let audience = client
        .audience_for(resource)
        .map_err(|error| refused(error, "invalid_target"))?;
    issue_with_jwt(
        jwt_svc,
        audience,
        Claims {
            sub: Subject::Client(client.client_id.clone()),
            client_id: Some(client.client_id.clone()),
            org_id: client.payload.org_id,
            roles: client.payload.roles.clone(),
            scopes,
            exp: jwt_svc.expiry(),
        },
    )
}

fn refused(error: TokenError, reason: &'static str) -> TokenError {
    tracing::warn!(
        target: crate::oauth::TARGET,
        reason,
        "token request refused"
    );
    error
}

fn token_error_from_auth(err: AuthError) -> TokenError {
    if matches!(err, AuthError::Unavailable { .. }) {
        TokenError::Server(err.into())
    } else {
        TokenError::InvalidGrant
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nest_rs::authn::{JwtOptions, JwtService};
    use std::time::Duration;

    use crate::authz::constants::{POSTS_READ, POSTS_WRITE};

    const SECRET: &str = "test-secret-padded-to-thirty-two-b";
    const RESOURCE: &str = "https://api.example.com";
    const OTHER_RESOURCE: &str = "https://mcp.example.com";

    fn jwt_with_ttl(ttl: Duration) -> JwtService {
        let mut opts = JwtOptions::new(SECRET);
        opts.expires_in = ttl;
        JwtService::new(opts).expect("jwt service")
    }

    fn verifier(resource: &str) -> JwtService {
        let mut opts = JwtOptions::new(SECRET);
        opts.audience = Some(resource.into());
        JwtService::new(opts).expect("verifier")
    }

    fn user_claims(jwt_svc: &JwtService, roles: Vec<Role>, scopes: Vec<String>) -> Claims {
        Claims {
            sub: Subject::User(Uuid::now_v7()),
            client_id: None,
            org_id: Uuid::now_v7(),
            roles,
            scopes,
            exp: jwt_svc.expiry(),
        }
    }

    #[test]
    fn issue_returns_a_bearer_token_envelope_with_the_jwt_ttl() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(900));
        let claims = user_claims(&jwt_svc, vec![Role::User], constants::all());
        let token = issue_with_jwt(&jwt_svc, RESOURCE, claims).expect("issue");

        assert_eq!(
            token.token_type, "Bearer",
            "token_type must be the RFC 6750 wire constant",
        );
        assert_eq!(
            token.expires_in, 900,
            "expires_in must mirror the JWT TTL (seconds)",
        );
        assert!(
            !token.access_token.expose_secret().is_empty(),
            "access_token must be the signed JWT, not an empty placeholder",
        );
    }

    #[tokio::test]
    async fn issue_round_trips_user_subject_through_verify() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let issued = user_claims(&jwt_svc, vec![Role::Admin], constants::all());
        let token = issue_with_jwt(&jwt_svc, RESOURCE, issued.clone()).expect("issue");

        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.sub, issued.sub);
        assert_eq!(claims.client_id, None);
        assert_eq!(claims.org_id, issued.org_id);
        assert!(claims.is_admin());
    }

    #[tokio::test]
    async fn the_token_and_its_response_carry_the_scopes_granted() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let issued = user_claims(&jwt_svc, vec![Role::User], vec![POSTS_READ.to_owned()]);
        let token = issue_with_jwt(&jwt_svc, RESOURCE, issued).expect("issue");

        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.scopes, [POSTS_READ]);
        assert_eq!(token.scope, [POSTS_READ]);
    }

    #[test]
    fn issue_surfaces_signing_failure_as_server_error() {
        let verify_only_svc = JwtService::new(JwtOptions::eddsa_verify(test_ed25519_public_pem()))
            .expect("verify-only jwt");
        let claims = user_claims(&verify_only_svc, vec![Role::User], constants::all());
        let err = issue_with_jwt(&verify_only_svc, RESOURCE, claims)
            .expect_err("sign without key ⇒ Sign(_)");
        assert!(matches!(err, TokenError::Sign(_)));
        assert_eq!(err.to_string(), "server_error");
    }

    fn test_ed25519_public_pem() -> &'static str {
        "-----BEGIN PUBLIC KEY-----\n\
         MCowBQYDK2VwAyEAGb9ECWmEzf6FQbrBZ9w7lshQhqowtrbLDFw4rXAxZuE=\n\
         -----END PUBLIC KEY-----\n"
    }

    fn auth_client(roles: Vec<Role>, scopes: &[&str]) -> AuthenticatedClient {
        AuthenticatedClient::new(
            "ci",
            scopes.iter().copied(),
            [RESOURCE],
            ClientPayload {
                org_id: Uuid::now_v7(),
                roles,
            },
        )
    }

    #[test]
    fn grant_client_credentials_rejects_unknown_grant_type() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let err = grant_client_credentials_with_jwt(
            &jwt_svc,
            "password",
            None,
            &[],
            &auth_client(vec![Role::User], &[POSTS_READ]),
        )
        .expect_err("non-CC grant rejected");
        assert!(matches!(err, TokenError::UnsupportedGrant));
        assert_eq!(err.to_string(), "unsupported_grant_type");
    }

    #[test]
    fn grant_client_credentials_rejects_scope_outside_the_client_grant() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let err = grant_client_credentials_with_jwt(
            &jwt_svc,
            "client_credentials",
            Some(POSTS_WRITE),
            &[],
            &auth_client(vec![Role::User], &[POSTS_READ]),
        )
        .expect_err("scope mismatch rejected");
        assert!(matches!(err, TokenError::InvalidScope));
    }

    #[tokio::test]
    async fn grant_client_credentials_issues_the_requested_subset_with_the_clients_org_and_roles() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(vec![Role::Admin], &[POSTS_READ, POSTS_WRITE]);
        let token = grant_client_credentials_with_jwt(
            &jwt_svc,
            "client_credentials",
            Some(POSTS_READ),
            &[],
            &client,
        )
        .expect("happy path");
        assert_eq!(token.token_type, "Bearer");
        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.sub, Subject::Client("ci".into()));
        assert_eq!(claims.client_id.as_deref(), Some("ci"));
        assert_eq!(claims.org_id, client.payload.org_id);
        assert_eq!(claims.roles, [Role::Admin]);
        assert_eq!(claims.scopes, [POSTS_READ]);
    }

    #[tokio::test]
    async fn a_machine_grant_names_its_client_as_its_subject() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(vec![Role::User], &[POSTS_READ]);
        let token =
            grant_client_credentials_with_jwt(&jwt_svc, "client_credentials", None, &[], &client)
                .expect("issue");
        let claims: serde_json::Value = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims["sub"], "ci", "{claims}");
        assert_eq!(claims["client_id"], "ci", "{claims}");
    }

    #[tokio::test]
    async fn an_absent_scope_is_granted_the_registered_set_and_says_so() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(vec![Role::User], &[POSTS_READ]);
        let token =
            grant_client_credentials_with_jwt(&jwt_svc, "client_credentials", None, &[], &client)
                .expect("absent scope ok");
        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.scopes, [POSTS_READ]);
        assert_eq!(claims.roles, [Role::User]);
        assert_eq!(token.scope, [POSTS_READ]);
    }

    use nest_rs::oauth::server::RegisteredClient;
    use nest_rs::social::SocialRegistry;

    use crate::users::UsersService;

    fn oauth_service(ttl: Duration) -> OAuthService {
        let jwt_svc = Arc::new(jwt_with_ttl(ttl));
        let providers = Arc::new(SocialRegistry::default());
        let config = Arc::new(OAuthConfig {
            clients: vec![RegisteredClient::new(
                "ci",
                "s3cret",
                [POSTS_READ],
                [RESOURCE],
                ClientPayload {
                    org_id: Uuid::now_v7(),
                    roles: vec![Role::User],
                },
            )],
            sign_in_resources: vec![RESOURCE.into(), OTHER_RESOURCE.into()],
            default_org_id: Uuid::now_v7(),
        });
        OAuthService::new(jwt_svc, providers, Arc::new(UsersService), config)
    }

    #[test]
    fn a_first_party_login_is_delegated_every_scope() {
        let svc = oauth_service(Duration::from_secs(60));
        let token = svc
            .issue(None, Uuid::now_v7(), Uuid::now_v7(), vec![Role::User])
            .expect("issue");
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.scope, constants::all());
    }

    #[test]
    fn oauth_service_issue_method_delegates_to_the_helper() {
        let svc = oauth_service(Duration::from_secs(120));
        let sub = Uuid::now_v7();
        let org = Uuid::now_v7();
        let token = svc.issue(None, sub, org, vec![Role::Admin]).expect("issue");
        assert_eq!(token.expires_in, 120);
        assert!(!token.access_token.expose_secret().is_empty());
    }

    #[test]
    fn oauth_service_grant_client_credentials_rejects_non_cc_grant() {
        let svc = oauth_service(Duration::from_secs(60));
        let err = svc
            .grant_client_credentials("password", None, &[], &auth_client(vec![Role::User], &[]))
            .expect_err("password grant on CC endpoint ⇒ unsupported");
        assert!(matches!(err, TokenError::UnsupportedGrant));
    }

    #[test]
    fn oauth_service_grant_client_credentials_issues_with_the_authenticated_org() {
        let svc = oauth_service(Duration::from_secs(60));
        let client = auth_client(vec![Role::User], &[POSTS_READ]);
        let token = svc
            .grant_client_credentials("client_credentials", None, &[], &client)
            .expect("happy CC path");
        assert_eq!(token.token_type, "Bearer");
    }

    #[test]
    fn oauth_service_authorize_returns_none_for_an_unknown_provider() {
        let svc = oauth_service(Duration::from_secs(60));
        assert!(
            svc.authorize("github").is_none(),
            "an empty registry knows no provider ⇒ None ⇒ 404",
        );
    }

    #[test]
    fn oauth_service_authenticate_client_matches_a_registered_pair() {
        let svc = oauth_service(Duration::from_secs(60));
        let auth = svc
            .authenticate_client("ci", "s3cret")
            .expect("matching pair");
        assert_eq!(auth.scopes, [POSTS_READ]);
        assert_eq!(auth.payload.roles, [Role::User]);
    }

    #[test]
    fn oauth_service_authenticate_client_rejects_unknown_id() {
        let svc = oauth_service(Duration::from_secs(60));
        let err = svc
            .authenticate_client("ghost", "s3cret")
            .expect_err("unknown id");
        assert!(nest_rs::core::error_message(&err).contains("invalid client credentials"));
    }

    #[test]
    fn token_error_from_auth_maps_a_store_outage_to_server_error_not_invalid_credentials() {
        assert!(
            matches!(
                token_error_from_auth(AuthError::unavailable("store down", None)),
                TokenError::Server(_),
            ),
            "a store outage during login must be server_error (503), never invalid_grant (400)",
        );
        assert!(matches!(
            token_error_from_auth(AuthError::failed("invalid credentials")),
            TokenError::InvalidGrant,
        ));
    }

    #[tokio::test]
    async fn a_client_token_names_the_resource_it_was_requested_for() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let mut client = auth_client(vec![Role::User], &[POSTS_READ]);
        client.resources = vec![RESOURCE.into(), OTHER_RESOURCE.into()];

        let token = grant_client_credentials_with_jwt(
            &jwt_svc,
            "client_credentials",
            None,
            &[OTHER_RESOURCE.into()],
            &client,
        )
        .expect("issue");

        verifier(OTHER_RESOURCE)
            .verify::<Claims>(token.access_token.expose_secret())
            .await
            .expect("the resource requested accepts it");
        assert!(
            verifier(RESOURCE)
                .verify::<Claims>(token.access_token.expose_secret())
                .await
                .is_err(),
            "another resource the client is registered for refuses it"
        );
    }

    #[test]
    fn a_client_token_for_a_resource_outside_its_registration_is_refused() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let err = grant_client_credentials_with_jwt(
            &jwt_svc,
            "client_credentials",
            None,
            &[OTHER_RESOURCE.into()],
            &auth_client(vec![Role::User], &[POSTS_READ]),
        )
        .expect_err("unregistered resource");
        assert!(matches!(err, TokenError::InvalidTarget));
    }

    #[tokio::test]
    async fn a_sign_in_names_the_resource_it_asks_for_or_the_first_served() {
        let svc = oauth_service(Duration::from_secs(60));
        for (resource, audience) in [(None, RESOURCE), (Some(OTHER_RESOURCE), OTHER_RESOURCE)] {
            let token = svc
                .issue(resource, Uuid::now_v7(), Uuid::now_v7(), vec![Role::User])
                .expect("issue");
            verifier(audience)
                .verify::<Claims>(token.access_token.expose_secret())
                .await
                .expect("minted for the resource asked for");
        }
        assert!(matches!(
            svc.issue(
                Some("https://billing.example.com"),
                Uuid::now_v7(),
                Uuid::now_v7(),
                vec![Role::User]
            ),
            Err(TokenError::InvalidTarget)
        ));
    }
}
