//! **Adapter** skeletons — one transport bolted onto an existing port
//! (`g http|graphql|ws|queue|schedule|mcp|events <feature>`).
//!
//! Each skeleton delegates to the port service's `count()` (the method
//! `g feature` emits). A `g resource` port's `CrudService` has none, so each
//! transport takes the differing handler as `{{op}}` / `{{op_body}}` /
//! `{{op_value}}` ([`crud_vars`](super::crud::crud_vars)); GraphQL alone gets a
//! second template ([`GRAPHQL_RESOLVER_CRUD`]), the full `#[crud]` block.

/// `mod.rs` for an adapter folder: `mod <handler>; mod module;` + re-exports.
/// `{{handler_mod}}`/`{{handler}}`/`{{tmodule}}` are layered per transport.
pub(crate) const MOD: &str = r#"mod {{handler_mod}};
mod module;

pub use module::{{tmodule}};
"#;

/// Adapter `module.rs` — imports the port, provides the handler, whose edge
/// decorator brings what serves it, so the app importing the adapter serves it
/// with no import of its own; only the app pins one.
///
/// **Every adapter imports the port**, the queue's included: a processor
/// handing the job to the port service would otherwise fail the access graph.
pub(crate) const MODULE: &str = r#"use nest_rs::core::module;

use super::{{handler_mod}}::{{handler}};
use crate::{{snake}}::{{module}};

#[module(
    imports = [{{module}}],
    providers = [{{handler}}],
)]
pub struct {{tmodule}};
"#;

/// The HTTP adapter for a port with no entity: guarded like its `#[crud]`
/// twin, so the route answers only a caller the two guards admit.
pub(crate) const HTTP_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::http::{controller, routes};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::{{snake}}::{{service}};

#[controller(path = "/{{kebab}}")]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct {{controller}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[routes]
impl {{controller}} {
    // Guarded by the two guards on the struct, so it needs no posture of its
    // own; the boot refuses a route no guard covers. A route open on purpose
    // says #[public]; one serving rows declares #[authorize(Action, Entity)].
    #[get("/")]
    #[api(summary = "Count {{kebab}} items")]
    async fn list(&self) -> String {
        format!("{} items", self.svc.count())
    }
}
"#;

/// The note above a single operation gated on the port's entity: the grant it
/// waits for, by the path `authz/ability.rs` resolves.
pub(crate) const GATED_ON_ENTITY: &str =
    "    // Gated on the port's entity: it answers FORBIDDEN until AuthzAbility grants
    // `ab.can(nest_rs::authz::Action::Read, crate::{{snake}}::Entity)`.";

/// The MCP tool's addition over an entity: rmcp needs the typed value back, so
/// the mask works on the wire type the tool returns.
pub(crate) const ROWS_MASKED_AS_JSON: &str =
    "    // An answer made of its rows returns the `#[expose]`d wire type wrapped in
    // `Json<...>`, which is what arms the field-level mask.";

/// The note above a single operation gated on the port's `#[subject]`, which
/// masks nothing: rows wait for the posture on their entity.
pub(crate) const GATED_ON_SUBJECT: &str =
    "    // Gated on {{entity}}, this port's #[subject]: a capability no table holds, so
    // the posture masks nothing. It answers FORBIDDEN until AuthzAbility grants
    // `ab.can(nest_rs::authz::Action::Read, crate::{{snake}}::{{entity}})`. Before it
    // answers with an entity's rows, it is re-gated on that entity,
    // `#[authorize(Read, {{entity}}Entity)]`, the posture that masks them.";

/// The GraphQL adapter for a port with no `CrudService`: guarded like its
/// `#[crud]` twin, its operation gated on `{{subject}}` — the port's entity, or
/// the `#[subject]` the generator writes for a port with none.
pub(crate) const GRAPHQL_RESOLVER: &str = r#"use std::sync::Arc;

use async_graphql::Result;
use nest_rs::authz::Read;
use nest_rs::graphql::{operations, resolver};

use crate::authz::AuthzGuard;
use crate::{{snake}}::{{{subject_use}}, {{service}}};

// `/graphql` authenticates at its request, through the guards
// AuthzGraphqlModule declares for it, so the resolver binds the ability guard
// alone — AuthnGuard has no GraphQL check, and binding it here is a compile
// error.
#[resolver]
#[use_guards(AuthzGuard)]
pub struct {{resolver}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[operations]
impl {{resolver}} {
{{posture_note}}
    #[query]
    #[authorize(Read, {{subject}})]
    async fn {{snake}}_count(&self) -> Result<usize> {
        Ok(self.svc.count())
    }
}
"#;

/// The GraphQL adapter for a **resource** port: the `#[crud]` resolver behind
/// the app's guards, the twin of `resource::HTTP_CONTROLLER`.
///
/// **One guard on the struct**: `/graphql` authenticates at its request through
/// the guards `AuthzGraphqlModule` declares, and `AuthnGuard` declares no
/// `GuardFor<GraphqlView>`, so `#[resolver]` refuses it at compile time.
pub(crate) const GRAPHQL_RESOLVER_CRUD: &str = r#"use std::sync::Arc;

use nest_rs::graphql::{crud, resolver};

use crate::authz::AuthzGuard;
use crate::{{snake}}::{{service}};

// `/graphql` authenticates at its request, through the guards
// AuthzGraphqlModule declares for it, so the resolver binds the ability guard
// alone — AuthnGuard has no GraphQL check, and binding it here is a compile
// error.
#[resolver]
#[use_guards(AuthzGuard)]
pub struct {{resolver}} {
    #[inject]
    svc: Arc<{{service}}>,
}

// Every operation declares #[authorize(Action, Entity)], so each is
// authenticated, ability-filtered, transactional and field-masked — generated
// by #[crud], the same way the HTTP controller is. Each answers FORBIDDEN until
// AuthzAbility grants its action on `crate::{{snake}}::Entity`.
#[crud(service = {{service}})]
impl {{resolver}} {}
"#;

pub(crate) const WS_GATEWAY: &str = r#"use std::sync::Arc;

use nest_rs::core::error_message;
use nest_rs::ws::{WsClient, gateway, messages};

use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::{{snake}}::{{service}};

// The path carries the feature name: a self-mount path is its exclusive
// namespace, so a bare `/ws` made the *second* generated gateway fail boot on a
// duplicate path. `/ws/<feature>` also stays clear of the HTTP controller
// adapter, which claims `/<feature>`. The guards run at the upgrade, so the
// socket opens only for a caller they admit.
#[gateway(path = "/ws/{{kebab}}")]
#[use_guards(AuthnGuard, AuthzGuard)]
pub struct {{gateway}} {
    #[inject]
    svc: Arc<{{service}}>,
}

#[messages]
impl {{gateway}} {
    // Covered by the two guards on the struct, which ran at the upgrade, so it
    // needs no posture of its own; the boot refuses a message no guard covers.
    // A message answering with entity rows declares
    // `#[authorize(Read, Entity)]`, and the mask comes with it.
    #[subscribe_message("{{kebab}}.{{op}}")]
    async fn {{op}}(&self, client: &WsClient) {
{{op_body}}
        // Best-effort fan-out: a peer disconnecting mid-broadcast is normal, so
        // a delivery failure is logged rather than propagated (this handler
        // returns `()` — there is no client left to surface an error to).
        if let Err(e) = client.broadcast("{{kebab}}.{{op}}", {{op_value}}) {
            tracing::warn!(target: crate::{{snake}}::TARGET, error = %error_message(&e), "broadcast failed");
        }
    }
}
"#;

pub(crate) const QUEUE_PROCESSOR: &str = r#"use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::queue::processor;

use crate::{{snake}}::{{{command}}, {{queue}}};

#[injectable]
pub struct {{processor}};

#[processor]
impl {{processor}} {
    // One attempt at a time per replica, the default: `concurrency = N` runs N
    // side by side in each replica, and more replicas scale the rest. `retries`
    // is the budget the port counts, backing off between attempts; an `Err`
    // carrying a `Problem` with a 4xx other than 408 and 429 dead-letters the
    // job at once, since no retry clears a client's error.
    #[process(queue = {{queue}}, retries = 3)]
    async fn handle(&self, job: {{command}}) -> Result<()> {
        let _ = job;
        Ok(())
    }
}
"#;

/// The queue payload **and its `#[queue]` marker**, at the feature *port*: they
/// are the producer↔worker contract, and the marker outside the private
/// `queue::processor` keeps the typed `push(Q, job, options)` reachable.
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
/// `jobs.push({{queue}}, job).await?` from a provider injecting
/// `jobs: Arc<Jobs>`; it answers the job's `PushReceipt`.
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
    // `replicas = "one"`, and the app imports `RedisScheduleModule` (feature
    // `redis-schedule`) so one replica claims each tick.
    #[every("60s")]
    async fn tick(&self) -> Result<()> {
{{op_body}}
        Ok(())
    }
}
"#;

pub(crate) const EVENTS_LISTENER: &str = r#"use anyhow::Result;
use nest_rs::core::injectable;
use nest_rs::events::listeners;

use crate::{{snake}}::{{event}};

#[injectable]
pub struct {{listener}};

#[listeners]
impl {{listener}} {
    // Runs once the emitter's transaction has committed, as a unit of work of
    // its own: an `Err` rolls back what it wrote and is logged at `error`.
    // Cut after ten minutes unless `#[on_event(timeout = "…")]` says otherwise.
    #[on_event]
    async fn on_changed(&self, event: {{event}}) -> Result<()> {
        let _ = event;
        Ok(())
    }
}
"#;

/// The published fact the `events/` listener receives — at the feature *port*,
/// where the emitting service and every listener import it.
pub(crate) const EVENTS_EVENT: &str = r#"/// A `{{kebab}}` fact, emitted on the event bus. Rename it to what happened
/// (e.g. `PostPublishedEvent`).
#[derive(Debug, Clone)]
pub struct {{event}} {
    pub id: String,
}
"#;

pub(crate) const MCP_TOOL: &str = r#"//! MCP tool for `{{snake}}`.
//!
//! Security: the MCP endpoint gates at its request, through the global guard
//! pool (`GlobalGuards`) and the `EndpointGuards<McpView, _>` a module
//! declares, either alone, else deny-all. Import `AuthzMcpModule`
//! (`features::authz::mcp`) so callers are authenticated and the ambient
//! `Ability` is attached.
//!
//! An operation those guards cover needs no posture of its own, as an HTTP route
//! under a guard needs none. Where it gates more, it says so:
//!
//! * `#[authorize(Action, Entity)]` — the class-level gate plus automatic
//!   field-level masking of the value you return. Reach for it whenever the
//!   answer is entity data, and return the `#[expose]`d wire type (`Json<T>`)
//!   so the mask has a shape to work on.
//! * `#[authorize(Action, Subject)]` on a `#[subject]` — the same gate on a
//!   capability no table holds, and no mask: the answer is no row.
//!
//! `#[public]` admits a caller with no credential, so it belongs on an endpoint
//! `McpAccess::Public` opens, where every operation states its posture; behind
//! guards that authenticate it can open nothing, and the boot refuses it.
//!
//! Arguments are held to the rules their type declares, as on every other
//! transport: `Parameters<T>` runs `validator` before the body and answers a
//! rejection with `invalid_params`, which is the one MCP error a model can act
//! on.
use std::sync::Arc;

// A fallible service call goes through `Opaque` — `use nest_rs::mcp::Opaque;`
// then `.opaque()?` — which logs the real error for the operator and hands the
// model a constant one. An error's `Display` reaches a language model verbatim.
use nest_rs::authz::Read;
use nest_rs::mcp::{McpError, mcp, tools};

use crate::{{snake}}::{{{subject_use}}, {{service}}};

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
{{posture_note}}
    #[authorize(Read, {{subject}})]
    async fn {{op}}(&self) -> Result<String, McpError> {
{{op_body}}
        Ok({{op_value}})
    }
}
"#;
