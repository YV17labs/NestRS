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
    #[query]
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
        let _ = client.broadcast("{{kebab}}.count", &self.svc.count());
    }
}
"#;

pub const QUEUE_PROCESSOR: &str = r#"use anyhow::Result;
use nest_rs_core::injectable;
use nest_rs_queue::processor;
use serde::{Deserialize, Serialize};

/// The job payload exchanged over the `{{kebab}}` queue. Producer and consumer
/// apps share this contract through the feature crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {{singular}}Job {
    pub id: String,
}

#[injectable]
#[derive(Default)]
pub struct {{processor}};

#[processor]
impl {{processor}} {
    #[process(queue = "{{kebab}}", concurrency = 1, retries = 3)]
    async fn handle(&self, job: {{singular}}Job) -> Result<()> {
        tracing::info!(target: "features::{{snake}}", id = %job.id, "processing job");
        Ok(())
    }
}
"#;

/// The queue processor has no port dependency, so its module imports nothing.
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
//! Security: the MCP endpoint denies every request by default until an
//! `McpOperationGuard` is bound. Wire your app's `McpAbilityBridge`
//! (`features::authz::mcp`) as `dyn McpOperationGuard` so callers are
//! authenticated and the ambient `Ability` is installed; return entity rows
//! through `nest_rs_authz::mcp::masked_output` to apply field-level masking.
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
