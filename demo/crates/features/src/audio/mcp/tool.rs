use std::sync::Arc;

use nest_rs::mcp::{McpError, Opaque, Parameters, Valid, mcp, tools};

use crate::audio::{AudioService, TranscodeDto, TranscodeGuard};

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
    #[public]
    #[use_guards(TranscodeGuard)]
    async fn transcode_status(
        &self,
        Parameters(params): Parameters<Valid<TranscodeDto>>,
    ) -> Result<String, McpError> {
        let params = params.into_inner();

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
    use crate::audio::{AudioService, TranscodeGuard};

    #[test]
    fn mcp_tool_declares_its_injected_service_for_the_access_graph() {
        assert!(AudioTool::dependencies().is_empty());
        assert!(
            AudioTool::injected().contains(&TypeId::of::<AudioService>()),
            "the MCP tool's injected AudioService is recorded for the access graph",
        );
    }

    #[test]
    fn the_operation_guard_is_recorded_for_the_access_graph_too() {
        assert!(
            AudioTool::injected().contains(&TypeId::of::<TranscodeGuard>()),
            "a guard bound beside a #[tool] is a dependency the boot must be able \
             to resolve, exactly as on a controller — otherwise a missing module \
             would surface as an ungated tool instead of a boot error",
        );
    }
}
