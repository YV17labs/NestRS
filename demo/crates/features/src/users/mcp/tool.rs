use std::sync::Arc;

use nest_rs::mcp::{McpError, Opaque, mcp};
use nest_rs::seaorm::CrudService;

use crate::users::UsersService;

#[mcp]
#[derive(Clone)]
pub struct UsersTool {
    #[inject]
    svc: Arc<UsersService>,
}

#[mcp]
impl UsersTool {
    #[tool(
        description = "List the people the caller is allowed to see, by name. \
                       Scoped to the caller's organization."
    )]
    async fn list_people(&self) -> Result<String, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;

        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        Ok(if names.is_empty() {
            "no readable people".to_owned()
        } else {
            names.join("\n")
        })
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::core::Discoverable;

    use super::UsersTool;
    use crate::users::UsersService;

    #[test]
    fn mcp_tool_declares_its_injected_service_for_the_access_graph() {
        assert!(
            UsersTool::injected().contains(&TypeId::of::<UsersService>()),
            "the MCP tool's injected UsersService is recorded for the access graph",
        );
    }
}
