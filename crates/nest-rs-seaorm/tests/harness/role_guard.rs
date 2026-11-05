//! What both suites' `graphql::relations` read a schema through: the caller's
//! ability, by the `x-role` a test sends and the `ability_of` of the suite including
//! this, and what the schema answers and files.

use nest_rs_authz::__private::AmbientAbility;
use nest_rs_core::{Layer, UnitContext, async_trait, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::HttpView;
use nest_rs_testing::{LogCapture, TestApp};
use serde_json::{Value, json};

/// The caller's ability, by the `x-role` a test sends; no header is a visitor's.
#[injectable]
#[derive(Default)]
pub(crate) struct RoleGuard;

impl Layer for RoleGuard {}

#[async_trait]
impl Guard for RoleGuard {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let role = HttpView::of(cx)?
            .request()
            .header("x-role")
            .unwrap_or_default()
            .to_owned();
        cx.attach(AmbientAbility::new(super::ability_of(&role)));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for RoleGuard {}

/// What `query` answers `role`; an empty one sends no header.
pub(crate) async fn answer(app: &TestApp, role: &str, query: &str) -> Value {
    let mut request = app.http().post("/graphql");
    if !role.is_empty() {
        request = request.header("x-role", role);
    }
    request
        .body_json(&json!({ "query": query }))
        .send()
        .await
        .json()
        .await
        .value()
        .deserialize()
}

/// What a read of granted fields, or its refusal, never files.
#[track_caller]
pub(crate) fn assert_no_error_line(logs: &LogCapture) {
    let filed: Vec<_> = logs
        .events_at(tracing::Level::ERROR)
        .into_iter()
        .map(|event| format!("{} {} {:?}", event.target, event.message, event.fields))
        .collect();
    assert!(filed.is_empty(), "{filed:?}");
}
