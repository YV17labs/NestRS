use std::sync::Arc;

use nest_rs::mcp::{McpError, Opaque, Parameters, mcp};
use validator::Validate;

use crate::audio::{AudioService, TranscodeDto};

#[mcp]
#[derive(Clone)]
pub struct AudioTool {
    #[inject]
    svc: Arc<AudioService>,
}

#[mcp]
impl AudioTool {
    #[tool(
        description = "Report whether an uploaded audio file has been transcoded. \
                       Takes the source object key returned at upload time; answers \
                       `pending` while the worker has not produced the derived \
                       object, or `ready` with a short-lived download URL once it has."
    )]
    async fn transcode_status(
        &self,
        Parameters(params): Parameters<TranscodeDto>,
    ) -> Result<String, McpError> {
        params
            .validate()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        Ok(
            match self.svc.presign_result(&params.file).await.opaque()? {
                Some(ticket) => format!("ready — download (15 min): {}", ticket.url),
                None => format!("pending — no transcoded object for {} yet", params.file),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::core::Discoverable;

    use super::AudioTool;
    use crate::audio::AudioService;

    #[test]
    fn mcp_tool_declares_its_injected_service_for_the_access_graph() {
        assert!(AudioTool::dependencies().is_empty());
        assert!(
            AudioTool::injected().contains(&TypeId::of::<AudioService>()),
            "the MCP tool's injected AudioService is recorded for the access graph",
        );
    }
}
