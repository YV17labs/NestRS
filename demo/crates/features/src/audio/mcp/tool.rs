use std::sync::Arc;

use nest_rs::authz::Manage;
use nest_rs::mcp::{McpError, Opaque, Parameters, mcp, tools};

use crate::audio::{AudioService, TranscodeDto, Transcoding};

#[mcp]
#[derive(Clone)]
pub struct AudioTool {
    #[inject]
    svc: Arc<AudioService>,
}

#[tools]
impl AudioTool {
    #[tool(
        description = "Report whether an uploaded audio file has been transcoded. Takes the \
                       source object key returned at upload time; answers `pending` while the \
                       worker has not produced the derived object, or `ready` with a short-lived \
                       download URL once it has."
    )]
    #[authorize(Manage, Transcoding)]
    async fn transcode_status(
        &self,
        Parameters(params): Parameters<TranscodeDto>,
    ) -> Result<String, McpError> {
        Ok(match self.svc.presign_result(params.file).await.opaque()? {
            Some(ticket) => format!("ready — download (15 min): {}", ticket.url),
            None => "pending — no transcoded object yet".to_owned(),
        })
    }
}
