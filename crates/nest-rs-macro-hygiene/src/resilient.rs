//! `#[resilient]` — an `async fn` method's body run under the policy a field of
//! `self` holds, through the umbrella alone: the expansion reaches
//! `nest-rs-resilience`, so the umbrella's `resilience` feature must pull it.
//! Each key a method takes and each host it decorates is a use site: a
//! provider, a struct built by hand through a trait's native `async fn`, and a
//! method of every impl half whose feature the build enables — the hooks',
//! the kernel's, under none — but `#[scheduled]`'s, whose tick answers
//! `anyhow::Result<()>` and so calls a decorated method rather than being one.

use std::sync::Arc;

use nest_rs::core::{Problem, ToProblem, injectable, instance};
use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

#[instance]
pub struct HygieneCarrier;

/// What the carrier answers when it fails: an error whose chain a retry reads.
#[derive(Debug)]
pub struct HygieneCarrierDown;

impl std::fmt::Display for HygieneCarrierDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the carrier did not answer")
    }
}

impl std::error::Error for HygieneCarrierDown {}

impl ToProblem for HygieneCarrierDown {
    fn to_problem(&self) -> Option<Problem> {
        None
    }
}

/// What a decorated method answers, through an alias: the probe reads it by
/// its type.
pub type HygieneAnswer<T> = Result<T, ResilienceError<HygieneCarrierDown>>;

fn depot(parcel: &str) -> Result<String, HygieneCarrierDown> {
    if parcel.is_empty() {
        return Err(HygieneCarrierDown);
    }
    Ok(parcel.to_owned())
}

#[injectable]
pub struct HygieneResilientShipments {
    #[inject]
    carrier: Arc<ResiliencePolicy<HygieneCarrier>>,
    #[inject]
    upstream: Arc<ResiliencePolicy>,
}

impl HygieneResilientShipments {
    /// Sent once, consuming what it was given.
    #[resilient(carrier)]
    pub async fn ship(&self, mut order: String) -> HygieneAnswer<usize> {
        order.push('!');
        Ok(order.len())
    }

    /// Sent again, `?` and an early `return` answering one attempt.
    #[resilient(carrier, idempotent)]
    pub async fn track(&self, parcel: &str) -> Result<String, ResilienceError<HygieneCarrierDown>> {
        if parcel == "none" {
            return Ok(String::new());
        }
        let depot = depot(parcel)?;
        Ok(depot)
    }

    /// Every key, through the `Arc` the container holds the host in.
    #[resilient(carrier, idempotent, timeout = "2s", retries = 3)]
    pub async fn rates(self: &Arc<Self>, zone: &str) -> HygieneAnswer<usize> {
        Ok(zone.len())
    }

    /// A generic method on the default instance.
    #[resilient(upstream, timeout = "500ms")]
    pub async fn preview<T: Default + Send>(&self) -> HygieneAnswer<T> {
        Ok(T::default())
    }

    /// A method compiled out takes its expansion with it.
    #[cfg(any())]
    #[resilient(carrier)]
    pub async fn compiled_out(&self) -> crate::does_not_exist::Answer {
        crate::does_not_exist::call()
    }
}

/// A call a trait declares, its future `Send`.
pub trait HygieneTracking {
    /// Where a parcel stands.
    fn status(
        &self,
        parcel: &str,
    ) -> impl std::future::Future<Output = HygieneAnswer<String>> + Send;
}

/// A host the app builds itself, holding a policy the container built.
pub struct HygieneHandBuilt {
    /// The policy each call runs under.
    pub carrier: Arc<ResiliencePolicy<HygieneCarrier>>,
}

impl HygieneTracking for HygieneHandBuilt {
    #[resilient(carrier, idempotent)]
    async fn status(&self, parcel: &str) -> HygieneAnswer<String> {
        depot(parcel)
    }
}

/// A route of a `#[routes]` block.
#[cfg(feature = "http")]
pub mod routes {
    use std::sync::Arc;

    use nest_rs::http::{controller, routes};
    use nest_rs::resilience::{ResiliencePolicy, resilient};

    use super::{HygieneAnswer, HygieneCarrierDown};

    #[controller(path = "/hygiene/resilient")]
    pub struct HygieneResilientController {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[routes]
    impl HygieneResilientController {
        #[get("/")]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn preview(&self) -> HygieneAnswer<String> {
            Err(HygieneCarrierDown)
        }
    }
}

/// A root field of an `#[operations]` block.
#[cfg(feature = "graphql")]
pub mod operations {
    use std::sync::Arc;

    use nest_rs::graphql::{operations, resolver};
    use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};

    use super::HygieneCarrierDown;

    #[resolver]
    pub struct HygieneResilientResolver {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    /// `#[operations]` reads a field's `Result` by its spelling, so the answer
    /// is written out rather than through an alias.
    #[operations]
    impl HygieneResilientResolver {
        #[query]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn hygiene_preview(&self) -> Result<String, ResilienceError<HygieneCarrierDown>> {
            Err(HygieneCarrierDown)
        }
    }
}

/// A message of a `#[messages]` block.
#[cfg(feature = "ws")]
pub mod messages {
    use std::sync::Arc;

    use nest_rs::resilience::{ResiliencePolicy, resilient};
    use nest_rs::ws::{gateway, messages};

    use super::{HygieneAnswer, HygieneCarrierDown};

    #[gateway(path = "/hygiene/resilient")]
    pub struct HygieneResilientGateway {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[messages]
    impl HygieneResilientGateway {
        #[subscribe_message("preview")]
        #[public]
        #[resilient(upstream)]
        async fn preview(&self) -> HygieneAnswer<String> {
            Err(HygieneCarrierDown)
        }
    }
}

/// A tool of a `#[tools]` block.
#[cfg(feature = "mcp")]
pub mod tools {
    use std::sync::Arc;

    use nest_rs::mcp::{mcp, tools};
    use nest_rs::resilience::{ResiliencePolicy, resilient};

    use super::{HygieneAnswer, HygieneCarrierDown};

    #[mcp(path = "/mcp/hygiene-resilient")]
    #[derive(Clone)]
    pub struct HygieneResilientTools {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[tools]
    impl HygieneResilientTools {
        /// Preview a link under the upstream's policy.
        #[tool]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn preview(&self) -> HygieneAnswer<String> {
            Err(HygieneCarrierDown)
        }
    }
}

/// A job of a `#[processor]` block.
#[cfg(feature = "queue")]
pub mod processor {
    use std::sync::Arc;

    use nest_rs::core::injectable;
    use nest_rs::queue::{processor, queue};
    use nest_rs::resilience::{ResiliencePolicy, resilient};

    use super::{HygieneAnswer, HygieneCarrierDown};

    #[queue(name = "hygiene-resilient", job = String)]
    pub struct HygieneResilientQueue;

    #[injectable]
    pub struct HygieneResilientProcessor {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[processor]
    impl HygieneResilientProcessor {
        #[process(queue = HygieneResilientQueue)]
        #[resilient(upstream)]
        async fn refresh(&self, url: String) -> HygieneAnswer<()> {
            drop(url);
            Err(HygieneCarrierDown)
        }
    }
}

/// A listener of a `#[listeners]` block.
#[cfg(feature = "events")]
pub mod listeners {
    use std::sync::Arc;

    use nest_rs::core::injectable;
    use nest_rs::events::listeners;
    use nest_rs::resilience::{ResiliencePolicy, resilient};

    use super::{HygieneAnswer, HygieneCarrierDown};

    /// A bus event the listener forwards to the upstream.
    #[derive(Clone)]
    pub struct HygieneShipped;

    #[injectable]
    pub struct HygieneResilientListener {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[listeners]
    impl HygieneResilientListener {
        #[on_event]
        #[resilient(upstream)]
        async fn on_shipped(&self, event: HygieneShipped) -> HygieneAnswer<()> {
            let HygieneShipped = event;
            Err(HygieneCarrierDown)
        }
    }
}

/// A probe of an `#[indicators]` block.
#[cfg(feature = "health")]
pub mod indicators {
    use std::sync::Arc;

    use nest_rs::core::injectable;
    use nest_rs::health::indicators;
    use nest_rs::resilience::{ResiliencePolicy, resilient};

    use super::HygieneAnswer;

    #[injectable]
    pub struct HygieneResilientIndicator {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[indicators]
    impl HygieneResilientIndicator {
        #[readiness]
        #[resilient(upstream, idempotent, timeout = "1s")]
        async fn upstream_reachable(&self) -> HygieneAnswer<()> {
            Ok(())
        }
    }
}

/// A hook of a `#[hooks]` block, the kernel's, under no feature.
pub mod hooks {
    use std::sync::Arc;

    use nest_rs::core::{hooks, injectable};
    use nest_rs::resilience::{ResiliencePolicy, resilient};

    use super::HygieneAnswer;

    #[injectable]
    pub struct HygieneResilientWarmer {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[hooks]
    impl HygieneResilientWarmer {
        #[on_module_init]
        #[resilient(upstream)]
        async fn warm(&self) -> HygieneAnswer<()> {
            Ok(())
        }
    }
}
