use std::sync::Arc;

use nest_rs::authz::Read;
use nest_rs::mcp::{Json, McpError, Opaque, mcp, tools};
use nest_rs::seaorm::CrudService;

use crate::users::{Entity as UserEntity, PersonDto, UsersService};

#[mcp]
#[derive(Clone)]
pub struct UsersTool {
    #[inject]
    svc: Arc<UsersService>,
}

#[tools]
impl UsersTool {
    #[tool(
        description = "List the people the caller is allowed to see. Scoped to the caller's \
                       organization."
    )]
    #[authorize(Read, UserEntity, unmasked)]
    async fn list_people(&self) -> Result<Json<Vec<PersonDto>>, McpError> {
        let rows = CrudService::list(&*self.svc).await.opaque()?;

        Ok(Json(
            rows.into_iter()
                .map(|row| PersonDto {
                    id: row.id,
                    name: row.name,
                })
                .collect(),
        ))
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
