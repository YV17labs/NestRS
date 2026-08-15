//! **Adapter** skeletons — one transport bolted onto an existing port
//! (`g http|graphql|ws|queue|schedule|mcp <feature>`).
//!
//! Each skeleton delegates to the port service's `count()` (the method
//! `g feature` emits) so a freshly-generated port + any adapter compiles
//! immediately. The handler is the seam the developer then fills in.
//!
//! **A `g resource` port has no `count()`** — its service is a `CrudService`
//! (`list`/`page`/`access`/`create`/`update`/`delete`), so a skeleton calling
//! `count()` on one does not compile. Each transport therefore renders one
//! template with the differing handler supplied as `{{op}}` / `{{op_body}}` /
//! `{{op_value}}` (see [`crud_vars`](super::crud_vars)), rather than a second
//! near-identical blob: the scaffolding, imports and path conventions have one
//! home each. GraphQL is the exception that earns a second template
//! ([`GRAPHQL_RESOLVER_CRUD`]) — over a resource it is not a stub but the full
//! `#[crud]` block behind the app's guards.

/// `mod.rs` for an adapter folder: `mod <handler>; mod module;` + re-exports.
/// `{{handler_mod}}`/`{{handler}}`/`{{tmodule}}` are layered per transport.
pub const MOD: &str = r#"mod {{handler_mod}};
mod module;

pub use {{handler_mod}}::{{handler}};
pub use module::{{tmodule}};
"#;

/// Adapter `module.rs` — imports the port, provides the handler.
///
/// **Every adapter imports the port**, including the queue's: the moment the
/// generated stub grows the shape the docs prescribe — a thin processor handing
/// the job to the port service — the access graph fails the boot unless the port
/// module is already there. Scaffolding the import costs nothing (registration
/// is idempotent) and removes a boot error from the developer's first edit.
pub const MODULE: &str = r#"use nest_rs::core::module;

use super::{{handler_mod}}::{{handler}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{handler}}],
)]
pub struct {{tmodule}};
"#;

pub const HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::http::{controller, routes};

use crate::{{snake}}::{{service}};

#[controller(path = "/{{kebab}}")]
pub struct {{controller}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[routes]
impl {{controller}} {
    // SECURITY: scaffolded as #[public] because this port holds no entity.
    // Before serving real rows, declare #[authorize(Action, Entity)] instead
    // (class gate + automatic response masking), bind
    // #[use_guards(AuthnGuard, AuthzGuard)] on the struct, and import
    // AuthzHttpModule in this adapter's module.rs — `nestrs g auth` writes all
    // three, and `nestrs g http` on a `g resource` port emits them.
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        format!("{} items", self.svc.count())
    }
}
"#;

pub const GRAPHQL_RESOLVER: &str = r#"use std::sync::Arc;

use async_graphql::Result;
use nest_rs::graphql::{operations, resolver};

use crate::{{snake}}::{{service}};

#[resolver]
pub struct {{resolver}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[operations]
impl {{resolver}} {
    // SECURITY: scaffolded as #[public] because this port holds no entity.
    // Before serving real rows, declare #[authorize(Action, Entity)] instead
    // (class gate + automatic response masking), bind
    // #[use_guards(AuthnGuard, AuthzGuard)] on the struct, and import
    // AuthzGraphqlModule in this adapter's module.rs — `nestrs g auth` writes
    // all three, and `nestrs g graphql` on a `g resource` port emits them.
    #[query]
    #[public]
    async fn {{snake}}_count(&self) -> Result<usize> {
        Ok(self.svc.count())
    }
}
"#;

/// The GraphQL adapter for a **resource** port: the `#[crud]` resolver behind
/// the app's guards, the exact twin of `resource::HTTP_CONTROLLER` — same
/// service, same ability, same rows, one transport over.
pub const GRAPHQL_RESOLVER_CRUD: &str = r#"use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::{{snake}}::{{{create_op}}, Entity as {{entity}}Entity, {{entity}}, {{service}}, {{update_op}}};

#[resolver]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct {{resolver}} {
    #[inject]
    svc: Arc<{{service}}>,
}

// Every operation declares #[authorize(Action, Entity)], so each is
// authenticated, ability-filtered, transactional and field-masked — generated
// by #[crud], the same way the HTTP controller is. They answer FORBIDDEN until
// AppAbility grants a rule for {{entity}}.
#[crud(
    service = svc,
    entity = {{entity}}Entity,
    output = {{entity}},
    create = {{create_op}},
    update = {{update_op}},
)]
impl {{resolver}} {}
"#;

/// The WS adapter's `module.rs`. `WsModule` is **not optional** — it owns the
/// connection registry every `WsClient` reads, for the default namespace and for
/// every `#[gateway(namespace = …)]` marker alike — so the generator writes it
/// rather than leaving the app to discover it at boot.
pub const WS_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::{{handler_mod}}::{{handler}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}, WsModule],
    providers = [{{handler}}],
)]
pub struct {{tmodule}};
"#;

pub const WS_GATEWAY: &str = r#"use std::sync::Arc;

use nest_rs::ws::{WsClient, gateway, messages};

use crate::{{snake}}::{{service}};

// The path carries the feature name: a self-mount path is its exclusive
// namespace, so a bare `/ws` made the *second* generated gateway fail boot on a
// duplicate path. `/ws/<feature>` also stays clear of the HTTP controller
// adapter, which claims `/<feature>`.
#[gateway(path = "/ws/{{kebab}}")]
pub struct {{gateway}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[messages]
impl {{gateway}} {
    // Every message declares its access posture, and this scaffold's reply is a
    // broadcast rather than entity rows — so `#[public]`: no gate, no mask, and
    // the guards a gateway binds on its struct still run at the upgrade. A
    // message that answers with entity rows takes
    // `#[authorize(Read, Entity)]` instead, and the mask comes with it.
    #[subscribe_message("{{kebab}}.{{op}}")]
    #[public]
    async fn {{op}}(&self, client: &WsClient) {
{{op_body}}
        // Best-effort fan-out: a peer disconnecting mid-broadcast is normal, so
        // a delivery failure is logged rather than propagated (this handler
        // returns `()` — there is no client left to surface an error to).
        if let Err(e) = client.broadcast("{{kebab}}.{{op}}", {{op_value}}) {
            tracing::warn!(target: "features::{{snake}}", error = %e, "broadcast failed");
        }
    }
}
"#;

pub const QUEUE_PROCESSOR: &str = r#"use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::queue::processor;

use crate::{{snake}}::{{{command}}, {{queue_name}}};

#[injectable]
#[derive(Default)]
pub struct {{processor}};

#[processor]
impl {{processor}} {
    #[process(queue = {{queue_name}}, retries = 3)]
    async fn handle(&self, job: {{command}}) -> Result<()> {
        tracing::info!(target: "features::{{snake}}", id = %job.id, "processing job");
        Ok(())
    }
}
"#;

/// The queue payload **and its `QueueName`** — both at the feature *port*, not
/// in the `queue/` adapter: they are the producer↔worker contract, and the
/// producer is usually the port's own service, one directory up. Keeping the
/// marker beside the payload is what makes `push_to::<Q>` reachable; declaring
/// it inside the private `queue::processor` module would leave the untyped
/// `push(name, job)` escape hatch as the only way to enqueue.
///
/// The default payload is a Command (the common case); rename it verb-led to
/// the real action, or switch to an `…Event` (past tense) when a fact is
/// published to several consumers.
pub const QUEUE_COMMAND: &str = r#"use nest_rs::queue::queue;
use serde::{Deserialize, Serialize};

/// Imperative payload for the `{{kebab}}` queue — "do this work", handled by one
/// processor. Rename it to the action it commands (e.g. `GenerateMediaVariantCommand`).
///
/// Plain `serde` derives rather than `#[input]`, and the difference is not
/// stylistic: `#[input]` carries `deny_unknown_fields`, which is right at an
/// edge a client controls and wrong on a producer↔worker contract. A producer
/// one deploy ahead that adds a field would have every job it enqueues refused
/// by the older worker — and a decode failure is a `JobError::abort`, so the
/// job dead-letters on its first attempt instead of retrying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {{command}} {
    pub id: String,
}

/// The queue's identity — wire name + payload type in one artifact the producer
/// and the processor both import, so a typo or a mismatched payload is a compile
/// error rather than a job that silently never drains. Enqueue with
/// `queue.push_to::<{{queue_name}}>(…)`.
#[queue(name = "{{kebab}}", job = {{command}})]
pub struct {{queue_name}};
"#;

pub const SCHEDULE_TASKS: &str = r#"use std::sync::Arc;

use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::schedule::scheduled;

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
{{op_body}}
        // Every event carries at least one structured field: the cadence is what
        // tells a reader which tick fired when several share this target.
        tracing::info!(target: "features::{{snake}}", every = "60s", "scheduled tick");
        Ok(())
    }
}
"#;

pub const MCP_TOOL: &str = r#"//! MCP tool for `{{snake}}`.
//!
//! Security: the MCP endpoint gates through the app's `dyn McpOperationGuard`,
//! else the global guard pool (`use_guards_global`), else deny-all. Wire your
//! app's `McpAbilityBridge` (`features::authz::mcp`) as `dyn McpOperationGuard`
//! so callers are authenticated and the ambient `Ability` is installed.
//!
//! Every operation then declares its own posture, exactly as a `#[query]` or an
//! HTTP verb does:
//!
//! * `#[authorize(Action, Entity)]` — the class-level gate plus automatic
//!   field-level masking of the value you return. Reach for it whenever the
//!   answer is entity data, and return the `#[expose]`d wire type (`Json<T>`)
//!   so the mask has a shape to work on.
//! * `#[public]` — no gate and no mask. The endpoint still authenticated the
//!   caller; this says only that *this* operation has no entity to gate. Pair it
//!   with `#[use_guards(...)]` when the answer is a capability rather than a row.
//!
//! Arguments validate through the same pipes every other transport uses:
//! `Parameters<Valid<T>>` runs `validator` before the body and answers a
//! rejection with `invalid_params`, which is the one MCP error a model can act
//! on.
use std::sync::Arc;

// A fallible service call goes through `Opaque` — `use nest_rs::mcp::Opaque;`
// then `.opaque()?` — which logs the real error for the operator and hands the
// model a constant one. An error's `Display` reaches a language model verbatim.
use nest_rs::mcp::{McpError, mcp, tools};

use crate::{{snake}}::{{service}};

#[mcp]
#[derive(Clone)]
pub struct {{tool}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[tools]
impl {{tool}} {
    // The description is an argument, not a doc comment: it is the sentence a
    // language model reads to choose this tool, so it is a value the decorator
    // compiles in rather than prose a cleanup could delete. `#[tool]` accepts a
    // doc comment as a fallback; an operation with neither does not compile.
    #[tool(description = "{{op_description}}")]
    // This operation answers with a plain summary, so there is no entity to gate
    // or mask. The moment it returns rows, swap this for
    // `#[authorize(Read, <Entity>)]` and return the `#[expose]`d wire type
    // wrapped in `Json<...>`, which is what arms the field-level mask.
    #[public]
    async fn {{op}}(&self) -> Result<String, McpError> {
{{op_body}}
        Ok({{op_value}})
    }
}
"#;
