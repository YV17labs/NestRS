//! [`OAuthResourceController`] — serves the RFC 9728 document at
//! `/.well-known/oauth-protected-resource`, in both the forms a client may ask
//! for it.

use std::sync::Arc;

use nest_rs_http::{IntoResponse, Path, Response, StatusCode, controller, routes};

use crate::metadata::ProtectedResourceMetadata;

/// The discovery endpoint every OAuth client reaches before it holds a token.
/// Declared `#[public]`: an unauthenticated caller must read it.
#[controller(path = "/.well-known")]
pub(crate) struct OAuthResourceController {
    #[inject]
    metadata: Arc<ProtectedResourceMetadata>,
}

#[routes]
impl OAuthResourceController {
    #[get("/oauth-protected-resource")]
    #[public]
    async fn metadata(&self) -> ProtectedResourceMetadata {
        (*self.metadata).clone()
    }

    /// The path-aware form (RFC 9728 §3.1); the tail must be *this* resource's
    /// path, or every sibling path would claim this identity.
    #[get("/oauth-protected-resource/{*resource_path}")]
    #[public]
    async fn metadata_for_path(&self, Path(resource_path): Path<String>) -> Response {
        if resource_path.trim_end_matches('/') != self.metadata.resource_path() {
            return StatusCode::NOT_FOUND.into_response();
        }
        (*self.metadata).clone().into_response()
    }
}
