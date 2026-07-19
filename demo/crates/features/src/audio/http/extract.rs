use poem::http::StatusCode;
use poem::web::Multipart;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use validator::Validate;

use crate::audio::UploadRequestDto;

/// The `file` part of a `multipart/form-data` upload, buffered and validated
/// at the edge — the handler receives an already-checked value instead of
/// walking the form itself. The filename runs through the same anti-traversal
/// allowlist as the presigned path.
pub struct UploadedAudio {
    pub filename: String,
    pub bytes: Vec<u8>,
}

impl<'a> FromRequest<'a> for UploadedAudio {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let mut form = Multipart::from_request(req, body).await?;
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
            return Ok(Self { filename, bytes });
        }
        Err(Error::from_string(
            "multipart body has no `file` part",
            StatusCode::BAD_REQUEST,
        ))
    }
}
