use std::sync::Arc;

use nest_rs_mcp::mcp;
use nest_rs_mcp::{
    CallToolResult, ContentBlock, McpError, Parameters, ServerHandler, tool, tool_handler,
    tool_router,
};
use validator::Validate;

use crate::audio::{AudioService, TranscodeDto};

#[mcp(path = "/mcp")]
#[derive(Clone)]
pub struct AudioTool {
    #[inject]
    svc: Arc<AudioService>,
}

#[tool_router]
impl AudioTool {
    #[tool(
        description = "Report whether an uploaded audio file has been transcoded. \
                       Takes the source object key returned at upload time; answers \
                       `pending` while the worker has not produced the derived object, \
                       or `ready` with a short-lived download URL once it has."
    )]
    async fn transcode_status(
        &self,
        Parameters(params): Parameters<TranscodeDto>,
    ) -> Result<CallToolResult, McpError> {
        params
            .validate()
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        let status = self.svc.presign_result(&params.file).await.map_err(|e| {
            tracing::error!(target: "features::audio", error = ?e, "audio tool status lookup failed");
            McpError::internal_error("audio operation failed", None)
        })?;

        let summary = match status {
            Some(ticket) => format!("ready — download (15 min): {}", ticket.url),
            None => format!("pending — no transcoded object for {} yet", params.file),
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(summary)]))
    }
}

#[tool_handler]
impl ServerHandler for AudioTool {}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Discoverable;

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
