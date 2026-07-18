use std::sync::Arc;

use nest_rs_http::{Valid, controller, routes};
use nest_rs_throttler::{Throttle, ThrottlerGuard};
use poem::http::StatusCode;
use poem::web::{Json, Query};
use poem::{Error, Result};

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
