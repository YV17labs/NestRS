use std::sync::Arc;

use nestrs_http::{controller, routes};
use nestrs_queue::QueueConnection;
use poem::http::StatusCode;
use poem::web::Json;
use poem::{Error, Result};

use crate::audio::core::{TranscodeJob, AUDIO_QUEUE};
use crate::authn::AuthGuard;
use crate::authz::AppAbilityGuard;

/// Producer side: an HTTP request enqueues a job for the `worker` app to
/// consume. Injects only the shared [`QueueConnection`] — no transcoder, no
/// entity. Authed like the rest of the API.
#[controller(path = "/audio")]
#[use_guards(AuthGuard, AppAbilityGuard)]
pub struct AudioController {
    #[inject]
    queue: Arc<QueueConnection>,
}

#[routes]
impl AudioController {
    #[post("/transcode")]
    #[api(
        summary = "Enqueue a transcode job for the worker to process",
        description = "Pushes a TranscodeJob onto the shared `audio` queue; the separate \
                       platform-worker deployable consumes it over Redis (two apps exchanging, \
                       no RPC). Requires a bearer JWT.",
        tags("Audio")
    )]
    async fn transcode(&self, body: Json<TranscodeJob>) -> Result<Json<TranscodeJob>> {
        let job = body.0;
        self.queue
            .of::<TranscodeJob>(AUDIO_QUEUE)
            .push(job.clone())
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
        tracing::info!(target: "features::audio", file = %job.file, "enqueued transcode job");
        Ok(Json(job))
    }
}
