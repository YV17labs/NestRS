use std::any::TypeId;
use std::sync::Arc;

use nest_rs_authz::{Ability, Action};
use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::async_trait;
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
            Denial::internal("TranscodeGuard requires AuthGuard + AuthzGuard to run first")
        })?;

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
