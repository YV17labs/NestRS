use std::sync::Arc;

use std::future::ready;

use futures_util::StreamExt;
use nest_rs::core::error_message;
use nest_rs::http::{
    Body, Header, HeaderValue, HttpError, Json, Query, Response, Result, SseEvent, SseStream,
    StatusCode, Valid, controller, routes,
};
use nest_rs::throttler::{Throttle, ThrottlerGuard};

use super::uploaded_audio::UploadedAudio;
use crate::audio::TranscodeGuard;
use crate::audio::{
    AudioService, DirectUploadDto, PresignedUrlDto, StreamResumeDto, TranscodeDto, TranscodeState,
};
use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;

#[controller(path = "/audio")]
#[use_guards(ThrottlerGuard, AuthnGuard, AuthzGuard, TranscodeGuard)]
pub struct AudioController {
    #[inject]
    svc: Arc<AudioService>,
}

#[routes]
impl AudioController {
    #[post("/uploads")]
    #[meta(Throttle::per_minute(20))]
    #[api(
        summary = "Mint a presigned PUT URL for a direct audio upload",
        description = "Returns a short-lived presigned PUT URL, the method and headers the upload \
                       sends verbatim, and the object key the server minted for it. The URL binds \
                       `Content-Type: audio/mpeg`: object storage refuses an upload sending \
                       another type, or none. The client pushes the file bytes straight to object \
                       storage (the server never proxies the payload), then calls \
                       `POST /audio/transcode` with the returned key. Requires a bearer JWT and \
                       the admin capability (`Manage` on the caller's org)."
    )]
    async fn create_upload(&self) -> Result<Json<PresignedUrlDto>> {
        Ok(Json(self.svc.presign_upload().await?))
    }

    #[get("/results")]
    #[meta(Throttle::per_minute(60))]
    #[api(
        summary = "Fetch a presigned GET URL for a transcoded object",
        description = "Given the source object `key` an upload returned (query param `file`, a \
                       UUID), returns a short-lived presigned GET URL for the derived object the \
                       worker produced, or `404` while it does not exist yet. Requires a bearer \
                       JWT and the admin capability."
    )]
    async fn result(&self, query: Valid<Query<TranscodeDto>>) -> Result<Json<PresignedUrlDto>> {
        let file = query.into_inner().file;
        match self.svc.presign_result(file).await? {
            Some(ticket) => Ok(Json(ticket)),
            None => Err(HttpError::from_status(StatusCode::NOT_FOUND)),
        }
    }

    #[post("/uploads/direct")]
    #[meta(Throttle::per_minute(20))]
    #[api(
        summary = "Upload an audio file directly as multipart/form-data",
        description = "The single-round-trip alternative to the presigned flow: the client posts a \
                       `multipart/form-data` body with a `file` part, which streams straight into \
                       the object store — the file never sits whole in server memory — and the \
                       response carries the object key the server minted plus a presigned GET \
                       URL. The part's filename is never read: no client names a storage key. \
                       Requires a bearer JWT and the admin capability.",
        multipart = DirectUploadDto
    )]
    async fn upload_direct(&self, upload: UploadedAudio) -> Result<Json<PresignedUrlDto>> {
        Ok(Json(
            self.svc
                .store_upload(upload.part.into_byte_stream())
                .await?,
        ))
    }

    #[get("/download")]
    #[meta(Throttle::per_minute(60))]
    #[api(
        summary = "Stream a transcoded object back through the server",
        description = "Given the source object `key` (query param `file`, a UUID), streams the \
                       derived object's bytes chunk by chunk as the response body — the large \
                       file never sits whole in server memory — or `404` while the worker has not \
                       produced it yet. The presigned `GET /audio/results` URL is the zero-proxy \
                       alternative; this endpoint is the streamed proxy. Requires a bearer JWT \
                       and the admin capability.",
        response_content_type = "audio/mpeg"
    )]
    async fn download(&self, query: Valid<Query<TranscodeDto>>) -> Result<Response> {
        let file = query.into_inner().file;
        match self.svc.open_result(file).await? {
            Some(stream) => Ok(Response::builder()
                .content_type(HeaderValue::from_static("audio/mpeg"))
                .body(Body::from_stream(stream))),
            None => Err(HttpError::from_status(StatusCode::NOT_FOUND)),
        }
    }

    #[sse("/events")]
    #[meta(Throttle::per_minute(60))]
    #[api(
        summary = "Stream transcode progress as Server-Sent Events",
        description = "Opens a `text/event-stream` that polls the transcode — the derived \
                       object first, then the record the worker files once it has produced it — \
                       and emits a `transcode` event (`{state: pending|ready}`) until it is \
                       produced, then ends — a live progress feed the browser reads with `EventSource`. Each \
                       event carries the poll attempt as its SSE id, so a browser reconnecting \
                       sends it back as `Last-Event-ID` and is not re-sent the progress ticks it \
                       already displayed; the terminal state is always sent. Requires a bearer \
                       JWT and the admin capability."
    )]
    async fn events(
        &self,
        query: Valid<Query<TranscodeDto>>,
        resume: Header<StreamResumeDto>,
    ) -> SseStream {
        let file = query.into_inner().file;
        let seen = resume.into_inner().last_event_id;
        let events = self
            .svc
            .clone()
            .transcode_events(file)
            .filter(move |payload| {
                let replayed = matches!(payload.state, TranscodeState::Pending)
                    && seen.is_some_and(|last| payload.attempt <= last);
                ready(!replayed)
            })
            .map(|payload| {
                let id = payload.attempt.to_string();
                let body = serde_json::to_string(&payload).unwrap_or_else(|e| {
                    tracing::error!(
                        target: crate::audio::TARGET,
                        error = %error_message(&e),
                        "failed to serialize transcode event",
                    );
                    r#"{"state":"error"}"#.to_string()
                });
                SseEvent::message(body).event_type("transcode").id(id)
            });
        SseStream::new(events)
    }

    #[post("/transcode")]
    #[meta(Throttle::per_minute(20))]
    #[api(
        summary = "Enqueue a transcode job for the worker to process",
        description = "Accepts a TranscodeDto body and enqueues a TranscodeCommand onto the \
                       shared `audio` queue, answering `202` once it is queued; the separate \
                       worker deployable consumes it over Redis (two apps exchanging, no RPC). \
                       Requires a bearer JWT, is admin-only (`Manage` on the caller's org, \
                       enforced by `TranscodeGuard`) and rate-limited by `ThrottlerGuard`; its \
                       `file` is the key an upload returned, a UUID, so no client names a \
                       storage path."
    )]
    #[http_code(202)]
    async fn transcode(&self, body: Valid<Json<TranscodeDto>>) -> Result<()> {
        self.svc.enqueue_transcode(body.into_inner().file).await?;
        Ok(())
    }
}
