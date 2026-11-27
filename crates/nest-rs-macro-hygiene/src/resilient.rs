//! `#[resilient]` — an `async fn` method's body run under the policy a field of
//! `self` holds, through the umbrella alone: the expansion reaches
//! `nest-rs-resilience`, so the umbrella's `resilience` feature must pull it.
//! Each key a method takes — its pins, its predicates, its fallback and its
//! token — and each host it decorates is a use site: a
//! provider, a struct built by hand through a trait's native `async fn`, and a
//! method of every impl half whose feature the build enables — the hooks',
//! the kernel's, under none. So is each predicate reading an argument its
//! method owns, run once or again, one its host holds as a value, a function
//! pointer or a boxed closure, an error that is no `std::error::Error`
//! held in one of the app's own, the way out of each borrow error a
//! predicate reading what its body changes earns, an argument a macro's
//! pattern binds, a parameter under a `#[cfg]` beside every key, a fallback
//! handed arguments a `macro_rules!` wrote apart from the decorator, a body
//! declaring items under the names its method answers in, in each form the
//! keys expand to, one opening on an inner attribute, an answer holding an
//! `impl Trait` in each shape a key changes, and a policy held in a field named
//! as a key is.

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

/// An error of the app's own holding one that is no `std::error::Error`
/// itself, a boxed one: what a decorated method answers in its place.
#[derive(Debug)]
pub struct HygieneDepotFailed(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for HygieneDepotFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the depot failed")
    }
}

impl std::error::Error for HygieneDepotFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for HygieneDepotFailed {
    fn from(failed: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self(failed)
    }
}

fn stocked(parcel: &str) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    Ok(parcel.parse::<usize>()?)
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

macro_rules! hygiene_unit {
    ($unit:ident) => {
        $unit
    };
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

    /// Run once, a predicate reading an argument the body would change: bound
    /// without `mut`, its copy changed, and read as the method was called.
    #[resilient(carrier, record_if = |_error| !order.is_empty())]
    pub async fn relabel(&self, order: String) -> HygieneAnswer<usize> {
        let mut order = order.clone();
        order.push('!');
        Ok(order.len())
    }

    /// Run once, the body writing through a `&mut` its predicate leaves alone,
    /// reading the argument the method takes for it.
    #[resilient(carrier, record_if = |_error| counted)]
    pub async fn reload(&self, crates: &mut Vec<String>, counted: bool) -> HygieneAnswer<usize> {
        crates.push(String::new());
        Ok(crates.len())
    }

    /// An argument a macro's pattern binds, under every key but `cancel` and
    /// `fallback`.
    #[resilient(carrier, idempotent, record_if = HygieneCarrierDown::busy)]
    pub async fn reweigh_in(&self, hygiene_unit!(grams): usize) -> HygieneAnswer<usize> {
        Ok(grams)
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

    /// A boxed error, which is no `std::error::Error`, held in one of the
    /// app's own that is: each `?` converts into it.
    #[resilient(carrier, idempotent)]
    pub async fn stock(&self, parcel: &str) -> Result<usize, ResilienceError<HygieneDepotFailed>> {
        Ok(stocked(parcel)?)
    }

    /// An `impl Trait` inside the answer, its hidden type inferred from the
    /// body as the undecorated method's is: run once, capturing nothing.
    #[resilient(carrier)]
    pub async fn label(&self, parcel: String) -> HygieneAnswer<impl std::fmt::Display + use<>> {
        let label = depot(&parcel)?;
        Ok(format!("{label}, labelled"))
    }

    /// Sent again with its token, an `impl Iterator` borrowing the host.
    #[resilient(carrier, idempotent, cancel = token)]
    pub async fn manifest(&self) -> HygieneAnswer<impl Iterator<Item = usize> + '_> {
        if token.is_cancelled() {
            return Err(HygieneCarrierDown);
        }
        Ok(std::iter::once(&self.carrier).map(Arc::strong_count))
    }

    /// Two in a tuple, under a predicate reading the argument its body reads.
    #[resilient(carrier, record_if = |_error| !parcel.is_empty())]
    pub async fn weigh_both(
        &self,
        parcel: String,
    ) -> HygieneAnswer<(impl std::fmt::Display + use<>, impl Clone + use<>)> {
        Ok((depot(&parcel)?, parcel.len()))
    }

    /// A parameter under a `#[cfg]`, plain or inside a `#[cfg_attr]`: handed
    /// to the fallback in the build that has it, and named in no other.
    #[resilient(carrier, fallback = last_routed, fallback_if = |_error| !zone.is_empty())]
    pub async fn route(
        &self,
        #[cfg(any())] absent: crate::does_not_exist::Zone,
        #[cfg(feature = "resilience")] zone: String,
        #[cfg_attr(feature = "resilience", cfg(any()))] unit: crate::does_not_exist::Unit,
        parcel: String,
    ) -> HygieneAnswer<usize> {
        Ok(zone.len() + parcel.len())
    }

    async fn last_routed(
        &self,
        #[cfg(any())] absent: crate::does_not_exist::Zone,
        #[cfg(feature = "resilience")] zone: String,
        #[cfg_attr(feature = "resilience", cfg(any()))] unit: crate::does_not_exist::Unit,
        parcel: String,
        _error: ResilienceError<HygieneCarrierDown>,
    ) -> HygieneAnswer<usize> {
        Ok(zone.len() + parcel.len())
    }

    /// Such a parameter under every other key: sent again, a predicate reading
    /// the one its build has, the token named beside it.
    #[resilient(
        carrier,
        idempotent,
        retry_if = |_error| !zone.is_empty(),
        record_if = |_error| !zone.is_empty(),
        cancel = token
    )]
    pub async fn reroute(
        &self,
        #[cfg(any())] absent: crate::does_not_exist::Zone,
        #[cfg(feature = "resilience")] zone: String,
    ) -> HygieneAnswer<usize> {
        let _attempt: CancellationToken = token;
        Ok(zone.len())
    }

    /// And run once under a predicate, its one attempt lending the arguments.
    #[resilient(carrier, record_if = |_error| !zone.is_empty())]
    pub async fn route_once(
        &self,
        #[cfg(any())] absent: crate::does_not_exist::Zone,
        #[cfg(feature = "resilience")] zone: String,
    ) -> HygieneAnswer<usize> {
        Ok(zone.len())
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

impl HygieneHandBuilt {
    /// A method and its fallback each named raw, the fallback's type parameter
    /// fixed by the argument it is handed.
    #[resilient(carrier, fallback = r#try)]
    pub async fn r#match(&self, parcel: &str) -> HygieneAnswer<usize> {
        Ok(depot(parcel)?.len())
    }

    async fn r#try<P: AsRef<str> + Send>(
        &self,
        parcel: P,
        _error: ResilienceError<HygieneCarrierDown>,
    ) -> HygieneAnswer<usize> {
        Ok(parcel.as_ref().len())
    }
}

/// What a quote answers, under the name a body's own struct takes.
pub struct HygieneQuote(pub usize);

/// A host whose bodies declare items under the names their methods answer in:
/// an item is in scope of its whole block, and the attempt's type is read
/// where the signature is.
pub struct HygieneShadowing {
    /// The policy each call runs under.
    pub carrier: Arc<ResiliencePolicy<HygieneCarrier>>,
}

impl HygieneShadowing {
    /// Sent once, the error's name imported as another's.
    #[resilient(carrier)]
    pub async fn parsed(&self, parcel: &str) -> Result<usize, ResilienceError<HygieneCarrierDown>> {
        use std::num::ParseIntError as HygieneCarrierDown;
        let parsed = parcel
            .parse()
            .map_err(|_unparsed: HygieneCarrierDown| crate::resilient::HygieneCarrierDown)?;
        Ok(parsed)
    }

    /// Sent again, `Result` imported as an alias of one parameter.
    #[resilient(carrier, idempotent)]
    pub async fn read(&self, parcel: &str) -> Result<usize, ResilienceError<HygieneCarrierDown>> {
        use std::io::Result;
        let read: Result<usize> = Ok(parcel.len());
        let read = read.map_err(|_unread| crate::resilient::HygieneCarrierDown)?;
        if read == 0 {
            return Err(crate::resilient::HygieneCarrierDown);
        }
        Ok(read)
    }

    /// Under a fallback, its token the closure's own `let`: a struct of the
    /// name of what the method answers.
    #[resilient(carrier, fallback = unquoted, cancel = token)]
    pub async fn quoted(
        &self,
        parcel: &str,
    ) -> Result<HygieneQuote, ResilienceError<HygieneCarrierDown>> {
        struct HygieneQuote(bool);
        let HygieneQuote(cancelled) = HygieneQuote(token.is_cancelled());
        if cancelled {
            return Err(HygieneCarrierDown);
        }
        Ok(crate::resilient::HygieneQuote(parcel.len()))
    }

    async fn unquoted(
        &self,
        parcel: &str,
        _error: ResilienceError<HygieneCarrierDown>,
    ) -> Result<HygieneQuote, ResilienceError<HygieneCarrierDown>> {
        Ok(HygieneQuote(parcel.len()))
    }

    /// Sent again under a token, the `let` naming it the attempt's own: the
    /// answer's alias declared as another type.
    #[resilient(carrier, idempotent, cancel = token)]
    pub async fn counted(&self, parcel: &str) -> HygieneAnswer<usize> {
        type HygieneAnswer<T> = Option<T>;
        let counted: HygieneAnswer<usize> = (!token.is_cancelled()).then_some(parcel.len());
        counted.ok_or(HygieneCarrierDown)
    }

    /// An inner attribute still opens the body, over every statement of it,
    /// and its items are in scope of them.
    #[resilient(carrier, idempotent)]
    pub async fn labelled(&self, parcel: &str) -> HygieneAnswer<usize> {
        #![expect(
            unused_variables,
            reason = "the witness: the expectation is met by a statement of the body"
        )]
        const LABEL: &str = "parcel";
        let unread = LABEL;
        Ok(parcel.len())
    }
}

/// A host holding its policy in a field named as a key is: the list's first
/// entry is the field, whatever its name.
pub struct HygieneFallbackProvider {
    /// The policy the fallback's own provider runs under.
    pub fallback: Arc<ResiliencePolicy<HygieneCarrier>>,
    /// A policy named as the one key written alone.
    pub idempotent: Arc<ResiliencePolicy<HygieneCarrier>>,
}

impl HygieneFallbackProvider {
    /// The field alone.
    #[resilient(fallback)]
    pub async fn stored(&self, parcel: &str) -> HygieneAnswer<usize> {
        Ok(depot(parcel)?.len())
    }

    /// The field, then the key of its name.
    #[resilient(fallback, idempotent, fallback = last_stored)]
    pub async fn restored(&self, parcel: &str) -> HygieneAnswer<usize> {
        Ok(depot(parcel)?.len())
    }

    async fn last_stored(
        &self,
        parcel: &str,
        _error: ResilienceError<HygieneCarrierDown>,
    ) -> HygieneAnswer<usize> {
        Ok(parcel.len())
    }

    /// A field named `idempotent`, the key written after it.
    #[resilient(idempotent, idempotent)]
    pub async fn resent(&self, parcel: &str) -> HygieneAnswer<usize> {
        Ok(depot(parcel)?.len())
    }
}

/// A host holding its predicates as values: function pointers, and a boxed
/// closure.
pub struct HygieneJudged {
    /// The policy each call runs under.
    pub carrier: Arc<ResiliencePolicy<HygieneCarrier>>,
    /// Which `Err` is sent again.
    pub sent_again: fn(&HygieneCarrierDown) -> bool,
    /// Which `Err` is a failure in the breaker.
    pub counted: Box<dyn Fn(&HygieneCarrierDown) -> bool + Send + Sync>,
    /// Which errors the fallback answers for.
    pub served: fn(&ResilienceError<HygieneCarrierDown>) -> bool,
}

impl HygieneJudged {
    /// Each predicate read off the host, by value or by reference.
    #[resilient(
        carrier,
        idempotent,
        retry_if = self.sent_again,
        record_if = &self.counted,
        fallback = unshipped,
        fallback_if = self.served,
    )]
    pub async fn ship(&self, parcel: String) -> HygieneAnswer<usize> {
        Ok(parcel.len())
    }

    async fn unshipped(
        &self,
        parcel: String,
        _error: ResilienceError<HygieneCarrierDown>,
    ) -> HygieneAnswer<usize> {
        Ok(parcel.len())
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
/// and again; the decorator under another name and under a condition, which
/// `#[operations]` leaves on the method like any attribute; and a subscription
/// under a posture naming a subject, answering a stream a type names.
#[cfg(feature = "graphql")]
pub mod operations {
    use std::sync::Arc;

    use nest_rs::graphql::async_graphql::futures_util::stream::{self, BoxStream};
    use nest_rs::graphql::{operations, resolver};
    use nest_rs::resilience::resilient as hygiene_shielded;
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

        #[query]
        #[public]
        #[hygiene_shielded(upstream)]
        async fn hygiene_renamed(&self) -> Result<String, ResilienceError<HygieneCarrierDown>> {
            Err(HygieneCarrierDown)
        }

        #[mutation]
        #[public]
        #[cfg_attr(all(), cfg_attr(all(), hygiene_shielded(upstream, idempotent)))]
        async fn hygiene_conditional(&self) -> Result<String, ResilienceError<HygieneCarrierDown>> {
            Err(HygieneCarrierDown)
        }

        /// A condition that does not hold leaves the method undecorated: its
        /// body answers the method's own type.
        #[query]
        #[public]
        #[cfg_attr(any(), resilient(upstream))]
        async fn hygiene_unconditional(
            &self,
        ) -> Result<String, ResilienceError<HygieneCarrierDown>> {
            Ok(String::new())
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

        #[cfg(feature = "authz")]
        #[subscription]
        #[authorize(nest_rs::authz::Read, crate::subject::HygieneExports)]
        #[resilient(upstream)]
        async fn hygiene_gated_feed(
            &self,
        ) -> Result<BoxStream<'static, i32>, ResilienceError<HygieneCarrierDown>> {
            Err(HygieneCarrierDown)
        }

        #[cfg(feature = "authz")]
        #[subscription]
        #[public(nest_rs::authz::Read, crate::subject::HygieneExports)]
        #[resilient(upstream, idempotent)]
        async fn hygiene_gated_arrivals(
            &self,
        ) -> Result<stream::Iter<std::ops::Range<i32>>, ResilienceError<HygieneCarrierDown>>
        {
            Ok(stream::iter(0..3))
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
