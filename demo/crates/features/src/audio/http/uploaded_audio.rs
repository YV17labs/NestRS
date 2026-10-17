use nest_rs::http::{
    Field, FromRequest, HttpError, Multipart, ProblemDetails, Request, RequestBody, Result,
};

pub struct UploadedAudio {
    pub part: Field,
}

impl<'a> FromRequest<'a> for UploadedAudio {
    async fn from_request(req: &'a Request, body: &mut RequestBody) -> Result<Self> {
        let mut form = Multipart::from_request(req, body).await?;
        while let Some(part) = form.next_field().await? {
            if part.name() == Some("file") {
                return Ok(Self { part });
            }
        }
        Err(HttpError::from(
            ProblemDetails::bad_request().with_detail("multipart body has no `file` part"),
        ))
    }
}
