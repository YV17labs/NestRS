use std::sync::Arc;

use nest_rs::authn::{AuthError, JwtService, PrincipalIdentity};
use nest_rs::config::{ConfigError, Namespaced};
use nest_rs::core::{async_trait, hooks, injectable};
use nest_rs::oauth::client::TokenSet;
use nest_rs::oauth::server::{
    AccessTokenRequest, AccessTokenResponse, OAuthServerConfig, TokenError,
};
use nest_rs::social::{SocialProfile, SocialRedirect, SocialRegistry, SocialSignIn};
use uuid::Uuid;

use super::config::{ClientPayload, SignInConfig, registered_within_policy};
use crate::authz::constants;
use crate::users::{SocialIdentity, UserRole, UsersService};
use crate::{Claims, Role, Subject};

#[derive(Debug, Clone, PrincipalIdentity)]
pub struct Caller {
    #[principal(actor)]
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
    pub resource: Option<String>,
}

pub type AuthenticatedClient = nest_rs::oauth::server::AuthenticatedClient<ClientPayload>;

#[injectable]
pub struct SignInService {
    #[inject]
    jwt_svc: Arc<JwtService>,
    #[inject]
    providers: Arc<SocialRegistry>,
    #[inject]
    users_svc: Arc<UsersService>,
    #[inject]
    config: Arc<SignInConfig>,
    #[inject]
    clients: Arc<OAuthServerConfig<ClientPayload>>,
}

#[hooks]
impl SignInService {
    #[on_module_init]
    async fn refuse_clients_outside_the_policy(&self) -> nest_rs::core::anyhow::Result<()> {
        registered_within_policy(&self.clients, &self.config).map_err(|errors| {
            ConfigError::validation(OAuthServerConfig::<ClientPayload>::NAMESPACE, errors).into()
        })
    }
}

impl SignInService {
    pub fn new(
        jwt_svc: Arc<JwtService>,
        providers: Arc<SocialRegistry>,
        users_svc: Arc<UsersService>,
        config: Arc<SignInConfig>,
        clients: Arc<OAuthServerConfig<ClientPayload>>,
    ) -> Self {
        Self {
            jwt_svc,
            providers,
            users_svc,
            config,
            clients,
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
                client_id: self.config.client_id.clone(),
                org_id,
                roles,
                scopes: constants::all(),
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
        request: &AccessTokenRequest,
        client: &AuthenticatedClient,
    ) -> Result<AccessTokenResponse, TokenError> {
        grant_client_credentials_with_jwt(&self.jwt_svc, &request.scope, &request.resource, client)
    }

    pub fn authorize(
        &self,
        provider: &str,
        resource: Option<&str>,
    ) -> Result<SocialRedirect, TokenError> {
        self.sign_in_audience(resource)?;
        Ok(self
            .providers
            .authorize(provider, &self.jwt_svc, resource)?)
    }

    fn sign_in_audience(&self, resource: Option<&str>) -> Result<&str, TokenError> {
        let resources = &self.config.resources;
        match resource {
            None => resources.first(),
            Some(requested) => resources.iter().find(|served| *served == requested),
        }
        .map(String::as_str)
        .ok_or_else(|| {
            tracing::warn!(
                target: crate::sign_in::TARGET,
                reason = "invalid_target",
                "sign-in refused"
            );
            TokenError::InvalidTarget
        })
    }
}

#[async_trait]
impl SocialSignIn for SignInService {
    type Principal = Caller;

    async fn sign_in(
        &self,
        profile: SocialProfile,
        _tokens: TokenSet,
        resource: Option<&str>,
    ) -> Result<Caller, AuthError> {
        let user = self
            .users_svc
            .resolve_social_identity(&social_identity(profile), self.config.default_org_id)
            .await?;
        Ok(Caller {
            user_id: user.id,
            org_id: user.org_id,
            roles: vec![role_from_db(user.role)],
            resource: resource.map(str::to_owned),
        })
    }
}

fn role_from_db(role: UserRole) -> Role {
    match role {
        UserRole::Admin => Role::Admin,
        UserRole::User => Role::User,
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
    let access_token = jwt_svc.sign_for(audience, &claims)?;
    tracing::debug!(
        target: crate::sign_in::TARGET,
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
    scope: &[String],
    resource: &[String],
    client: &AuthenticatedClient,
) -> Result<AccessTokenResponse, TokenError> {
    let scopes = client.scope_for(scope)?;
    let audience = client.audience_for(resource)?;
    issue_with_jwt(
        jwt_svc,
        audience,
        Claims {
            sub: Subject::Client(client.client_id.clone()),
            client_id: client.client_id.clone(),
            org_id: client.payload.org_id,
            roles: client.payload.roles.clone(),
            scopes,
        },
    )
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
    use nest_rs::http::{IntoResponse, StatusCode};
    use std::time::Duration;

    use crate::authz::constants::{POSTS_READ, POSTS_WRITE};

    const SECRET: &str = "test-secret-padded-to-thirty-two-b";
    const ISSUER: &str = "https://auth.example.com";
    const RESOURCE: &str = "https://api.example.com";
    const OTHER_RESOURCE: &str = "https://mcp.example.com";

    fn named(mut opts: JwtOptions) -> JwtOptions {
        opts.issuer = Some(ISSUER.into());
        opts.audience = Some(ISSUER.into());
        opts
    }

    fn jwt_with_ttl(ttl: Duration) -> JwtService {
        let mut opts = named(JwtOptions::new(SECRET));
        opts.expires_in = ttl;
        JwtService::new(opts).expect("jwt service")
    }

    fn verifier(resource: &str) -> JwtService {
        let mut opts = named(JwtOptions::new(SECRET));
        opts.audience = Some(resource.into());
        JwtService::new(opts).expect("verifier")
    }

    fn user_claims(roles: Vec<Role>, scopes: Vec<String>) -> Claims {
        Claims {
            sub: Subject::User(Uuid::now_v7()),
            client_id: SignInConfig::default().client_id,
            org_id: Uuid::now_v7(),
            roles,
            scopes,
        }
    }

    #[test]
    fn role_from_db_maps_each_role() {
        assert!(matches!(role_from_db(UserRole::Admin), Role::Admin));
        assert!(matches!(role_from_db(UserRole::User), Role::User));
    }

    #[test]
    fn issue_returns_a_bearer_token_envelope_with_the_jwt_ttl() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(900));
        let claims = user_claims(vec![Role::User], constants::all());
        let token = issue_with_jwt(&jwt_svc, RESOURCE, claims).expect("issue");

        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.expires_in, 900);
        assert!(!token.access_token.expose_secret().is_empty());
    }

    #[tokio::test]
    async fn issue_round_trips_user_subject_through_verify() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let issued = user_claims(vec![Role::Admin], constants::all());
        let token = issue_with_jwt(&jwt_svc, RESOURCE, issued.clone()).expect("issue");

        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.sub, issued.sub);
        assert_eq!(claims.client_id, issued.client_id);
        assert_eq!(claims.org_id, issued.org_id);
        assert!(claims.is_admin());
    }

    #[tokio::test]
    async fn the_token_and_its_response_carry_the_scopes_granted() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let issued = user_claims(vec![Role::User], vec![POSTS_READ.into()]);
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
        let verify_only_svc =
            JwtService::new(named(JwtOptions::eddsa_verify(test_ed25519_public_pem())))
                .expect("verify-only jwt");
        let claims = user_claims(vec![Role::User], constants::all());
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

    #[tokio::test]
    async fn a_machine_grant_issues_the_requested_subset_as_its_client() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(
            vec![Role::Admin],
            &[POSTS_READ.as_str(), POSTS_WRITE.as_str()],
        );
        let token = grant_client_credentials_with_jwt(&jwt_svc, &[POSTS_READ.into()], &[], &client)
            .expect("issue");

        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.sub, Subject::Client("ci".into()));
        assert_eq!(claims.client_id, "ci");
        assert_eq!(claims.org_id, client.payload.org_id);
        assert_eq!(claims.roles, [Role::Admin]);
        assert_eq!(claims.scopes, [POSTS_READ]);
        assert_eq!(token.scope, [POSTS_READ]);
    }

    fn sign_in_service(ttl: Duration) -> SignInService {
        SignInService::new(
            Arc::new(jwt_with_ttl(ttl)),
            Arc::new(SocialRegistry::default()),
            Arc::new(UsersService),
            Arc::new(SignInConfig {
                resources: vec![RESOURCE.into(), OTHER_RESOURCE.into()],
                default_org_id: Uuid::now_v7(),
                ..SignInConfig::default()
            }),
            Arc::new(OAuthServerConfig::default()),
        )
    }

    #[tokio::test]
    async fn a_first_party_login_names_its_own_client() {
        let token = sign_in_service(Duration::from_secs(60))
            .issue(None, Uuid::now_v7(), Uuid::now_v7(), vec![Role::User])
            .expect("issue");

        let claims: Claims = verifier(RESOURCE)
            .verify(token.access_token.expose_secret())
            .await
            .expect("verify");
        assert_eq!(claims.client_id, SignInConfig::default().client_id);
    }

    #[test]
    fn a_first_party_login_is_delegated_every_scope() {
        let token = sign_in_service(Duration::from_secs(120))
            .issue(None, Uuid::now_v7(), Uuid::now_v7(), vec![Role::User])
            .expect("issue");
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.expires_in, 120);
        assert_eq!(token.scope, constants::all());
    }

    #[test]
    fn an_unknown_provider_is_not_found_and_an_unserved_resource_refused_before_it() {
        let svc = sign_in_service(Duration::from_secs(60));
        let unknown = svc.authorize("github", None).expect("answered");
        assert_eq!(unknown.into_response().status(), StatusCode::NOT_FOUND);
        assert!(matches!(
            svc.authorize("github", Some("https://billing.example.com")),
            Err(TokenError::InvalidTarget)
        ));
    }

    #[test]
    fn token_error_from_auth_maps_a_store_outage_to_server_error_not_invalid_credentials() {
        assert!(matches!(
            token_error_from_auth(AuthError::unavailable("store down", None)),
            TokenError::Server(_),
        ));
        assert!(matches!(
            token_error_from_auth(AuthError::failed("invalid credentials")),
            TokenError::InvalidGrant,
        ));
    }

    #[tokio::test]
    async fn a_sign_in_names_the_resource_it_asks_for_or_the_first_served() {
        let svc = sign_in_service(Duration::from_secs(60));
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
