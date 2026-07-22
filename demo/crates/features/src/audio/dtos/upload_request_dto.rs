use nest_rs_http::input;
use schemars::JsonSchema;
use serde::Serialize;

#[input]
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UploadRequestDto {
    #[validate(
        length(min = 1, max = 255),
        custom(function = "super::transcode_dto::validate_transcode_file")
    )]
    pub filename: String,
}
