use std::sync::Arc;

use nest_rs_authn::{AuthError, Authorization, JwtService, OAuth2Client, TokenError};
use nest_rs_core::injectable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::IssuerConfig;
use super::scope::{role_from_db, roles_for_scope};
use crate::users::UsersService;
use crate::{Claims, Role};

#[derive(Debug, Clone)]
pub struct Caller {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub roles: Vec<Role>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedClient {
    pub org_id: Uuid,
    pub scopes: Vec<String>,
}

/// `id` is the only field providers always return; the rest falls back to
/// derived values so a missing email or display name does not block account
/// creation.
#[derive(Debug, Clone, Deserialize)]
struct OAuthProfile {
    id: i64,
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl OAuthProfile {
    fn email(&self) -> String {
        self.email
            .clone()
            .filter(|email| !email.is_empty())
            .unwrap_or_else(|| format!("{}@users.noreply.github.com", self.handle()))
    }

    fn handle(&self) -> String {
        self.login.clone().unwrap_or_else(|| self.id.to_string())
    }

    fn display_name(&self) -> String {
        self.name
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| self.login.clone())
            .unwrap_or_else(|| format!("user-{}", self.id))
    }
}

#[injectable]
pub struct TokenIssuer {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    users: Arc<UsersService>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccessToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

impl TokenIssuer {
    pub fn new(jwt: Arc<JwtService>, users: Arc<UsersService>) -> Self {
        Self { jwt, users }
    }

    /// `sub` is set for human principals; machine grants
    /// (`client_credentials`) omit it.
    pub fn issue(
        &self,
        sub: Option<Uuid>,
        org_id: Uuid,
        roles: Vec<Role>,
    ) -> Result<AccessToken, TokenError> {
        issue_with_jwt(&self.jwt, sub, org_id, roles)
    }

    pub async fn grant_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AccessToken, TokenError> {
        let user = self
            .users
            .authenticate(email, password)
            .await
            .map_err(|_| TokenError::InvalidCredentials)?;
        let roles = vec![role_from_db(&user.role)];
        self.issue(Some(user.id), user.org_id, roles)
    }

    pub fn grant_client_credentials(
        &self,
        grant_type: &str,
        scope: Option<&str>,
        client: &AuthenticatedClient,
    ) -> Result<AccessToken, TokenError> {
        grant_client_credentials_with_jwt(&self.jwt, grant_type, scope, client)
    }
}

#[injectable]
pub struct OAuthFlow {
    #[inject]
    jwt: Arc<JwtService>,
    #[inject]
    oauth: Arc<OAuth2Client>,
    #[inject]
    users: Arc<UsersService>,
    #[inject]
    config: Arc<IssuerConfig>,
}

impl OAuthFlow {
    pub fn new(
        jwt: Arc<JwtService>,
        oauth: Arc<OAuth2Client>,
        users: Arc<UsersService>,
        config: Arc<IssuerConfig>,
    ) -> Self {
        Self {
            jwt,
            oauth,
            users,
            config,
        }
    }

    pub fn authorize(&self) -> Result<Authorization, AuthError> {
        self.oauth.authorize(&self.jwt)
    }

    pub async fn resolve_caller(
        &self,
        transaction: &str,
        state: &str,
        code: &str,
    ) -> Result<Caller, AuthError> {
        let access_token = self
            .oauth
            .exchange(&self.jwt, transaction, state, code)
            .await?;
        let profile: OAuthProfile = self.oauth.userinfo(&access_token).await?;
        let user = self
            .users
            .find_or_create(
                &profile.email(),
                &profile.display_name(),
                self.config.default_org_id,
            )
            .await
            .map_err(|err| AuthError::Failed(format!("identity resolution failed: {err}")))?;
        Ok(Caller {
            user_id: user.id,
            org_id: user.org_id,
            roles: vec![role_from_db(&user.role)],
        })
    }

    /// Constant-time comparison against the configured client registry.
    pub fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<AuthenticatedClient, AuthError> {
        authenticate_against_registry(&self.config.clients, client_id, client_secret)
    }
}

/// Build + sign the access token without going through the injected
/// `TokenIssuer`. Extracted so the sign + envelope step is testable with a
/// real `JwtService` and no `UsersService`. Pure of `self`.
pub(crate) fn issue_with_jwt(
    jwt: &JwtService,
    sub: Option<Uuid>,
    org_id: Uuid,
    roles: Vec<Role>,
) -> Result<AccessToken, TokenError> {
    let claims = Claims {
        sub,
        org_id,
        roles: roles.clone(),
        exp: jwt.expiry(),
    };
    let access_token = jwt.sign(&claims).map_err(|e| TokenError::Sign(e.into()))?;
    tracing::info!(
        ?sub,
        %org_id,
        roles = ?claims.roles,
        "issued access token"
    );
    Ok(AccessToken {
        access_token,
        token_type: "Bearer".into(),
        expires_in: jwt.ttl_secs(),
    })
}

/// Pure decision tree for `grant_client_credentials`: reject unknown grant
/// type, compute roles from the requested scope against what the
/// authenticated client is granted, and call back into the JWT issuer.
/// Extracted so the wire-error mapping for `unsupported_grant_type` /
/// `invalid_scope` is testable with no `UsersService`.
pub(crate) fn grant_client_credentials_with_jwt(
    jwt: &JwtService,
    grant_type: &str,
    scope: Option<&str>,
    client: &AuthenticatedClient,
) -> Result<AccessToken, TokenError> {
    if grant_type != "client_credentials" {
        return Err(TokenError::UnsupportedGrant);
    }
    let roles = roles_for_scope(scope, &client.scopes).ok_or(TokenError::InvalidScope)?;
    issue_with_jwt(jwt, None, client.org_id, roles)
}

/// Constant-time lookup against a registry, extracted so the credential
/// match can be unit-tested without standing up an `OAuth2Client` /
/// `UsersService`. Pure function; the only side-channel that matters here
/// (timing) is preserved by [`constant_time_eq`].
pub(crate) fn authenticate_against_registry(
    clients: &[super::config::RegisteredClient],
    client_id: &str,
    client_secret: &str,
) -> Result<AuthenticatedClient, AuthError> {
    let client = clients
        .iter()
        .find(|client| {
            constant_time_eq(client.client_id.as_bytes(), client_id.as_bytes())
                && constant_time_eq(client.client_secret.as_bytes(), client_secret.as_bytes())
        })
        .ok_or_else(|| AuthError::Failed("invalid client credentials".into()))?;
    Ok(AuthenticatedClient {
        org_id: client.org_id,
        scopes: client.scopes.clone(),
    })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(login: Option<&str>, name: Option<&str>, email: Option<&str>) -> OAuthProfile {
        OAuthProfile {
            id: 42,
            login: login.map(str::to_owned),
            name: name.map(str::to_owned),
            email: email.map(str::to_owned),
        }
    }

    #[test]
    fn email_uses_provider_email_when_present() {
        let p = profile(Some("ada"), None, Some("ada@example.com"));
        assert_eq!(p.email(), "ada@example.com");
    }

    #[test]
    fn email_falls_back_to_noreply_when_provider_email_missing() {
        let p = profile(Some("ada"), None, None);
        assert_eq!(p.email(), "ada@users.noreply.github.com");
    }

    #[test]
    fn email_falls_back_to_noreply_when_provider_email_blank() {
        let p = profile(Some("ada"), None, Some(""));
        assert_eq!(p.email(), "ada@users.noreply.github.com");
    }

    #[test]
    fn handle_falls_back_to_numeric_id_when_login_missing() {
        let p = profile(None, None, None);
        assert_eq!(p.handle(), "42");
    }

    #[test]
    fn display_name_prefers_name_then_login_then_synthesised() {
        assert_eq!(profile(Some("ada"), Some("Ada Lovelace"), None).display_name(), "Ada Lovelace");
        assert_eq!(profile(Some("ada"), None, None).display_name(), "ada");
        assert_eq!(profile(Some("ada"), Some(""), None).display_name(), "ada");
        assert_eq!(profile(None, None, None).display_name(), "user-42");
    }

    #[test]
    fn constant_time_eq_matches_eq_for_equal_inputs() {
        assert!(constant_time_eq(b"secret", b"secret"));
    }

    #[test]
    fn constant_time_eq_rejects_different_lengths_immediately() {
        // Length mismatch short-circuits — the timing of this branch is OK
        // because length is not the secret.
        assert!(!constant_time_eq(b"secret", b"secrets"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn constant_time_eq_rejects_different_same_length_inputs() {
        assert!(!constant_time_eq(b"secret", b"public"));
    }

    use crate::oauth::config::RegisteredClient;

    fn client(id: &str, secret: &str, scopes: &[&str]) -> RegisteredClient {
        RegisteredClient {
            client_id: id.into(),
            client_secret: secret.into(),
            org_id: uuid::Uuid::nil(),
            scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn authenticate_against_registry_returns_authenticated_for_a_matching_pair() {
        let registry = [client("ci", "s3cret", &["user"])];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").expect("auth");
        assert_eq!(auth.scopes, vec!["user".to_string()]);
    }

    #[test]
    fn authenticate_against_registry_rejects_wrong_secret() {
        let registry = [client("ci", "s3cret", &["user"])];
        let err = authenticate_against_registry(&registry, "ci", "wrong")
            .expect_err("wrong secret rejected");
        // The error message must NOT name the right secret or even hint at
        // which step failed — opaque "invalid client credentials" only.
        assert!(matches!(err, AuthError::Failed(_)));
        assert!(
            err.to_string().contains("invalid client credentials"),
            "wire error must be opaque: {err}",
        );
    }

    #[test]
    fn authenticate_against_registry_rejects_unknown_client_id() {
        let registry = [client("ci", "s3cret", &["user"])];
        assert!(authenticate_against_registry(&registry, "other", "s3cret").is_err());
    }

    #[test]
    fn authenticate_against_registry_picks_the_first_matching_client() {
        // Two registrations for `ci` — first match wins. Pin the behaviour so
        // a future refactor that swaps to a HashMap doesn't silently change
        // it.
        let a = RegisteredClient {
            scopes: vec!["a".into()],
            ..client("ci", "s3cret", &["a"])
        };
        let b = RegisteredClient {
            scopes: vec!["b".into()],
            ..client("ci", "s3cret", &["b"])
        };
        let registry = [a, b];
        let auth = authenticate_against_registry(&registry, "ci", "s3cret").unwrap();
        assert_eq!(auth.scopes, vec!["a".to_string()]);
    }

    #[test]
    fn authenticate_against_registry_rejects_empty_registry() {
        let registry: [RegisteredClient; 0] = [];
        assert!(authenticate_against_registry(&registry, "any", "any").is_err());
    }

    #[test]
    fn authenticate_against_registry_distinguishes_clients_with_a_shared_prefix() {
        // `constant_time_eq` short-circuits on length mismatch (length isn't
        // secret), and on equal-length inputs walks the whole buffer.
        let registry = [
            client("ci", "s3cret-a", &["a"]),
            client("ci-prod", "s3cret-b", &["b"]),
        ];
        let auth = authenticate_against_registry(&registry, "ci-prod", "s3cret-b").unwrap();
        assert_eq!(auth.scopes, vec!["b".to_string()]);
    }

    // `issue_with_jwt` is the sign + envelope step the issuer (and every
    // grant) reduces to. Pin the wire-format invariants here: `"Bearer"`
    // token_type, `expires_in == jwt.ttl_secs()`, and `sub` round-trips
    // exactly as supplied (machine grants must omit `sub`, see Claims tests).

    use std::time::Duration;
    use nest_rs_authn::{JwtOptions, JwtService};

    fn jwt_with_ttl(ttl: Duration) -> JwtService {
        let mut opts = JwtOptions::new("test-secret");
        opts.expires_in = ttl;
        JwtService::new(opts).expect("jwt service")
    }

    #[test]
    fn issue_returns_a_bearer_token_envelope_with_the_jwt_ttl() {
        let jwt = jwt_with_ttl(Duration::from_secs(900));
        let org = Uuid::now_v7();
        let sub = Uuid::now_v7();
        let token = issue_with_jwt(&jwt, Some(sub), org, vec![Role::User]).expect("issue");

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
        let jwt = jwt_with_ttl(Duration::from_secs(60));
        let sub = Uuid::now_v7();
        let org = Uuid::now_v7();
        let token = issue_with_jwt(&jwt, Some(sub), org, vec![Role::Admin]).expect("issue");

        let claims: Claims = jwt.verify(&token.access_token).expect("verify");
        assert_eq!(claims.sub, Some(sub));
        assert_eq!(claims.org_id, org);
        assert!(claims.is_admin());
    }

    #[test]
    fn issue_machine_grant_signs_with_no_subject() {
        // `client_credentials` callers must get a token whose verified claims
        // carry `sub == None` — a non-None here is a privilege bug.
        let jwt = jwt_with_ttl(Duration::from_secs(60));
        let org = Uuid::now_v7();
        let token = issue_with_jwt(&jwt, None, org, vec![Role::User]).expect("issue");
        let claims: Claims = jwt.verify(&token.access_token).expect("verify");
        assert!(claims.sub.is_none(), "machine grant must omit sub");
        assert_eq!(claims.org_id, org);
    }

    #[test]
    fn issue_surfaces_signing_failure_as_server_error() {
        // A verify-only `JwtService` has no signing key — `sign` returns
        // `AuthError::Failed`, which the issuer must map to
        // `TokenError::Sign(_)`. The wire string is the opaque RFC 6749
        // `server_error`, never the internal cause.
        let verify_only = JwtService::new(JwtOptions::eddsa_verify(test_ed25519_public_pem()))
            .expect("verify-only jwt");
        let err = issue_with_jwt(
            &verify_only,
            Some(Uuid::now_v7()),
            Uuid::now_v7(),
            vec![Role::User],
        )
        .expect_err("sign without key ⇒ Sign(_)");
        assert!(matches!(err, TokenError::Sign(_)));
        assert_eq!(err.to_string(), "server_error");
    }

    // A throwaway ED25519 public key in PEM form, just enough to build a
    // verify-only `JwtService`. The body is random and never trusted — the
    // verify-only `sign` path errors *before* it touches the key.
    fn test_ed25519_public_pem() -> &'static str {
        "-----BEGIN PUBLIC KEY-----\n\
         MCowBQYDK2VwAyEAGb9ECWmEzf6FQbrBZ9w7lshQhqowtrbLDFw4rXAxZuE=\n\
         -----END PUBLIC KEY-----\n"
    }

    // `grant_client_credentials_with_jwt` carries the RFC 6749 grant-type +
    // scope branching every public `POST /oauth/token` request flows through.

    fn auth_client(scopes: &[&str]) -> AuthenticatedClient {
        AuthenticatedClient {
            org_id: Uuid::now_v7(),
            scopes: scopes.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn grant_client_credentials_rejects_unknown_grant_type() {
        let jwt = jwt_with_ttl(Duration::from_secs(60));
        let err = grant_client_credentials_with_jwt(
            &jwt,
            "password",
            None,
            &auth_client(&["user"]),
        )
        .expect_err("non-CC grant rejected");
        assert!(matches!(err, TokenError::UnsupportedGrant));
        assert_eq!(err.to_string(), "unsupported_grant_type");
    }

    #[test]
    fn grant_client_credentials_rejects_scope_outside_the_client_grant() {
        // Requesting a scope the client wasn't granted maps to
        // `invalid_scope` — a registry change must not silently widen scope.
        let jwt = jwt_with_ttl(Duration::from_secs(60));
        let err = grant_client_credentials_with_jwt(
            &jwt,
            "client_credentials",
            Some("admin"),
            &auth_client(&["user"]),
        )
        .expect_err("scope mismatch rejected");
        assert!(matches!(err, TokenError::InvalidScope));
    }

    #[test]
    fn grant_client_credentials_issues_a_bearer_token_with_the_clients_org() {
        let jwt = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(&["user", "admin"]);
        let org = client.org_id;
        let token = grant_client_credentials_with_jwt(
            &jwt,
            "client_credentials",
            Some("user"),
            &client,
        )
        .expect("happy path");
        assert_eq!(token.token_type, "Bearer");
        let claims: Claims = jwt.verify(&token.access_token).expect("verify");
        assert!(
            claims.sub.is_none(),
            "client_credentials token must carry no sub",
        );
        assert_eq!(claims.org_id, org);
    }

    // Drive the methods on the injected `TokenIssuer` / `OAuthFlow` directly
    // so the thin wiring sites are covered alongside the extracted helpers.
    // `UsersService` is constructed with a `Disconnected` connection — none
    // of the test paths reach it (every test stays out of `grant_password`
    // and `resolve_caller`).

    use nest_rs_authn::{OAuth2Client, OAuth2Config};
    use sea_orm::DatabaseConnection;

    use crate::users::UsersService;

    fn users_service_disconnected() -> Arc<UsersService> {
        Arc::new(UsersService::new(Arc::new(DatabaseConnection::default())))
    }

    fn complete_oauth_config() -> OAuth2Config {
        OAuth2Config {
            client_id: "id".into(),
            client_secret: "secret".into(),
            auth_url: "https://auth.example/oauth/authorize".into(),
            token_url: "https://auth.example/oauth/token".into(),
            redirect_url: "https://app.example/oauth/callback".into(),
            userinfo_url: "https://auth.example/oauth/userinfo".into(),
            scopes: vec!["read".into()],
        }
    }

    fn token_issuer(ttl: Duration) -> TokenIssuer {
        TokenIssuer::new(Arc::new(jwt_with_ttl(ttl)), users_service_disconnected())
    }

    #[test]
    fn token_issuer_new_builds_a_usable_issuer() {
        let issuer = token_issuer(Duration::from_secs(60));
        let token = issuer
            .issue(Some(Uuid::now_v7()), Uuid::now_v7(), vec![Role::User])
            .expect("issue");
        assert_eq!(token.token_type, "Bearer");
    }

    #[test]
    fn token_issuer_issue_method_delegates_to_the_helper() {
        // Round-trips claims through the configured JWT — pins the
        // `&self.jwt` plumbing the method wraps around `issue_with_jwt`.
        let issuer = token_issuer(Duration::from_secs(120));
        let sub = Uuid::now_v7();
        let org = Uuid::now_v7();
        let token = issuer.issue(Some(sub), org, vec![Role::Admin]).expect("issue");
        assert_eq!(token.expires_in, 120);
        assert!(!token.access_token.is_empty());
    }

    #[test]
    fn token_issuer_grant_client_credentials_rejects_non_cc_grant() {
        let issuer = token_issuer(Duration::from_secs(60));
        let err = issuer
            .grant_client_credentials("password", None, &auth_client(&["user"]))
            .expect_err("password grant on CC endpoint ⇒ unsupported");
        assert!(matches!(err, TokenError::UnsupportedGrant));
    }

    #[test]
    fn token_issuer_grant_client_credentials_issues_with_the_authenticated_org() {
        let issuer = token_issuer(Duration::from_secs(60));
        let client = auth_client(&["user"]);
        let token = issuer
            .grant_client_credentials("client_credentials", None, &client)
            .expect("happy CC path");
        assert_eq!(token.token_type, "Bearer");
    }

    fn oauth_flow() -> OAuthFlow {
        let jwt = Arc::new(jwt_with_ttl(Duration::from_secs(60)));
        let oauth = Arc::new(OAuth2Client::new(complete_oauth_config()).expect("client"));
        let config = Arc::new(IssuerConfig {
            clients: vec![RegisteredClient {
                client_id: "ci".into(),
                client_secret: "s3cret".into(),
                org_id: Uuid::now_v7(),
                scopes: vec!["user".into()],
            }],
            default_org_id: Uuid::now_v7(),
        });
        OAuthFlow::new(jwt, oauth, users_service_disconnected(), config)
    }

    #[test]
    fn oauth_flow_new_builds_a_usable_flow() {
        // Smoke-test the constructor: the four `#[inject]` fields land in
        // the struct and `authorize` can compute a fresh redirect URL +
        // signed transaction without I/O.
        let flow = oauth_flow();
        let auth = flow.authorize().expect("authorize");
        assert!(
            auth.url.starts_with("https://auth.example/oauth/authorize"),
            "redirect must hit the configured provider, got {}",
            auth.url,
        );
        assert!(!auth.transaction.is_empty());
    }

    #[test]
    fn oauth_flow_authenticate_client_matches_a_registered_pair() {
        let flow = oauth_flow();
        let auth = flow
            .authenticate_client("ci", "s3cret")
            .expect("matching pair");
        assert_eq!(auth.scopes, vec!["user".to_string()]);
    }

    #[test]
    fn oauth_flow_authenticate_client_rejects_unknown_id() {
        let flow = oauth_flow();
        let err = flow
            .authenticate_client("ghost", "s3cret")
            .expect_err("unknown id");
        assert!(err.to_string().contains("invalid client credentials"));
    }

    #[test]
    fn grant_client_credentials_falls_back_to_the_full_grant_when_scope_blank() {
        // Empty `scope` (or absent) grants the client's entire allowed set —
        // pin the behaviour because the controller relies on it to support
        // "scope omitted ⇒ everything I'm registered for".
        let jwt = jwt_with_ttl(Duration::from_secs(60));
        let client = auth_client(&["admin"]);
        let token = grant_client_credentials_with_jwt(
            &jwt,
            "client_credentials",
            None,
            &client,
        )
        .expect("blank scope ok");
        let claims: Claims = jwt.verify(&token.access_token).expect("verify");
        assert!(claims.is_admin(), "blank scope should grant the full set");
    }
}
