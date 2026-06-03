//! Request-scoped context: the typed value a guard or interceptor attaches to
//! a request for the handler to read back. The HTTP analog of NestJS's
//! `request.user`.

use std::ops::Deref;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

/// Extracts a request-scoped value of type `T` an upstream guard or
/// interceptor attached.
///
/// Rejects with `500` if absent — a missing context means the guard that
/// should have set it never ran on this route (a wiring bug, not a client
/// error). `T` is cloned out of the request; store an `Arc<_>` for a large
/// value.
pub struct Ctx<T>(pub T);

impl<T> Ctx<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Ctx<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'a, T: Clone + Send + Sync + 'static> FromRequest<'a> for Ctx<T> {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        match req.extensions().get::<T>() {
            Some(value) => Ok(Ctx(value.clone())),
            None => Err(Error::from_string(
                format!(
                    "missing request context `{}` — is the guard or interceptor that sets it \
                     applied to this route?",
                    std::any::type_name::<T>()
                ),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }
}
