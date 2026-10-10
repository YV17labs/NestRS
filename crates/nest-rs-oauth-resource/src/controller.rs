//! [`OAuthResourceController`] — serves the RFC 9728 document at
//! `/.well-known/oauth-protected-resource`, in both the forms a client may ask
//! for it.

use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::http::StatusCode;
use poem::web::Path;
use poem::{IntoResponse, Response};

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
    async fn metadata(&self) -> Response {
        self.document()
    }

    /// The path-aware form (RFC 9728 §3.1); the tail must be *this* resource's
    /// path, or every sibling path would claim this identity.
    #[get("/oauth-protected-resource/{*resource_path}")]
    #[public]
    async fn metadata_for_path(&self, Path(resource_path): Path<String>) -> Response {
        if resource_path.trim_end_matches('/') != self.metadata.resource_path() {
            return StatusCode::NOT_FOUND.into_response();
        }
        self.document()
    }

    /// The frozen document, serialized, shared by both routes.
    fn document(&self) -> Response {
        // Never an empty 200: a client would read it as a resource with no
        // authorization server and abandon the flow.
        match serde_json::to_vec(&*self.metadata) {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .content_type("application/json")
                .body(body),
            Err(error) => {
                tracing::error!(
                    target: crate::TARGET,
                    %error,
                    resource = self.metadata.resource(),
                    "protected resource metadata failed to serialize",
                );
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .finish()
            }
        }
    }
}
