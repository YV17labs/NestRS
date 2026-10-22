//! A custom authentication `Strategy` and the guard running it on a
//! controller, through the umbrella alone: the request it reads is
//! `nest_rs::http`'s, and no server library is named.

use nest_rs::authn::{AuthError, AuthnGuard, PrincipalIdentity, Strategy, bearer_token};
use nest_rs::core::{AmbientValue, async_trait};
use nest_rs::http::{Ctx, Request, controller, routes};

/// The caller a bearer token admits, named by its subject, never by the token.
#[derive(Clone)]
pub struct HygieneCaller(pub String);

/// The subject the witness's one caller carries.
const SUBJECT: &str = "hygiene-caller";

impl AmbientValue for HygieneCaller {}

impl PrincipalIdentity for HygieneCaller {
    fn actor_id(&self) -> Option<String> {
        Some(self.0.clone())
    }
}

/// Admits a request carrying a bearer token, as the witness's one caller.
pub struct HygieneStrategy;

#[async_trait]
impl Strategy for HygieneStrategy {
    type Principal = HygieneCaller;

    async fn authenticate(&self, req: &mut Request) -> Result<HygieneCaller, AuthError> {
        bearer_token(req.headers())
            .map(|_| HygieneCaller(SUBJECT.to_owned()))
            .ok_or(AuthError::MissingCredentials)
    }
}

/// The guard over the strategy, as an app aliases its own.
pub type HygieneAuthn = AuthnGuard<HygieneStrategy>;

#[controller(path = "/hygiene-authn")]
#[use_guards(HygieneAuthn)]
pub struct HygieneAuthnController;

#[routes]
impl HygieneAuthnController {
    /// The caller the guard attached.
    #[get("/me")]
    async fn me(&self, caller: Ctx<HygieneCaller>) -> String {
        caller.into_inner().0
    }
}
