use poem::http::StatusCode;
use poem::web::{Field, Multipart};
use poem::{Error, FromRequest, Request, RequestBody, Result};
use validator::Validate;

use crate::audio::UploadRequestDto;

pub struct UploadedAudio {
    pub filename: String,
    pub part: Field,
}

impl<'a> FromRequest<'a> for UploadedAudio {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let mut form = Multipart::from_request(req, body).await?;
        while let Some(part) = form
            .next_field()
            .await
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::BAD_REQUEST))?
        {
            if part.name() != Some("file") {
                continue;
            }
            let filename = part.file_name().map(str::to_owned).unwrap_or_default();
            UploadRequestDto {
                filename: filename.clone(),
            }
            .validate()
            .map_err(|e| Error::from_string(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
            return Ok(Self { filename, part });
        }
        Err(Error::from_string(
            "multipart body has no `file` part",
            StatusCode::BAD_REQUEST,
        ))
    }
}
