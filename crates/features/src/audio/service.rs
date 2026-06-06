use std::time::Duration;

use anyhow::Result;
use nest_rs_core::injectable;

/// Consumer-side work: the actual transcode. Injected by the `#[processor]`,
/// so only an app that mounts the queue adapter pulls it in.
#[injectable]
#[derive(Default)]
pub struct Transcoder;

impl Transcoder {
    pub async fn transcode(&self, file: &str) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tracing::info!(target: "features::audio", file, "transcoded");
        Ok(())
    }
}
