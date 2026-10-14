//! `McpDataContext` re-installs the ability an MCP request's in-band guards
//! kept on it around each operation, which runs on rmcp's own task.

#![cfg(feature = "mcp")]

use nest_rs_authz::{AbilityBuilder, current_ability};
use nest_rs_core::{Container, Layer, UnitContext};
use nest_rs_guards::{Denial, Guard, async_trait};
use nest_rs_http::__private::check_request;
use nest_rs_mcp::{McpError, McpToolContext};
use nest_rs_seaorm::mcp::McpDataContext;
use poem::Request;

/// Attaches an ability, as the authorization guard does.
struct Granting;

impl Layer for Granting {}

#[async_trait]
impl Guard for Granting {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        let ability = AbilityBuilder::new()
            .build()
            .expect("an ability with no rule builds");
        cx.attach(ability);
        Ok(())
    }
}

#[tokio::test]
async fn capture_reads_the_ability_the_in_band_guards_kept_on_the_request() {
    let container = Container::builder()
        .provide_arc(crate::harness::connect_arc().await)
        .build();
    let ctx = McpDataContext::from_container(&container);

    let mut req = Request::default();
    check_request(
        &nest_rs_mcp::__private::ENDPOINT,
        [("Granting", &Granting as &dyn Guard)],
        &mut req,
    )
    .await
    .unwrap_or_else(|denial| panic!("Granting admits every request: {denial:?}"));
    let captured = ctx.capture(&req);

    let outcome = ctx
        .around(
            &captured,
            Box::pin(async {
                assert!(
                    current_ability().is_some(),
                    "the request's ability is re-installed around the operation",
                );
                Err(McpError::invalid_request("the operation ran", None))
            }),
        )
        .await;
    assert!(
        outcome.is_err_and(|err| err.message == "the operation ran"),
        "the operation itself answered",
    );
}
