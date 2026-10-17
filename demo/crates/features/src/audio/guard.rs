use std::any::TypeId;

use nest_rs::authz::{Ability, Action};
use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::http::HttpView;
use nest_rs::mcp::McpView;

use crate::orgs::Entity as OrgEntity;

#[injectable]
#[derive(Default)]
pub struct TranscodeGuard;

impl Layer for TranscodeGuard {}

#[async_trait]
impl Guard for TranscodeGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = cx.ambient::<Ability>().ok_or_else(|| {
            Denial::internal("TranscodeGuard requires AuthnGuard + AuthzGuard to run first")
        })?;
        Self::decide(&ability)
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = cx.ambient::<Ability>().ok_or_else(|| {
            Denial::internal("TranscodeGuard requires AuthzMcpModule's guards to run first")
        })?;
        Self::decide(&ability)
    }
}

impl GuardFor<HttpView> for TranscodeGuard {}

impl GuardFor<McpView> for TranscodeGuard {}

impl TranscodeGuard {
    fn decide(ability: &Ability) -> Result<(), Denial> {
        if ability.can_class(Action::Manage, TypeId::of::<OrgEntity>()) {
            return Ok(());
        }

        tracing::warn!(
            target: crate::audio::TARGET,
            action = ?Action::Manage,
            subject = std::any::type_name::<OrgEntity>(),
            "transcode denied: caller lacks the admin capability",
        );
        Err(Denial::forbidden(
            "transcoding requires the admin capability",
        ))
    }
}
