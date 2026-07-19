use std::sync::Arc;

use nest_rs_authn::{
    AuthError, Authorization, JwtService, TokenError, authenticate_against_registry,
};
use nest_rs_core::injectable;
use nest_rs_social::{SocialProfile, SocialProviders};
use uuid::Uuid;

use super::config::IssuerConfig;
use super::dtos::AccessTokenDto;
use super::scope::{role_from_db, roles_for_scope};
use crate::users::{SocialIdentity, UsersService};
use crate::{Claims, Role};

#[derive(Debug, Clone)]
pub struct Caller {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

pub type AuthenticatedClient = nest_rs_authn::AuthenticatedClient<Uuid>;

impl nest_rs_authn::PrincipalIdentity for Caller {
    fn actor_id(&self) -> Option<String> {
        Some(self.user_id.to_string())
    }
}

#[injectable]
pub struct OAuthService {
    #[inject]
    jwt_svc: Arc<JwtService>,
    #[inject]
    providers: Arc<SocialProviders>,
    #[inject]
    users_svc: Arc<UsersService>,
    #[inject]
    config: Arc<IssuerConfig>,
}

impl OAuthService {
    pub fn new(
        jwt_svc: Arc<JwtService>,
        providers: Arc<SocialProviders>,
        users_svc: Arc<UsersService>,
        config: Arc<IssuerConfig>,
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
        sub: Option<Uuid>,
        org_id: Uuid,
        roles: Vec<Role>,
    ) -> Result<AccessTokenDto, TokenError> {
        issue_with_jwt(&self.jwt_svc, sub, org_id, roles)
    }

    pub async fn grant_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AccessTokenDto, TokenError> {
        let user = self
            .users_svc
            .authenticate(email, password)
            .await
            .map_err(token_error_from_auth)?;
        let roles = vec![role_from_db(&user.role)];
        self.issue(Some(user.id), user.org_id, roles)
    }

    pub fn grant_client_credentials(
        &self,
        grant_type: &str,
        scope: Option<&str>,
        client: &AuthenticatedClient,
    ) -> Result<AccessTokenDto, TokenError> {
        grant_client_credentials_with_jwt(&self.jwt_svc, grant_type, scope, client)
    }

    pub fn authorize(&self, provider: &str) -> Option<Result<Authorization, AuthError>> {
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
            .ok_or_else(|| AuthError::Failed("unknown social provider".into()))?;

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
            roles: vec![role_from_db(&user.role)],
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
    sub: Option<Uuid>,
    org_id: Uuid,
    roles: Vec<Role>,
) -> Result<AccessTokenDto, TokenError> {
    let claims = Claims {
        sub,
        org_id,
        roles: roles.clone(),
        exp: jwt_svc.expiry(),
    };
    let access_token = jwt_svc
        .sign(&claims)
        .map_err(|e| TokenError::Sign(e.into()))?;
    tracing::debug!(
        target: "features::oauth",
        ?sub,
        %org_id,
        roles = ?claims.roles,
        "issued access token"
    );
    Ok(AccessTokenDto {
        access_token,
        token_type: "Bearer".into(),
        expires_in: jwt_svc.ttl_secs(),
    })
}

pub(crate) fn grant_client_credentials_with_jwt(
    jwt_svc: &JwtService,
    grant_type: &str,
    scope: Option<&str>,
    client: &AuthenticatedClient,
) -> Result<AccessTokenDto, TokenError> {
    if grant_type != "client_credentials" {
        tracing::warn!(target: "features::oauth", grant_type, "unsupported grant type");
        return Err(TokenError::UnsupportedGrant);
    }
    let roles = roles_for_scope(scope, &client.scopes).ok_or_else(|| {
        tracing::warn!(
            target: "features::oauth",
            requested_scope = ?scope,
            allowed = ?client.scopes,
            "requested scope not granted"
        );
        TokenError::InvalidScope
    })?;
    issue_with_jwt(jwt_svc, None, client.payload, roles)
}

fn token_error_from_auth(err: AuthError) -> TokenError {
    if matches!(err, AuthError::Unavailable(_)) {
        TokenError::Server(err.into())
    } else {
        TokenError::InvalidCredentials
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nest_rs_authn::{JwtOptions, JwtService};
    use std::time::Duration;

    fn jwt_with_ttl(ttl: Duration) -> JwtService {
        let mut opts = JwtOptions::new("test-secret");
        opts.expires_in = ttl;
        JwtService::new(opts).expect("jwt service")
    }

    #[test]
    fn issue_returns_a_bearer_token_envelope_with_the_jwt_ttl() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(900));
        let org = Uuid::now_v7();
        let sub = Uuid::now_v7();
        let token = issue_with_jwt(&jwt_svc, Some(sub), org, vec![Role::User]).expect("issue");

        assert_eq!(
            token.token_type, "Bearer",
            "token_type must be the RFC 6750 wire constant",
        );
        assert_eq!(
            token.expires_in, 900,
            "expires_in must mirror the JWT TTL (seconds)",
        );
        assert!(
            !token.access_token.is_empty(),
            "access_token must be the signed JWT, not an empty placeholder",
        );
    }

    #[test]
    fn issue_round_trips_user_subject_through_verify() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let sub = Uuid::now_v7();
        let org = Uuid::now_v7();
        let token = issue_with_jwt(&jwt_svc, Some(sub), org, vec![Role::Admin]).expect("issue");

        let claims: Claims = jwt_svc.verify(&token.access_token).expect("verify");
        assert_eq!(claims.sub, Some(sub));
        assert_eq!(claims.org_id, org);
        assert!(claims.is_admin());
    }

    #[test]
    fn issue_machine_grant_signs_with_no_subject() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let org = Uuid::now_v7();
        let token = issue_with_jwt(&jwt_svc, None, org, vec![Role::User]).expect("issue");
        let claims: Claims = jwt_svc.verify(&token.access_token).expect("verify");
        assert!(claims.sub.is_none(), "machine grant must omit sub");
        assert_eq!(claims.org_id, org);
    }

    #[test]
    fn issue_surfaces_signing_failure_as_server_error() {
        let verify_only_svc = JwtService::new(JwtOptions::eddsa_verify(test_ed25519_public_pem()))
            .expect("verify-only jwt");
        let err = issue_with_jwt(
            &verify_only_svc,
            Some(Uuid::now_v7()),
            Uuid::now_v7(),
            vec![Role::User],
        )
        .expect_err("sign without key ⇒ Sign(_)");
        assert!(matches!(err, TokenError::Sign(_)));
        assert_eq!(err.to_string(), "server_error");
    }

    fn test_ed25519_public_pem() -> &'static str {
        "-----BEGIN PUBLIC KEY-----\n\
         MCowBQYDK2VwAyEAGb9ECWmEzf6FQbrBZ9w7lshQhqowtrbLDFw4rXAxZuE=\n\
         -----END PUBLIC KEY-----\n"
    }

    fn auth_client(scopes: &[&str]) -> AuthenticatedClient {
        AuthenticatedClient {
            payload: Uuid::now_v7(),
            scopes: scopes.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn grant_client_credentials_rejects_unknown_grant_type() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let err =
            grant_client_credentials_with_jwt(&jwt_svc, "password", None, &auth_client(&["user"]))
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
            Some("admin"),
            &auth_client(&["user"]),
        )
        .expect_err("scope mismatch rejected");
        assert!(matches!(err, TokenError::InvalidScope));
    }

    #[test]
    fn grant_client_credentials_issues_a_bearer_token_with_the_clients_org() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(&["user", "admin"]);
        let org = client.payload;
        let token = grant_client_credentials_with_jwt(
            &jwt_svc,
            "client_credentials",
            Some("user"),
            &client,
        )
        .expect("happy path");
        assert_eq!(token.token_type, "Bearer");
        let claims: Claims = jwt_svc.verify(&token.access_token).expect("verify");
        assert!(
            claims.sub.is_none(),
            "client_credentials token must carry no sub",
        );
        assert_eq!(claims.org_id, org);
    }

    use nest_rs_authn::RegisteredClient;
    use nest_rs_social::SocialProviders;
    use sea_orm::DatabaseConnection;

    use crate::users::UsersService;

    fn users_service_disconnected() -> Arc<UsersService> {
        Arc::new(UsersService::new(Arc::new(DatabaseConnection::default())))
    }

    fn oauth_service(ttl: Duration) -> OAuthService {
        let jwt_svc = Arc::new(jwt_with_ttl(ttl));
        let providers = Arc::new(SocialProviders::default());
        let config = Arc::new(IssuerConfig {
            clients: vec![RegisteredClient {
                client_id: "ci".into(),
                client_secret: "s3cret".into(),
                scopes: vec!["user".into()],
                payload: Uuid::now_v7(),
            }],
            default_org_id: Uuid::now_v7(),
        });
        OAuthService::new(jwt_svc, providers, users_service_disconnected(), config)
    }

    #[test]
    fn oauth_service_issue_builds_a_usable_token() {
        let svc = oauth_service(Duration::from_secs(60));
        let token = svc
            .issue(Some(Uuid::now_v7()), Uuid::now_v7(), vec![Role::User])
            .expect("issue");
        assert_eq!(token.token_type, "Bearer");
    }

    #[test]
    fn oauth_service_issue_method_delegates_to_the_helper() {
        let svc = oauth_service(Duration::from_secs(120));
        let sub = Uuid::now_v7();
        let org = Uuid::now_v7();
        let token = svc.issue(Some(sub), org, vec![Role::Admin]).expect("issue");
        assert_eq!(token.expires_in, 120);
        assert!(!token.access_token.is_empty());
    }

    #[test]
    fn oauth_service_grant_client_credentials_rejects_non_cc_grant() {
        let svc = oauth_service(Duration::from_secs(60));
        let err = svc
            .grant_client_credentials("password", None, &auth_client(&["user"]))
            .expect_err("password grant on CC endpoint ⇒ unsupported");
        assert!(matches!(err, TokenError::UnsupportedGrant));
    }

    #[test]
    fn oauth_service_grant_client_credentials_issues_with_the_authenticated_org() {
        let svc = oauth_service(Duration::from_secs(60));
        let client = auth_client(&["user"]);
        let token = svc
            .grant_client_credentials("client_credentials", None, &client)
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
        assert_eq!(auth.scopes, vec!["user".to_string()]);
    }

    #[test]
    fn oauth_service_authenticate_client_rejects_unknown_id() {
        let svc = oauth_service(Duration::from_secs(60));
        let err = svc
            .authenticate_client("ghost", "s3cret")
            .expect_err("unknown id");
        assert!(err.to_string().contains("invalid client credentials"));
    }

    #[test]
    fn token_error_from_auth_maps_a_store_outage_to_server_error_not_invalid_credentials() {
        assert!(
            matches!(
                token_error_from_auth(AuthError::Unavailable("store down".into())),
                TokenError::Server(_),
            ),
            "a store outage during login must be server_error (500), never invalid_credentials (401)",
        );
        assert!(matches!(
            token_error_from_auth(AuthError::Failed("invalid credentials".into())),
            TokenError::InvalidCredentials,
        ));
    }

    #[test]
    fn grant_client_credentials_falls_back_to_the_full_grant_when_scope_blank() {
        let jwt_svc = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(&["admin"]);
        let token =
            grant_client_credentials_with_jwt(&jwt_svc, "client_credentials", None, &client)
                .expect("blank scope ok");
        let claims: Claims = jwt_svc.verify(&token.access_token).expect("verify");
        assert!(claims.is_admin(), "blank scope should grant the full set");
    }
}
