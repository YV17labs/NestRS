use nest_rs_http::input;
use serde::Serialize;

#[input]
#[derive(Debug, Clone, Serialize)]
pub struct UploadRequestDto {
    #[validate(
        length(min = 1, max = 255),
        custom(function = "super::transcode_dto::validate_transcode_file")
    )]
    pub filename: String,
}
