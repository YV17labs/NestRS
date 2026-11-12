//! The decorators reached through `use nest_rs::prelude::*;` and nothing else.
//!
//! Both halves of each pair are applied, not merely imported: a glob import
//! cannot fail on a name it does not find. Each edge takes the one `Scoped<T>`
//! the prelude names, so a parameter on any of them fails the day it names one
//! edge's alone.

use nest_rs::prelude::*;

/// A provider, a typed input and a typed reply, none named by a `use` of its
/// own; `#[module]`'s prelude witness is `module.rs`.
#[injectable]
pub struct PreludeService;

#[input]
#[derive(Clone)]
pub struct PreludeInput {
    #[validate(length(min = 1))]
    pub value: String,
}

#[output]
pub struct PreludeOutput {
    pub value: String,
}

/// The enum both decoded and sent, with no feature of its own.
#[expect(
    clippy::exhaustive_enums,
    reason = "a witness of what a developer writes, never published"
)]
#[wire_enum]
pub enum PreludeTier {
    Free,
    Pro,
}

#[cfg(feature = "http")]
pub mod http {
    use nest_rs::prelude::*;

    use super::PreludeService;

    #[controller(path = "/prelude")]
    pub struct PreludeController;

    #[routes]
    impl PreludeController {
        #[get("/")]
        #[public]
        async fn index(&self, _service: Scoped<PreludeService>) -> String {
            "ok".into()
        }
    }
}

#[cfg(feature = "graphql")]
pub mod graphql {
    use nest_rs::prelude::*;

    use super::PreludeService;

    #[resolver]
    pub struct PreludeResolver;

    #[operations]
    impl PreludeResolver {
        #[query]
        #[public]
        async fn prelude(&self, _service: Scoped<PreludeService>) -> String {
            "ok".into()
        }
    }
}

#[cfg(feature = "mcp")]
pub mod mcp {
    use nest_rs::prelude::*;

    use super::PreludeService;

    #[mcp(path = "/prelude-mcp")]
    #[derive(Clone, Default)]
    pub struct PreludeTool;

    #[tools]
    impl PreludeTool {
        #[tool(description = "Answer with a constant.")]
        #[public]
        async fn ping(
            &self,
            _service: Scoped<PreludeService>,
        ) -> Result<String, nest_rs::mcp::McpError> {
            Ok("ok".into())
        }
    }
}

#[cfg(feature = "ws")]
pub mod ws {
    use nest_rs::prelude::*;

    use super::PreludeService;

    #[gateway(path = "/prelude-ws")]
    #[derive(Default)]
    pub struct PreludeGateway;

    #[messages]
    impl PreludeGateway {
        #[subscribe_message("prelude.ping")]
        #[public]
        async fn ping(&self, _service: Scoped<PreludeService>) {}
    }
}

#[cfg(feature = "queue")]
pub mod queue {
    use nest_rs::prelude::*;

    use super::PreludeInput;

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
}

#[cfg(feature = "schedule")]
pub mod schedule {
    use nest_rs::prelude::*;

    #[injectable]
    pub struct PreludeTasks;

    #[scheduled]
    impl PreludeTasks {
        #[every("60s", transactional = false)]
        async fn tick(&self) -> nest_rs::core::anyhow::Result<()> {
            Ok(())
        }
    }
}
