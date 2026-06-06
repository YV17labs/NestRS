//! Generic bearer-JWT [`Strategy`](super::Strategy) — verifies into caller-chosen claims type `C`.

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::injectable;
use poem::Request;
use serde::de::DeserializeOwned;

use crate::error::AuthError;
use crate::jwt::JwtService;
use crate::passport::{Outcome, Strategy, bearer_token};

#[injectable]
pub struct JwtStrategy<C: Send + Sync + 'static> {
    #[inject]
    jwt: Arc<JwtService>,
    _claims: PhantomData<C>,
}

impl<C: Send + Sync + 'static> JwtStrategy<C> {
    /// Construct with an already-resolved [`JwtService`] (container or tests).
    pub fn new(jwt: Arc<JwtService>) -> Self {
        Self {
            jwt,
            _claims: PhantomData,
        }
    }
}

#[async_trait]
impl<C: DeserializeOwned + Clone + Send + Sync + 'static> Strategy for JwtStrategy<C> {
    type Principal = C;

    async fn authenticate(&self, req: &mut Request) -> Result<Outcome<C>, AuthError> {
        let token = bearer_token(req).ok_or(AuthError::MissingCredentials)?;
        let claims: C = self.jwt.verify(token)?;
        Ok(Outcome::Authenticated(claims))
    }
}
