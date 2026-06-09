use std::sync::Arc;

use nest_rs_http::{controller, routes};
use poem::http::StatusCode;
use poem::web::Json;
use poem::{Error, Result};

use crate::audio::{AudioService, TranscodeJob};
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;

#[controller(path = "/audio")]
#[use_guards(AuthGuard, AuthzGuard)]
pub struct AudioController {
    #[inject]
    svc: Arc<AudioService>,
}

#[routes]
impl AudioController {
    #[post("/transcode")]
    #[api(
        summary = "Enqueue a transcode job for the worker to process",
        description = "Pushes a TranscodeJob onto the shared `audio` queue; the separate \
                       publish-worker deployable consumes it over Redis (two apps exchanging, \
                       no RPC). Requires a bearer JWT.",
        tags("Audio")
    )]
    async fn transcode(&self, body: Json<TranscodeJob>) -> Result<Json<TranscodeJob>> {
        let job = body.0;
        self.svc
            .enqueue_transcode(job.file.clone())
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
        Ok(Json(job))
    }
}
