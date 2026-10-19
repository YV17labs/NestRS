//! Generic bearer-JWT [`Strategy`] — verifies into caller-chosen claims type `C`.

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::{AmbientValue, injectable};
use nest_rs_http::Request;
use serde::de::DeserializeOwned;

use crate::JwtService;
use crate::error::AuthError;
use crate::{PrincipalIdentity, Strategy, bearer_token};

/// Bearer-token [`Strategy`]: reads the `Authorization: Bearer`
/// token and verifies it into the caller-chosen claims type `C` via the injected
/// [`JwtService`]. `C` must be the principal (`DeserializeOwned +
/// PrincipalIdentity + AmbientValue`).
#[injectable]
pub struct JwtStrategy<C: Send + Sync + 'static> {
    #[inject]
    svc: Arc<JwtService>,
    _claims: PhantomData<C>,
}

impl<C: Send + Sync + 'static> JwtStrategy<C> {
    /// Construct with an already-resolved [`JwtService`] (container or tests).
    pub fn new(svc: Arc<JwtService>) -> Self {
        Self {
            svc,
            _claims: PhantomData,
        }
    }
}

#[async_trait]
impl<C: DeserializeOwned + PrincipalIdentity + AmbientValue + Clone> Strategy for JwtStrategy<C> {
    type Principal = C;

    async fn authenticate(&self, req: &mut Request) -> Result<C, AuthError> {
        let token = bearer_token(req.headers()).ok_or(AuthError::MissingCredentials)?;
        let claims: C = self.svc.verify(token).await?;
        Ok(claims)
    }
}
