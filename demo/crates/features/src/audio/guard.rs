use std::any::TypeId;

use nest_rs::authz::{Ability, Action};
use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, GrantedScopes, Guard, GuardFor, async_trait};
use nest_rs::http::HttpView;
use nest_rs::mcp::McpView;

use crate::authz::constants::AUDIO_TRANSCODE;
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
        Self::decide(&ability, cx.ambient::<GrantedScopes>().as_deref())
    }

    async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = cx.ambient::<Ability>().ok_or_else(|| {
            Self::unwired("TranscodeGuard requires AuthzMcpModule's guards to run first")
        })?;
        Self::decide(&ability, cx.ambient::<GrantedScopes>().as_deref())
    }

    fn denial_statuses(&self, _public: bool) -> &'static [u16] {
        &[403]
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

    fn decide(ability: &Ability, granted: Option<&GrantedScopes>) -> Result<(), Denial> {
        if !ability.can_class(Action::Manage, TypeId::of::<OrgEntity>()) {
            tracing::warn!(
                target: crate::audio::TARGET,
                action = ?Action::Manage,
                subject = std::any::type_name::<OrgEntity>(),
                reason = "no_class_grant",
                "transcode denied: caller lacks the admin capability",
            );
            return Err(Denial::forbidden(
                "transcoding requires the admin capability",
            ));
        }
        if granted.is_none_or(|granted| granted.as_slice().iter().any(|s| s == AUDIO_TRANSCODE)) {
            return Ok(());
        }

        tracing::warn!(
            target: crate::audio::TARGET,
            scope = AUDIO_TRANSCODE,
            reason = "insufficient_scope",
            "transcode denied: the token does not carry the transcode scope",
        );
        Err(Denial::insufficient_scope(
            [AUDIO_TRANSCODE],
            "the request requires higher privileges than provided by the access token",
        ))
    }
}

#[cfg(test)]
mod tests {
    use nest_rs::authz::{AbilityBuilder, AbilityFactory};
    use uuid::Uuid;

    use super::*;
    use crate::authz::{AuthzAbility, constants};
    use crate::{Claims, Role, Subject};

    fn verdict(roles: Vec<Role>, scopes: &[&str]) -> Result<(), Denial> {
        let claims = Claims {
            sub: Subject::Client("demo-service".into()),
            client_id: Some("demo-service".into()),
            org_id: Uuid::now_v7(),
            roles,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            exp: 0,
        };
        let granted = GrantedScopes::new(scopes.iter().copied());
        let mut builder = AbilityBuilder::new().with_granted_scopes(Some(granted.shared()));
        AuthzAbility.define(&claims, &mut builder);
        let ability = builder.build().expect("valid test ability");
        TranscodeGuard::decide(&ability, Some(&granted))
    }

    #[test]
    fn an_admin_token_granted_the_transcode_scope_may_transcode() {
        assert!(verdict(vec![Role::Admin], &constants::ALL).is_ok());
    }

    #[test]
    fn a_member_is_refused_with_no_challenge_a_wider_token_could_answer() {
        let refused =
            verdict(vec![Role::User], &constants::ALL).expect_err("a member never transcodes");
        assert_eq!(refused.http_status(), 403);
        assert_eq!(refused.bearer_error(), None, "{refused:?}");
    }

    #[test]
    fn a_principal_that_is_not_scope_aware_is_not_scope_gated() {
        let mut builder = AbilityBuilder::new();
        builder.can(Action::Manage, OrgEntity);
        let ability = builder.build().expect("valid test ability");
        assert!(TranscodeGuard::decide(&ability, None).is_ok());
    }

    #[test]
    fn an_admin_token_narrowed_to_reading_posts_is_challenged_for_the_transcode_scope() {
        let refused = verdict(vec![Role::Admin], &[constants::POSTS_READ])
            .expect_err("the advertised scope gates what it names");
        assert_eq!(refused.http_status(), 403);
        assert_eq!(
            refused.bearer_error(),
            Some("insufficient_scope"),
            "{refused:?}"
        );
        assert_eq!(refused.required_scopes(), [constants::AUDIO_TRANSCODE]);
    }
}
