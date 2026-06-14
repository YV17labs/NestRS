use nest_rs_mcp::schemars;
use serde::Deserialize;
use validator::Validate;

#[derive(Debug, Deserialize, schemars::JsonSchema, Validate)]
pub struct CoordsParamsDto {
    #[validate(range(min = -90.0, max = 90.0))]
    pub latitude: f64,

    #[validate(range(min = -180.0, max = 180.0))]
    pub longitude: f64,
}
