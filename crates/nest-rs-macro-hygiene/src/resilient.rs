//! `#[resilient]` — an `async fn` method's body run under the policy a field of
//! `self` holds, through the umbrella alone: the expansion reaches
//! `nest-rs-resilience`, so the umbrella's `resilience` feature must pull it.
//! Each key a method takes — its pins, its predicates, its fallback and its
//! token — and each host it decorates is a use site: a
//! provider, a struct built by hand through a trait's native `async fn`, and a
//! method of every impl half whose feature the build enables — the hooks',
//! the kernel's, under none. So is each predicate reading an argument its
//! method owns, run once or again, and a fallback handed arguments a
//! `macro_rules!` wrote apart from the decorator.

use std::sync::Arc;

use nest_rs::core::{Problem, ToProblem, injectable, instance};
use nest_rs::resilience::{CancellationToken, ResilienceError, ResiliencePolicy, resilient};

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

impl HygieneCarrierDown {
    /// Whether the carrier asked to be called again, which a predicate reads.
    pub fn busy(&self) -> bool {
        true
    }
}

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

    /// Each answer counted as its predicate says, one a function's path and
    /// two closures.
    #[resilient(
        carrier,
        idempotent,
        retry_if = HygieneCarrierDown::busy,
        retry_ok_if = |depot: &String| depot.is_empty(),
        record_if = |error| !error.busy(),
    )]
    pub async fn locate(&self, parcel: &str) -> HygieneAnswer<String> {
        depot(parcel)
    }

    /// A fallback handed the argument and the run's error, answering for the
    /// policy's refusals alone, the attempt's token read under the name given.
    #[resilient(
        carrier,
        fallback = last_known,
        fallback_if = |error| !matches!(error, ResilienceError::Failed(_)),
        cancel = token
    )]
    pub async fn weigh(&self, parcel: String) -> HygieneAnswer<usize> {
        if token.is_cancelled() {
            return Ok(0);
        }
        Ok(depot(&parcel)?.len())
    }

    /// The fallback of an `idempotent` method through the `Arc`, each attempt's
    /// token its own.
    #[resilient(carrier, idempotent, fallback = last_known, cancel = token)]
    pub async fn measure(self: &Arc<Self>, parcel: String) -> HygieneAnswer<usize> {
        let _attempt: CancellationToken = token;
        Ok(depot(&parcel)?.len())
    }

    async fn last_known(
        &self,
        parcel: String,
        error: ResilienceError<HygieneCarrierDown>,
    ) -> HygieneAnswer<usize> {
        match error {
            ResilienceError::Failed(failed) => Err(ResilienceError::Failed(failed)),
            _refused => Ok(parcel.len()),
        }
    }

    /// A predicate reading an argument the method owns, run once: the attempt
    /// borrows what the predicate reads once it answered.
    #[resilient(carrier, record_if = |_error| zone != "eu")]
    pub async fn quote(&self, zone: String) -> HygieneAnswer<usize> {
        Ok(zone.len())
    }

    /// Run once under a predicate, the body changing an argument bound `mut`
    /// and one taken by `&mut`, and consuming the first.
    #[resilient(carrier, record_if = HygieneCarrierDown::busy, cancel = token)]
    pub async fn restock(
        &self,
        mut order: String,
        crates: &mut Vec<String>,
    ) -> HygieneAnswer<usize> {
        if token.is_cancelled() {
            return Ok(0);
        }
        order.push('!');
        crates.push(order);
        std::future::ready(()).await;
        Ok(crates.len())
    }

    /// A filter reading the argument its fallback is handed next, by value.
    #[resilient(carrier, fallback = last_known, fallback_if = |_error| parcel != "none")]
    pub async fn estimate(&self, parcel: String) -> HygieneAnswer<usize> {
        Ok(parcel.len())
    }

    /// Run once, a predicate and a filter both reading what the fallback takes.
    #[resilient(
        carrier,
        record_if = |_error| !parcel.is_empty(),
        fallback = last_known,
        fallback_if = |_error| parcel != "none",
        cancel = token
    )]
    pub async fn reweigh(&self, parcel: String) -> HygieneAnswer<usize> {
        let _attempt: CancellationToken = token;
        Ok(parcel.len())
    }

    /// Sent again, every predicate reading an owned argument, the host and a
    /// generic one.
    #[resilient(
        carrier,
        idempotent,
        retry_if = |error| error.busy() && parcel != "none",
        retry_ok_if = |weight| *weight < parcel.len() + unit.as_ref().len(),
        record_if = |_error| Arc::strong_count(&self.carrier) > 0,
        fallback = last_known_in,
        fallback_if = |_error| parcel != "none" && !unit.as_ref().is_empty(),
    )]
    pub async fn remeasure<U>(&self, parcel: String, unit: U) -> HygieneAnswer<usize>
    where
        U: AsRef<str> + Send + Sync,
    {
        Ok(parcel.len() + unit.as_ref().len())
    }

    async fn last_known_in<U>(
        &self,
        parcel: String,
        unit: U,
        _error: ResilienceError<HygieneCarrierDown>,
    ) -> HygieneAnswer<usize>
    where
        U: AsRef<str> + Send + Sync,
    {
        Ok(parcel.len() + unit.as_ref().len())
    }

    /// A method compiled out takes its expansion with it.
    #[cfg(any())]
    #[resilient(carrier)]
    pub async fn compiled_out(&self) -> crate::does_not_exist::Answer {
        crate::does_not_exist::call()
    }
}

// The decorator and its keys are the macro's, the method's argument its caller's.
macro_rules! declare_fallback_keyed_here {
    ($method:ident($zone:ident)) => {
        impl HygieneResilientShipments {
            #[resilient(carrier, fallback = last_known, fallback_if = |_error| !$zone.is_empty())]
            pub async fn $method(&self, $zone: String) -> HygieneAnswer<usize> {
                Ok($zone.len())
            }
        }
    };
}

declare_fallback_keyed_here!(recount(parcel));

// The decorator and its keys are the caller's, the method's argument the macro's.
macro_rules! declare_fallback_keyed_there {
    ($(#[$decorator:meta])* $method:ident) => {
        impl HygieneResilientShipments {
            $(#[$decorator])*
            pub async fn $method(&self, parcel: String) -> HygieneAnswer<usize> {
                Ok(parcel.len())
            }
        }
    };
}

declare_fallback_keyed_there!(
    #[resilient(carrier, fallback = last_known)]
    recheck
);

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

/// A route and two `#[sse]` routes of a `#[routes]` block, sent once and again.
#[cfg(feature = "http")]
pub mod routes {
    use std::sync::Arc;

    use nest_rs::http::futures_util::stream;
    use nest_rs::http::{SseEvent, SseStream, controller, routes};
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

        #[sse("/feed")]
        #[public]
        #[resilient(upstream)]
        async fn feed(&self) -> HygieneAnswer<SseStream> {
            Err(HygieneCarrierDown)
        }

        #[sse("/arrivals")]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn arrivals(&self) -> HygieneAnswer<SseStream> {
            Ok(SseStream::new(stream::iter([SseEvent::message("arrived")])))
        }
    }
}

/// A root field and two subscriptions of an `#[operations]` block, sent once
/// and again.
#[cfg(feature = "graphql")]
pub mod operations {
    use std::sync::Arc;

    use nest_rs::graphql::async_graphql::futures_util::stream::{self, BoxStream};
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

        #[subscription]
        #[public]
        #[resilient(upstream)]
        async fn hygiene_feed(
            &self,
        ) -> Result<BoxStream<'static, i32>, ResilienceError<HygieneCarrierDown>> {
            Err(HygieneCarrierDown)
        }

        #[subscription]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn hygiene_arrivals(
            &self,
        ) -> Result<BoxStream<'static, i32>, ResilienceError<HygieneCarrierDown>> {
            Ok(Box::pin(stream::iter([1])))
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

/// A tool, a resource and its listing of a `#[tools]` block.
#[cfg(feature = "mcp")]
pub mod tools {
    use std::sync::Arc;

    use nest_rs::mcp::model::Resource;
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

        /// A parcel, read under the upstream's policy.
        #[resource(template = "hygiene-parcel://{id}")]
        #[public]
        #[resilient(upstream)]
        async fn parcel(&self, id: u32) -> HygieneAnswer<String> {
            Ok(format!("parcel {id}"))
        }

        #[resources]
        #[public]
        #[resilient(upstream, idempotent)]
        async fn parcels(&self) -> HygieneAnswer<Vec<Resource>> {
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

/// A tick of a `#[scheduled]` block, answering the policy's error through an
/// alias.
#[cfg(feature = "schedule")]
pub mod scheduled {
    use std::sync::Arc;

    use nest_rs::core::injectable;
    use nest_rs::resilience::{ResiliencePolicy, resilient};
    use nest_rs::schedule::scheduled;

    use super::HygieneAnswer;

    #[injectable]
    pub struct HygieneResilientTasks {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[scheduled]
    impl HygieneResilientTasks {
        #[every("1m")]
        #[resilient(upstream, idempotent)]
        async fn refresh(&self) -> HygieneAnswer<()> {
            Ok(())
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

/// An indicator of an `#[indicators]` block.
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
