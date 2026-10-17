use nest_rs::http::{
    Field, FromRequest, HttpError, Multipart, ProblemDetails, Request, RequestBody, Result,
};
use validator::Validate;

use crate::audio::UploadRequestDto;

pub struct UploadedAudio {
    pub filename: String,
    pub part: Field,
}

impl<'a> FromRequest<'a> for UploadedAudio {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let mut form = Multipart::from_request(req, body).await?;
        while let Some(part) = form.next_field().await? {
            if part.name() != Some("file") {
                continue;
            }
            let upload = UploadRequestDto {
                filename: part.file_name().map(str::to_owned).unwrap_or_default(),
            };
            if upload.validate().is_err() {
                return Err(HttpError::from(
                    ProblemDetails::unprocessable()
                        .with_detail("the `file` part's filename is not an accepted audio name"),
                ));
            }
            return Ok(Self {
                filename: upload.filename,
                part,
            });
        }
        Err(HttpError::from(
            ProblemDetails::bad_request().with_detail("multipart body has no `file` part"),
        ))
    }
}
