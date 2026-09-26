//! **Adapter** skeletons — one transport bolted onto an existing port
//! (`g http|graphql|ws|queue|schedule|mcp|events <feature>`).
//!
//! Each skeleton delegates to the port service's `count()` (the method
//! `g feature` emits) so a freshly-generated port + any adapter compiles
//! immediately. The handler is the seam the developer then fills in.
//!
//! **A `g resource` port has no `count()`** — its service is a `CrudService`
//! (`list`/`page`/`access`/`create`/`update`/`delete`), so a skeleton calling
//! `count()` on one does not compile. Each transport therefore renders one
//! template with the differing handler supplied as `{{op}}` / `{{op_body}}` /
//! `{{op_value}}` (see [`crud_vars`](super::crud::crud_vars)), rather than a second
//! near-identical blob: the scaffolding, imports and path conventions have one
//! home each. GraphQL is the exception that earns a second template
//! ([`GRAPHQL_RESOLVER_CRUD`]) — over a resource it is not a stub but the full
//! `#[crud]` block behind the app's guards.

/// `mod.rs` for an adapter folder: `mod <handler>; mod module;` + re-exports.
/// `{{handler_mod}}`/`{{handler}}`/`{{tmodule}}` are layered per transport.
pub(crate) const MOD: &str = r#"mod {{handler_mod}};
mod module;

pub use module::{{tmodule}};
"#;

/// Adapter `module.rs` — imports the port, provides the handler.
///
/// **Every adapter imports the port**, including the queue's: the moment the
/// generated stub grows the shape the docs prescribe — a thin processor handing
/// the job to the port service — the access graph fails the boot unless the port
/// module is already there. Scaffolding the import costs nothing (registration
/// is idempotent) and removes a boot error from the developer's first edit.
pub(crate) const MODULE: &str = r#"use nest_rs::core::module;

use super::{{handler_mod}}::{{handler}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{handler}}],
)]
pub struct {{tmodule}};
"#;

pub(crate) const HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

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
    // AuthzModule in this adapter's module.rs — `nestrs g auth` writes all
    // three, and `nestrs g http` on a `g resource` port emits them.
    #[get("/")]
    #[public]
    #[api(summary = "Count {{kebab}} items")]
    async fn list(&self) -> String {
        format!("{} items", self.svc.count())
    }
}
"#;

pub(crate) const GRAPHQL_RESOLVER: &str = r#"use std::sync::Arc;

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
    // (class gate + automatic response masking), bind #[use_guards(AuthzGuard)]
    // on the struct, and import AuthzGraphqlModule in this adapter's module.rs.
    // `/graphql` has no guard at the HTTP edge: that module's bridge is what
    // authenticates each operation, so AuthnGuard is not bound here — it has
    // no GraphQL check, and binding it is a compile error. `nestrs g graphql`
    // on a `g resource` port emits all three.
    #[query]
    #[public]
    async fn {{snake}}_count(&self) -> Result<usize> {
        Ok(self.svc.count())
    }
}
"#;

/// The GraphQL adapter for a **resource** port: the `#[crud]` resolver behind
/// the app's guards, the twin of `resource::HTTP_CONTROLLER` — same service,
/// same ability, same rows, one transport over.
///
/// **One guard on the struct, not two**, and the difference from the HTTP
/// controller is the transport's, not a shortcut: `/graphql` authenticates in
/// band, per operation, through the bridge `AuthzGraphqlModule` registers, and
/// `AuthnGuard` implements no `check_graphql` — so `#[resolver]` refuses it at
/// compile time rather than let it pass every operation. The demo's resolvers
/// bind `AuthzGuard` alone for the same reason. The two-guard form this
/// template carried failed that check from 6.0 on, unseen, because no e2e
/// compiled `g graphql` over a resource.
pub(crate) const GRAPHQL_RESOLVER_CRUD: &str = r#"use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

use crate::authz::AuthzGuard;
use crate::{{snake}}::{{{create_op}}, Entity as {{entity}}Entity, {{entity}}, {{service}}, {{update_op}}};

// `/graphql` has no guard at the HTTP edge: the bridge AuthzGraphqlModule
// registers authenticates each operation in band, so the resolver binds the
// ability guard alone — AuthnGuard has no GraphQL check, and binding it here is
// a compile error.
#[resolver]
#[use_guards(AuthzGuard)]
pub struct {{resolver}} {
    #[inject]
    svc: Arc<{{service}}>,
}

// Every operation declares #[authorize(Action, Entity)], so each is
// authenticated, ability-filtered, transactional and field-masked — generated
// by #[crud], the same way the HTTP controller is. They answer FORBIDDEN until
// AuthzAbility grants a rule for {{entity}}.
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
pub(crate) const WS_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::{{handler_mod}}::{{handler}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}, WsModule],
    providers = [{{handler}}],
)]
pub struct {{tmodule}};
"#;

pub(crate) const WS_GATEWAY: &str = r#"use std::sync::Arc;

use nest_rs::core::error_message;
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
            tracing::warn!(target: "features::{{snake}}", error = %error_message(&e), "broadcast failed");
        }
    }
}
"#;

pub(crate) const QUEUE_PROCESSOR: &str = r#"use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::queue::processor;

use crate::{{snake}}::{{{command}}, {{queue}}};

#[injectable]
#[derive(Default)]
pub struct {{processor}};

#[processor]
impl {{processor}} {
    // One attempt at a time per replica, the default: `concurrency = N` runs N
    // side by side in each replica, and more replicas scale the rest. `retries`
    // is the budget the port counts, backing off between attempts.
    #[process(queue = {{queue}}, retries = 3)]
    async fn handle(&self, job: {{command}}) -> Result<()> {
        let _ = job;
        Ok(())
    }
}
"#;

/// The queue payload **and its `#[queue]` marker** — both at the feature *port*,
/// not in the `queue/` adapter: they are the producer↔worker contract, and the
/// producer is usually the port's own service, one directory up. Keeping the
/// marker beside the payload is what makes the typed `push(Q, job, options)`
/// reachable; declaring it inside the private `queue::processor` module would
/// leave the untyped `push_json(name, value, options)` escape hatch as the only
/// way to push.
///
/// The default payload is a Command (the common case); rename it verb-led to
/// the real action, or switch to an `…Event` (past tense) when a fact is
/// published to several consumers.
pub(crate) const QUEUE_COMMAND: &str = r#"use nest_rs::queue::queue;
use serde::{Deserialize, Serialize};

/// Imperative payload for the `{{kebab}}` queue — "do this work", handled by one
/// processor. Rename it to the action it commands (e.g. `GenerateMediaVariantCommand`).
///
/// Plain `serde` derives rather than `#[input]`, and the difference is not
/// stylistic: `#[input]` carries `deny_unknown_fields`, which is right at an
/// edge a client controls and wrong on a producer↔worker contract. A producer
/// one deploy ahead that adds a field would have every job it pushes refused
/// by the older worker — and a decode failure is a `JobError::abort`, so the
/// job dead-letters on its first attempt instead of retrying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {{command}} {
    pub id: String,
}

/// The queue's identity — wire name + payload type in one artifact the producer
/// and the processor both import, so a typo or a mismatched payload is a compile
/// error rather than a job that silently never drains. Push with
/// `queue.push({{queue}}, job, None).await?` from a provider injecting
/// `queue: Arc<dyn JobProducer>`, with `JobProducerExt` in scope; it answers the
/// job's `PushReceipt`.
#[queue(name = "{{kebab}}", job = {{command}})]
pub struct {{queue}};
"#;

pub(crate) const SCHEDULE_TASKS: &str = r#"use std::sync::Arc;

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
    // Fires on every replica (`replicas = "each"`, the default), which suits
    // per-process work. Work that is the deployment's declares
    // `replicas = "one"`, and the app imports `RedisScheduleModule` beside
    // `ScheduleModule` (feature `redis-schedule`) so one replica claims each tick.
    #[every("60s")]
    async fn tick(&self) -> Result<()> {
{{op_body}}
        Ok(())
    }
}
"#;

pub(crate) const EVENTS_LISTENER: &str = r#"use nest_rs::core::injectable;
use nest_rs::events::listeners;

use crate::{{snake}}::{{event}};

#[injectable]
#[derive(Default)]
pub struct {{listener}};

#[listeners]
impl {{listener}} {
    #[on_event]
    async fn on_changed(&self, event: {{event}}) {
        let _ = event;
    }
}
"#;

/// The published fact the `events/` listener receives — at the feature *port*,
/// because the service that emits it and every listener import it. Past tense,
/// named for what happened.
pub(crate) const EVENTS_EVENT: &str = r#"/// A `{{kebab}}` fact, emitted on the event bus. Rename it to what happened
/// (e.g. `PostPublishedEvent`).
#[derive(Debug, Clone)]
pub struct {{event}} {
    pub id: String,
}
"#;

pub(crate) const MCP_TOOL: &str = r#"//! MCP tool for `{{snake}}`.
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
