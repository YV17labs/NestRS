use std::any::TypeId;
use std::sync::Arc;

use nest_rs_authz::{Ability, Action};
use nest_rs_core::{Layer, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::async_trait;
use poem::Request;

use crate::orgs::Entity as OrgEntity;

/// Admin-only capability gate for `POST /audio/transcode`.
///
/// Transcoding enqueues work onto the shared `audio` queue, so it must not be
/// open to every authenticated principal (that is the queue-flood / SSRF
/// surface the audit flagged). This guard enforces the demo's existing admin
/// capability — `Action::Manage` on [`orgs::Entity`](crate::orgs::Entity),
/// which only an admin actor holds (a plain member has just `Read`) — by reading
/// the request-scoped [`Ability`] that `AuthzGuard` seeds. Because it depends on
/// that seeded ability, bind it *after* `AuthGuard`/`AuthzGuard`:
/// `#[use_guards(ThrottlerGuard, AuthGuard, AuthzGuard, TranscodeGuard)]`.
///
/// The authorization *decision* lives here, in a guard — the one place the
/// framework permits it — so it stays greppable at the `#[use_guards(...)]`
/// site, never smuggled into a parameter type or the service.
///
/// Fails **closed**: a missing ability is a wiring bug (the auth guards did not
/// run first) and denies with `500`, never an open transcode.
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
        Err(Denial::forbidden("transcoding requires the admin capability"))
    }
}
