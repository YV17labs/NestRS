use std::sync::Arc;

use nest_rs_http::{controller, routes};
use nest_rs_redis::QueueConnection;
use poem::http::StatusCode;
use poem::web::Json;
use poem::{Error, Result};

use crate::audio::{AUDIO_QUEUE, TranscodeJob};

/// Producer side: an HTTP request enqueues a job for the `worker` app to
/// consume. Injects only the shared [`QueueConnection`] — no transcoder, no
/// entity. Authn/authz come from the app-level `use_guards_global` chain.
#[controller(path = "/audio")]
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
