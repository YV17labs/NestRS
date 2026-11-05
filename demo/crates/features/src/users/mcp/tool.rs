use std::sync::Arc;

use nest_rs::authz::Read;
use nest_rs::mcp::{Json, mcp, tools};
use nest_rs::seaorm::{CrudService, ServiceError};

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
    async fn list_people(&self) -> Result<Json<Vec<PersonDto>>, ServiceError> {
        let rows = self.svc.list().await?;

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
