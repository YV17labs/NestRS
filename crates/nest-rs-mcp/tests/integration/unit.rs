//! Covers `src/unit.rs` from the site's side: the declaration each tool and
//! prompt emits, naming its unit and carrying the host's and its own `#[meta]`.

use nest_rs_core::Posture;
use nest_rs_mcp::{McpError, mcp, tools};

#[derive(Debug, PartialEq)]
struct Audit(&'static str);

#[derive(Debug, PartialEq)]
struct Cost(u32);

/// Declared only on a site no test reads, so built by its declaration alone.
#[deny(dead_code)]
struct Unread;

#[mcp(path = "/mcp/meta")]
#[meta(Audit("host"))]
#[meta(Cost(1))]
#[derive(Clone, Default)]
struct MeteredTools;

#[tools]
impl MeteredTools {
    #[tool(name = "quote_price", description = "Quote a price.")]
    #[public]
    #[meta(Cost(5))]
    async fn price(&self) -> Result<String, McpError> {
        Ok(String::new())
    }

    #[tool(description = "Stay idle.")]
    #[public]
    #[meta(Unread)]
    async fn idle(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

#[test]
fn a_tool_declares_its_unit_address_and_metadata() {
    let handler = MeteredTools::__NESTRS_HANDLER_PRICE;
    assert_eq!(handler.unit(), nest_rs_mcp::unit::OPERATION);
    assert_eq!(handler.host_name(), "MeteredTools");
    assert_eq!(handler.method(), "price");
    assert_eq!(handler.address(), "quote_price", "the name a client calls");
    assert_eq!(handler.posture(), Posture::Public);
    let reflector = handler.reflector();
    assert_eq!(reflector.get::<Cost>(), Some(&Cost(5)));
    assert_eq!(reflector.get::<Audit>(), Some(&Audit("host")));
}
