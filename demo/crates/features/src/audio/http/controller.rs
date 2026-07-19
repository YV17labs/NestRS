use std::sync::Arc;

use std::time::Duration;

use futures_util::stream;
use nest_rs_http::{Valid, controller, routes};
use nest_rs_throttler::{Throttle, ThrottlerGuard};
use poem::http::StatusCode;
use poem::web::sse::{Event, SSE};
use poem::web::{Json, Multipart, Query};
use poem::{Body, Error, Response, Result};
use validator::Validate;

use super::guard::TranscodeGuard;
use crate::audio::{AudioService, PresignedUrlDto, TranscodeDto, UploadRequestDto};
use crate::authn::AuthGuard;
use crate::authz::AuthzGuard;

#[controller(path = "/audio")]
#[use_guards(ThrottlerGuard, AuthGuard, AuthzGuard, TranscodeGuard)]
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
        description = "Returns a short-lived presigned PUT URL plus the object key it addresses. \
                       The client pushes the file bytes straight to object storage (the server \
                       never proxies the payload), then calls `POST /audio/transcode` with the \
                       returned key. The `filename` is validated against the same anti-traversal \
                       allowlist as the transcode request. Requires a bearer JWT and the admin \
                       capability (`Manage` on the caller's org).",
        tags("Audio")
    )]
    async fn create_upload(
        &self,
        body: Valid<Json<UploadRequestDto>>,
    ) -> Result<Json<PresignedUrlDto>> {
        let req = body.into_inner();
        let ticket =
            self.svc.presign_upload(&req.filename).await.map_err(|e| {
                Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
            })?;
        Ok(Json(ticket))
    }

    #[get("/results")]
    #[meta(Throttle::per_minute(60))]
    #[api(
        summary = "Fetch a presigned GET URL for a transcoded object",
        description = "Given the source object `key` (query param `file`, validated like the \
                       transcode request), returns a short-lived presigned GET URL for the \
                       derived object the worker produced, or `404` while it does not exist yet. \
                       Requires a bearer JWT and the admin capability.",
        tags("Audio")
    )]
    async fn result(&self, query: Valid<Query<TranscodeDto>>) -> Result<Json<PresignedUrlDto>> {
        let file = query.into_inner().file;
        match self
            .svc
            .presign_result(&file)
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
        {
            Some(ticket) => Ok(Json(ticket)),
            None => Err(Error::from_status(StatusCode::NOT_FOUND)),
        }
    }

    #[post("/uploads/direct")]
    #[meta(Throttle::per_minute(20))]
    #[api(
        summary = "Upload an audio file directly as multipart/form-data",
        description = "The single-round-trip alternative to the presigned flow: the client posts a \
                       `multipart/form-data` body with a `file` part; the server buffers the part \
                       and stores it, then returns the object key plus a presigned GET URL. The \
                       part's filename is validated against the same anti-traversal allowlist as \
                       the presigned path. Requires a bearer JWT and the admin capability.",
        tags("Audio")
    )]
    async fn upload_direct(&self, mut form: Multipart) -> Result<Json<PresignedUrlDto>> {
        while let Some(field) = form
            .next_field()
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::BAD_REQUEST))?
        {
            if field.name() != Some("file") {
                continue;
            }
            let filename = field.file_name().map(str::to_owned).unwrap_or_default();
            UploadRequestDto {
                filename: filename.clone(),
            }
            .validate()
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
            let bytes = field
                .bytes()
                .await
                .map_err(|e| Error::from_string(e.to_string(), StatusCode::BAD_REQUEST))?;
            let ticket = self.svc.store_upload(&filename, bytes).await.map_err(|e| {
                Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
            })?;
            return Ok(Json(ticket));
        }
        Err(Error::from_string(
            "multipart body has no `file` part",
            StatusCode::BAD_REQUEST,
        ))
    }

    #[get("/download")]
    #[meta(Throttle::per_minute(60))]
    #[api(
        summary = "Stream a transcoded object back through the server",
        description = "Given the source object `key` (query param `file`), streams the derived \
                       object's bytes chunk by chunk as the response body — the large file never \
                       sits whole in server memory — or `404` while the worker has not produced it \
                       yet. The presigned `GET /audio/results` URL is the zero-proxy alternative; \
                       this endpoint is the streamed proxy. Requires a bearer JWT and the admin \
                       capability.",
        tags("Audio")
    )]
    async fn download(&self, query: Valid<Query<TranscodeDto>>) -> Result<Response> {
        let file = query.into_inner().file;
        match self
            .svc
            .open_result(&file)
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
        {
            Some(stream) => Ok(Response::builder()
                .content_type("audio/mpeg")
                .body(Body::from_bytes_stream(stream))),
            None => Err(Error::from_status(StatusCode::NOT_FOUND)),
        }
    }

    #[get("/events")]
    #[meta(Throttle::per_minute(60))]
    #[api(
        summary = "Stream transcode progress as Server-Sent Events",
        description = "Opens a `text/event-stream` that polls the derived object and emits a \
                       `transcode` event (`{state: pending|ready}`) until it is produced, then \
                       ends — a live progress feed the browser reads with `EventSource`. Requires \
                       a bearer JWT and the admin capability.",
        tags("Audio")
    )]
    async fn events(&self, query: Valid<Query<TranscodeDto>>) -> SSE {
        let file = query.into_inner().file;
        let svc = self.svc.clone();
        let events = stream::unfold(0u32, move |attempt| {
            let svc = svc.clone();
            let file = file.clone();
            async move {
                if attempt >= 20 {
                    return None;
                }
                let event = |state: &str| {
                    Event::message(format!("{{\"state\":\"{state}\",\"attempt\":{attempt}}}"))
                        .event_type("transcode")
                };
                match svc.result_ready(&file).await {
                    Ok(true) => Some((event("ready"), u32::MAX)),
                    Ok(false) => {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        Some((event("pending"), attempt + 1))
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "features::audio",
                            file = %file,
                            error = %e,
                            "transcode status poll failed",
                        );
                        Some((event("error"), u32::MAX))
                    }
                }
            }
        });
        SSE::new(events).keep_alive(Duration::from_secs(15))
    }

    #[post("/transcode")]
    #[meta(Throttle::per_minute(20))]
    #[api(
        summary = "Enqueue a transcode job for the worker to process",
        description = "Accepts a TranscodeDto body and enqueues a TranscodeCommand onto the \
                       shared `audio` queue; the separate worker deployable consumes it over \
                       Redis (two apps exchanging, no RPC). Requires a bearer JWT, is admin-only \
                       (`Manage` on the caller's org, enforced by `TranscodeGuard`), rate-limited \
                       by `ThrottlerGuard`, and its `file` is validated against a filename \
                       allowlist that blocks path traversal.",
        tags("Audio")
    )]
    async fn transcode(&self, body: Valid<Json<TranscodeDto>>) -> Result<Json<TranscodeDto>> {
        let job = body.into_inner();
        self.svc
            .enqueue_transcode(job.file.clone())
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
        Ok(Json(job))
    }
}
