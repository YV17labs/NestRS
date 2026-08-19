//! The decorators reached the way the docs tell a developer to reach them:
//! `use nest_rs::prelude::*;` and nothing else.
//!
//! **This is the only reader the prelude has**, and until it existed the module
//! had none anywhere in either workspace — advertised on the packages page,
//! written in the crate's own module doc, and compiled by nobody. It drifted
//! exactly where nothing looked: `#[resolver]` and `#[mcp]` were re-exported
//! without `#[operations]` and `#[tools]`, so following the documented import
//! gave a developer the struct half of two decorator pairs and an unresolved
//! attribute on the impl half — whose natural remedy is the second manifest
//! line this whole crate exists to disprove the need for.
//!
//! Both halves of a pair are applied below rather than merely imported: a glob
//! import cannot fail on a name it does not find, so only using an attribute
//! proves the prelude carries it.

use nest_rs::prelude::*;

/// A provider and a typed input, neither named by a `use` of its own.
///
/// `#[module]` is witnessed through the prelude too, and in `module.rs`: it is
/// the one decorator whose file the naming tables fix, so a second `#[module]`
/// here would break the layout rule to prove an import.
#[injectable]
pub struct PreludeService;

#[input]
#[derive(Clone)]
pub struct PreludeInput {
    #[validate(length(min = 1))]
    pub value: String,
}

/// HTTP's pair.
#[controller(path = "/prelude")]
pub struct PreludeController;

#[routes]
impl PreludeController {
    #[get("/")]
    #[public]
    async fn index(&self) -> String {
        "ok".into()
    }
}

/// GraphQL's pair — the struct half was re-exported and the impl half was not.
#[resolver]
pub struct PreludeResolver;

#[operations]
impl PreludeResolver {
    #[query]
    #[public]
    async fn prelude(&self) -> String {
        "ok".into()
    }
}

/// MCP's pair — same hole, same shape.
#[mcp(path = "/prelude-mcp")]
#[derive(Clone, Default)]
pub struct PreludeTool;

#[tools]
impl PreludeTool {
    #[tool(description = "Answer with a constant.")]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs::mcp::McpError> {
        Ok("ok".into())
    }
}

/// WS's pair, and the queue pair beside it — both were already whole, and both
/// are here so the prelude's decorator surface is witnessed as a set rather
/// than at the two members that happened to break.
#[gateway(path = "/prelude-ws")]
#[derive(Default)]
pub struct PreludeGateway;

#[messages]
impl PreludeGateway {
    #[subscribe_message("prelude.ping")]
    #[public]
    async fn ping(&self) {}
}

/// The port both halves of the queue pair agree on.
#[queue(name = "prelude", job = PreludeInput)]
pub struct PreludeQueue;

#[injectable]
pub struct PreludeProcessor;

#[processor]
impl PreludeProcessor {
    #[process(queue = PreludeQueue, transactional = false)]
    async fn run(&self, _job: PreludeInput) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }
}

#[injectable]
pub struct PreludeTasks;

#[scheduled]
impl PreludeTasks {
    #[every("60s", transactional = false)]
    async fn tick(&self) -> nest_rs::core::anyhow::Result<()> {
        Ok(())
    }
}
