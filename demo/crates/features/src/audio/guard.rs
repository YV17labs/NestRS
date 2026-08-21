use std::any::TypeId;
use std::sync::Arc;

use nest_rs::authz::{Ability, Action, current_ability};
use nest_rs::core::{Layer, injectable};
use nest_rs::guards::{Denial, Guard, HttpGuard, McpGuard, async_trait};
use nest_rs::mcp::McpOperationContext;
use poem::Request;

use crate::orgs::Entity as OrgEntity;

#[injectable]
#[derive(Default)]
pub struct TranscodeGuard;

impl Layer for TranscodeGuard {}

#[async_trait]
impl Guard for TranscodeGuard {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let ability = req.extensions().get::<Arc<Ability>>().ok_or_else(|| {
            Denial::internal("TranscodeGuard requires AppAuthnGuard + AppAuthzGuard to run first")
        })?;
        Self::decide(ability)
    }

    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        let ability = current_ability().ok_or_else(|| {
            Denial::internal("TranscodeGuard requires the MCP authz bridge to run first")
        })?;
        Self::decide(&ability)
    }
}

impl HttpGuard for TranscodeGuard {}

impl McpGuard for TranscodeGuard {}

impl TranscodeGuard {
    fn decide(ability: &Ability) -> Result<(), Denial> {
        if ability.can_class(Action::Manage, TypeId::of::<OrgEntity>()) {
            return Ok(());
        }

        tracing::warn!(
            target: "features::audio",
            action = ?Action::Manage,
            subject = std::any::type_name::<OrgEntity>(),
            "transcode denied: caller lacks the admin capability",
        );
        Err(Denial::forbidden(
            "transcoding requires the admin capability",
        ))
    }
}
