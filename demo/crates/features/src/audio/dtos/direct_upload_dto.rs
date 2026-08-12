use schemars::JsonSchema;

#[derive(Debug, Clone, JsonSchema)]
pub struct DirectUploadDto {
    #[schemars(extend("format" = "binary"))]
    pub file: String,
}
