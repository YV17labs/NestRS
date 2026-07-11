use nest_rs_authn::{OAuth2Client, OAuth2Config};
use nest_rs_core::{ContainerBuilder, DynamicModule};

/// Registers the two keyed `OAuth2Client`s — GitHub and Google — into the one
/// flat container under distinct keys, during the **collect** phase so that
/// importing [`SocialModule`](super::SocialModule) is self-sufficient at *every*
/// composition root: the binary and the e2e harness alike get both clients
/// without a root-level `provide_keyed` seed. This is the keyed-provider
/// exemplar's wiring — two instances of one type in one container,
/// disambiguated by key, never a per-provider newtype or a per-provider module.
///
/// Collect (not register) is deliberate: `App::build` snapshots the global keyed
/// set after collect but before the modules register, so a `provide_keyed` here
/// is what the access-graph keyed pass validates `SocialLoginService`'s
/// `#[inject(key = "…")]` fields against.
#[derive(Default)]
pub struct SocialOAuthClientsModule;

impl DynamicModule for SocialOAuthClientsModule {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        // The configs are static literals, so a validation failure is a
        // programming error that must abort boot — the one-shot-bootstrap
        // `expect` exception. Credentials are demo placeholders; a real
        // deployment reads them from configuration.
        let github =
            OAuth2Client::new(github_config()).expect("static GitHub OAuth config is valid");
        let google =
            OAuth2Client::new(google_config()).expect("static Google OAuth config is valid");
        builder
            .provide_keyed("github", github)
            .provide_keyed("google", google)
    }
}

fn github_config() -> OAuth2Config {
    OAuth2Config {
        client_id: "demo-github-client-id".into(),
        client_secret: "demo-github-client-secret".into(),
        auth_url: "https://github.com/login/oauth/authorize".into(),
        token_url: "https://github.com/login/oauth/access_token".into(),
        redirect_url: "http://localhost:3001/social/github/callback".into(),
        userinfo_url: "https://api.github.com/user".into(),
        scopes: vec!["read:user".into(), "user:email".into()],
    }
}

fn google_config() -> OAuth2Config {
    OAuth2Config {
        client_id: "demo-google-client-id".into(),
        client_secret: "demo-google-client-secret".into(),
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token_url: "https://oauth2.googleapis.com/token".into(),
        redirect_url: "http://localhost:3001/social/google/callback".into(),
        userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".into(),
        scopes: vec!["openid".into(), "email".into(), "profile".into()],
    }
}
