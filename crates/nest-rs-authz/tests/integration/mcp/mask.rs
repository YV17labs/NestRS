//! Automatic response masking through the real macro: `#[authorize(Action,
//! Entity)]` beside a `#[tool]` makes `#[mcp]` emit `masked_value_for` around
//! the returned value — the host body writes no masking call.
//!
//! The two shapes a tool actually returns are covered — the structured
//! `Json<T>` (SEP-2106) and a bare value — plus the fail-closed path MCP cannot
//! escape the way GraphQL does: with no selection set to acquit it, a mask that
//! strips a *required* field fails the operation rather than serving the row.

use std::sync::Arc;

use nest_rs_authz::mcp::McpAbilityBridge;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, input, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_mcp::{Json, McpError, McpOperationGuard, mcp, tools};

use super::{PassGuard, widget};

/// The wire shape: `name` optional so a field-restricted mask yields `null`
/// rather than an irreconcilable value.
#[input]
struct WidgetDto {
    id: i32,
    name: Option<String>,
}

/// A wire shape with a **required** `name`: when the mask strips it, the masked
/// value can no longer be deserialized, and the operation must fail closed.
#[input]
struct StrictWidgetDto {
    id: i32,
    name: String,
}

/// `admin` reads widgets unrestricted; `viewer` reads them but only the `id`
/// field; `auditor` reads only widget 1, and only its `id`.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let role = req
            .headers()
            .get("x-role")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let mut builder = AbilityBuilder::new();
        match role.as_str() {
            "admin" => {
                builder.can(Action::Read, widget::Entity);
            }
            "viewer" => {
                builder
                    .can(Action::Read, widget::Entity)
                    .fields([widget::Column::Id]);
            }
            "auditor" => {
                builder
                    .can(Action::Read, widget::Entity)
                    .when(|p| p.eq(widget::Column::Id, 1))
                    .fields([widget::Column::Id]);
            }
            _ => {}
        }
        req.extensions_mut()
            .insert(Arc::new(builder.build().expect("valid test ability")));
        Ok(())
    }
}

impl HttpGuard for AbilityInjector {}

#[mcp(path = "/mcp/mask")]
#[derive(Clone, Default)]
struct MaskTool;

#[tools]
impl MaskTool {
    /// Two widgets as structured content — the shape a real tool returns.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn widgets(&self) -> Result<Json<Vec<WidgetDto>>, McpError> {
        Ok(Json(vec![
            WidgetDto {
                id: 1,
                name: Some("first".to_owned()),
            },
            WidgetDto {
                id: 2,
                name: Some("second".to_owned()),
            },
        ]))
    }

    /// A prose summary. rmcp accepts a `String` as tool output, and the mask
    /// runs over it as a scalar — nothing entity-shaped to strip, so the value
    /// passes through untouched. Kept as a witness that arming the posture on a
    /// non-entity shape is harmless rather than a compile error.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn widget_summary(&self) -> Result<String, McpError> {
        Ok("two widgets".to_owned())
    }

    /// The same row through a shape whose `name` is required.
    #[tool]
    #[authorize(Read, widget::Entity)]
    async fn strict_widget(&self) -> Result<Json<StrictWidgetDto>, McpError> {
        Ok(Json(StrictWidgetDto {
            id: 1,
            name: "first".to_owned(),
        }))
    }

    /// The opt-out: no gate, no mask.
    #[tool]
    #[public]
    async fn raw_widget(&self) -> Result<Json<WidgetDto>, McpError> {
        Ok(Json(WidgetDto {
            id: 1,
            name: Some("first".to_owned()),
        }))
    }
}

type Bridge = McpAbilityBridge<PassGuard, AbilityInjector>;

#[module(providers = [
    PassGuard,
    AbilityInjector,
    Bridge as dyn McpOperationGuard,
    MaskTool,
])]
struct MaskModule;

async fn call(role: &str, tool: &str) -> String {
    super::call_as::<MaskModule>("/mcp/mask", role, tool).await
}

#[tokio::test]
async fn an_unrestricted_caller_sees_every_field() {
    let body = call("admin", "widgets").await;
    assert!(
        body.contains("first") && body.contains("second"),
        "nothing is masked for a caller whose grant restricts nothing: {body}",
    );
}

#[tokio::test]
async fn a_field_grant_strips_the_column_it_withholds() {
    let body = call("viewer", "widgets").await;
    assert!(
        !body.contains("first") && !body.contains("second"),
        "a grant limited to `id` masks `name` out of every row, with no masking \
         call in the tool body: {body}",
    );
    assert!(
        body.contains(r#""id":1"#),
        "…while the granted column still ships: {body}",
    );
}

#[tokio::test]
async fn a_row_the_ability_refuses_is_dropped() {
    let body = call("auditor", "widgets").await;
    // Matched on the structured payload's own key, not on the digit: the frame
    // around it carries `"2.0"` and an id of its own.
    assert!(
        body.contains(r#""id":1"#),
        "the row the rule admits still ships: {body}",
    );
    assert!(
        !body.contains(r#""id":2"#),
        "…and a rule predicated on `id = 1` drops the other before it reaches a \
         model: {body}",
    );
}

#[tokio::test]
async fn a_scalar_answer_passes_through_the_mask() {
    let body = call("viewer", "widget_summary").await;
    assert!(
        body.contains("two widgets"),
        "a `String` answer has nothing entity-shaped to strip, so the posture \
         still gates but the mask is a no-op — arming `#[authorize]` on a prose \
         summary is safe, not a compile error: {body}",
    );
}

#[tokio::test]
async fn a_stripped_required_field_fails_the_operation_closed() {
    // Thread-local: `#[tokio::test]` is a current-thread runtime, so the
    // endpoint's task runs on this thread.
    let logs = nest_rs_testing::LogCapture::install();
    let body = call("viewer", "strict_widget").await;
    assert!(
        !body.contains("first"),
        "MCP has no selection set to acquit the shape with, so a mask that \
         cannot be represented refuses rather than serving the row: {body}",
    );
    assert!(
        body.contains("error"),
        "…and it refuses loudly, never as a silent passthrough: {body}",
    );

    // The caller — a language model here — is handed an opaque refusal, which
    // is the same answer it would get from a denial. So the log is the only
    // thing that separates "you may not read this" from "this operation's
    // return type and this grant cannot both be satisfied", and the second is
    // a bug in the schema that a developer has to fix.
    let event = logs.expect_one("nest_rs::authz", "response masking failed");
    assert_eq!(event.level, "warn");
    assert!(
        event.field("entity").is_some_and(|e| e.contains("widget")),
        "the event names the entity whose mask could not be represented, got {:?}",
        event.fields,
    );
    assert!(
        event.field("reason").is_some(),
        "…and which of the mask's steps failed, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn public_opts_out_of_masking() {
    let body = call("viewer", "raw_widget").await;
    assert!(
        body.contains("first"),
        "`#[public]` declares no gate and no mask — the operation is responsible \
         for its own output: {body}",
    );
}
