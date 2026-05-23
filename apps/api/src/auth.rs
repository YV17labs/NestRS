//! API-key authentication: a guard that rejects unauthenticated requests and
//! attaches the [`Caller`] for handlers to read via `nestrs_http::Ctx<Caller>`.
//! A deliberately minimal stand-in for real auth (JWT, sessions) that exercises
//! the guard → request-context → handler path end to end.

use nestrs_core::injectable;
use nestrs_http::{async_trait, Guard};
use poem::http::StatusCode;
use poem::{Request, Response};

/// The authenticated caller, attached to the request by [`ApiKeyGuard`] and read
/// by handlers via `nestrs_http::Ctx<Caller>`.
#[derive(Debug, Clone)]
pub struct Caller {
    pub api_key: String,
}

/// Rejects any request without a non-empty `x-api-key` header (`401`); on
/// success attaches a [`Caller`]. An `#[injectable]` provider, so it is wired
/// into the container like any service and bound to a route with
/// `#[use_guards(ApiKeyGuard)]`.
#[injectable]
#[derive(Default)]
pub struct ApiKeyGuard;

#[async_trait]
impl Guard for ApiKeyGuard {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        // Own the key before borrowing the request mutably to attach the caller.
        let api_key = req
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .filter(|k| !k.is_empty())
            .map(str::to_owned);
        match api_key {
            Some(api_key) => {
                req.extensions_mut().insert(Caller { api_key });
                Ok(())
            }
            None => Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body("missing or empty x-api-key header")),
        }
    }
}
