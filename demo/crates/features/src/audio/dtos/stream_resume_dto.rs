use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
pub struct StreamResumeDto {
    #[serde(rename = "Last-Event-ID")]
    pub last_event_id: Option<u32>,
}
