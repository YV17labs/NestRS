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
        let rows = self.svc.list().await.opaque()?;

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
