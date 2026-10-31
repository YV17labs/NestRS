use nest_rs::core::input;

#[input]
#[derive(Debug, Clone, Copy)]
pub struct StreamResumeDto {
    #[serde(rename = "Last-Event-ID")]
    pub last_event_id: Option<u32>,
}
