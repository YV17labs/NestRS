use poem::http::StatusCode;
use poem::web::Multipart;
use poem::{Error, FromRequest, Request, RequestBody, Result};
use validator::Validate;

use crate::audio::UploadRequestDto;

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
