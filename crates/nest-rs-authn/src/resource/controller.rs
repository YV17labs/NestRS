//! [`ProtectedResourceController`] — serves the RFC 9728 document at
//! `/.well-known/oauth-protected-resource`, in both the forms a client may ask
//! for it.
//!
//! **Why two routes.** RFC 9728 §3.1 inserts the well-known string between the
//! authority and the resource's path, so a deployment identifying as
//! `https://api.example.com/mcp` publishes at
//! `…/.well-known/oauth-protected-resource/mcp`. That is the form the challenge
//! advertises and the one a spec-following client requests first. The
//! unsuffixed route stays mounted because it is what a bare-origin resource
//! publishes at, and what a client that skipped the challenge tries — but it
//! answers with this deployment's document either way, never a document for a
//! resource that merely shares the prefix.

use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::http::StatusCode;
use poem::web::Path;
use poem::{IntoResponse, Response};

use crate::resource::metadata::ProtectedResourceMetadata;

/// The discovery endpoint every OAuth client reaches before it holds a token.
///
/// It is **declared** public, not public by omission: an unauthenticated caller
/// must read it — that is the point of discovery — and `#[public]` is the one
/// greppable site that says so.
#[controller(path = "/.well-known")]
pub struct ProtectedResourceController {
    #[inject]
    metadata: Arc<ProtectedResourceMetadata>,
}

#[routes]
impl ProtectedResourceController {
    #[get("/oauth-protected-resource")]
    #[public]
    async fn metadata(&self) -> Response {
        self.document()
    }

    /// The path-aware form (RFC 9728 §3.1). The tail must be *this* resource's
    /// path: answering `…/oauth-protected-resource/anything` would tell a client
    /// that every sibling path is a protected resource with this identity, which
    /// is a discovery lie rather than a convenience.
    #[get("/oauth-protected-resource/*resource_path")]
    #[public]
    async fn metadata_for_path(&self, Path(resource_path): Path<String>) -> Response {
        if resource_path.trim_end_matches('/') != self.metadata.resource_path() {
            return StatusCode::NOT_FOUND.into_response();
        }
        self.document()
    }

    /// The frozen document, serialized. Shared by both routes so the two forms
    /// cannot answer differently.
    fn document(&self) -> Response {
        // The document is frozen at boot and its fields were validated then, so
        // a serialization failure here is out-of-memory territory. It must not
        // ship an empty 200 regardless: a client would read that as a resource
        // with no authorization server and abandon the flow.
        match serde_json::to_vec(&*self.metadata) {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .content_type("application/json")
                .body(body),
            Err(error) => {
                tracing::error!(
                    target: "nest_rs::authn",
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
