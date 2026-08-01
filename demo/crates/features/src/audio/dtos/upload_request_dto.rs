use nest_rs::http::input;

#[input]
#[derive(Debug, Clone)]
pub struct UploadRequestDto {
    #[validate(
        length(min = 1, max = 255),
        custom(function = "super::transcode_dto::validate_transcode_file")
    )]
    pub filename: String,
}
