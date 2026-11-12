use nest_rs::core::output;

#[output]
#[derive(Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum TranscodeState {
    Pending,
    Ready,
    Error,
}
