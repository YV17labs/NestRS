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
            Self::unwired("TranscodeGuard requires AuthnGuard + AuthzGuard to run first")
        })?;
        Self::decide(&ability)
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = cx.ambient::<Ability>().ok_or_else(|| {
            Self::unwired("TranscodeGuard requires AuthzMcpModule's guards to run first")
        })?;
        Self::decide(&ability)
    }

    fn files_its_denials(&self) -> bool {
        true
    }
}

impl GuardFor<HttpView> for TranscodeGuard {}

impl GuardFor<McpView> for TranscodeGuard {}

impl TranscodeGuard {
    fn unwired(why: &'static str) -> Denial {
        tracing::error!(
            target: crate::audio::TARGET,
            reason = "no_ability",
            "transcode denied: the ability guards did not run first",
        );
        Denial::internal(why)
    }

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
