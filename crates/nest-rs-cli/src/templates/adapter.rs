//! **Adapter** skeletons — one transport bolted onto an existing port
//! (`g http|graphql|ws|queue|schedule|mcp <feature>`).
//!
//! Each skeleton delegates to the port service's `count()` (the method
//! `g feature` emits) so a freshly-generated port + any adapter compiles
//! immediately. The handler is the seam the developer then fills in.

/// `mod.rs` for an adapter folder: `mod <handler>; mod module;` + re-exports.
/// `{{handler_mod}}`/`{{handler}}`/`{{tmodule}}` are layered per transport.
pub const MOD: &str = r#"mod {{handler_mod}};
mod module;

pub use {{handler_mod}}::{{handler}};
pub use module::{{tmodule}};
"#;

/// Adapter `module.rs` — imports the port, provides the handler.
pub const MODULE: &str = r#"use nest_rs_core::module;

use super::{{handler_mod}}::{{handler}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{handler}}],
)]
pub struct {{tmodule}};
"#;

pub const HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs_http::{controller, routes};

use crate::{{snake}}::{{service}};

#[controller(path = "/{{kebab}}")]
pub struct {{controller}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[routes]
impl {{controller}} {
    #[get("/")]
    async fn list(&self) -> String {
        format!("{} items", self.svc.count())
    }
}
"#;

pub const GRAPHQL_RESOLVER: &str = r#"use std::sync::Arc;

use async_graphql::Result;
use nest_rs_graphql::resolver;

use crate::{{snake}}::{{service}};

#[resolver]
pub struct {{resolver}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[resolver]
impl {{resolver}} {
    // SECURITY: scaffolded as #[public]. Before exposing real data, declare
    // #[authorize(Action, Entity)] instead (class gate + automatic response
    // masking) and import AuthzGraphqlModule — see crates/features/src/users/graphql/.
    #[query]
    #[public]
    async fn {{snake}}_count(&self) -> Result<usize> {
        Ok(self.svc.count())
    }
}
"#;

pub const WS_GATEWAY: &str = r#"use std::sync::Arc;

use nest_rs_ws::{WsClient, gateway, messages};

use crate::{{snake}}::{{service}};

#[gateway(path = "/ws")]
pub struct {{gateway}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[messages]
impl {{gateway}} {
    #[subscribe_message("{{kebab}}.count")]
    async fn count(&self, client: &WsClient) {
        // Best-effort fan-out: a peer disconnecting mid-broadcast is normal, so
        // a delivery failure is logged rather than propagated (this handler
        // returns `()` — there is no client left to surface an error to).
        if let Err(e) = client.broadcast("{{kebab}}.count", &self.svc.count()) {
            tracing::warn!(target: "features::{{snake}}", error = %e, "broadcast failed");
        }
    }
}
"#;

pub const QUEUE_PROCESSOR: &str = r#"use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_queue::processor;

use crate::{{snake}}::{{command}};

#[injectable]
#[derive(Default)]
pub struct {{processor}};

#[processor]
impl {{processor}} {
    #[process(queue = "{{kebab}}", concurrency = 1, retries = 3)]
    async fn handle(&self, job: {{command}}) -> Result<()> {
        tracing::info!(target: "features::{{snake}}", id = %job.id, "processing job");
        Ok(())
    }
}
"#;

/// The queue payload — an imperative **`Command`** living at the feature *port*,
/// not in the `queue/` adapter: it is a producer↔worker contract the consumer's
/// `processor.rs` imports. The default is a Command (the common case); rename it
/// verb-led to the real action, or switch to an `…Event` (past tense) when a
/// fact is published to several consumers.
pub const QUEUE_COMMAND: &str = r#"use serde::{Deserialize, Serialize};

/// Imperative payload for the `{{kebab}}` queue — "do this work", handled by one
/// processor. Rename it to the action it commands (e.g. `GenerateMediaVariantCommand`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {{command}} {
    pub id: String,
}
"#;

/// The queue processor has no port *provider* dependency, so its module imports
/// nothing — the `Command` it handles is a plain type, not an injected provider.
pub const QUEUE_MODULE: &str = r#"use nest_rs_core::module;

use super::processor::{{processor}};

#[module(providers = [{{processor}}])]
pub struct {{queue_module}};
"#;

pub const SCHEDULE_TASKS: &str = r#"use std::sync::Arc;

use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_schedule::scheduled;

use crate::{{snake}}::{{service}};

#[injectable]
pub struct {{tasks}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[scheduled]
impl {{tasks}} {
    #[every("60s")]
    async fn tick(&self) -> Result<()> {
        tracing::info!(target: "features::{{snake}}", count = self.svc.count(), "scheduled tick");
        Ok(())
    }
}
"#;

pub const MCP_TOOL: &str = r#"//! MCP tool for `{{snake}}`.
//!
//! Security: the MCP endpoint gates through the app's `dyn McpOperationGuard`,
//! else the global guard pool (`use_guards_global`), else deny-all. Wire your
//! app's `McpAbilityBridge` (`features::authz::mcp`) as `dyn McpOperationGuard`
//! so callers are authenticated and the ambient `Ability` is installed; return
//! entity rows through `nest_rs_authz::masked_output_ambient` to apply
//! field-level masking.
use std::sync::Arc;

use nest_rs_mcp::mcp;
use nest_rs_mcp::{CallToolResult, Content, McpError, ServerHandler, tool, tool_handler, tool_router};

use crate::{{snake}}::{{service}};

#[mcp(path = "/mcp")]
#[derive(Clone)]
pub struct {{tool}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[tool_router]
impl {{tool}} {
    #[tool(description = "Count {{kebab}} items.")]
    async fn count(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            self.svc.count().to_string(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for {{tool}} {}
"#;
