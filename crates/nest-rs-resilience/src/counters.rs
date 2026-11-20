//! The counters a policy records on — its breaker's transitions, its retries
//! and the refusals its runs end on — each made on the global meter at its first
//! use, so a policy built before `OpenTelemetry::init()` still counts. Without
//! the `opentelemetry` feature a record is nothing.

use crate::error::Refusal;

/// Each transition of a breaker, by the state it entered.
#[cfg(feature = "opentelemetry")]
pub(crate) const TRANSITIONS: &str = "nestrs.resilience.breaker.transitions";

/// Each call a policy sent again.
#[cfg(feature = "opentelemetry")]
pub(crate) const RETRIES: &str = "nestrs.resilience.retries";

/// Each run a policy ended on its own refusal, by the refusal.
#[cfg(feature = "opentelemetry")]
pub(crate) const REFUSALS: &str = "nestrs.resilience.refusals";

#[cfg(feature = "opentelemetry")]
pub(crate) const INSTANCE_ATTRIBUTE: &str = "nestrs.resilience.instance";

#[cfg(feature = "opentelemetry")]
pub(crate) const STATE_ATTRIBUTE: &str = "nestrs.resilience.breaker.state";

#[cfg(feature = "opentelemetry")]
pub(crate) const REFUSAL_ATTRIBUTE: &str = "nestrs.resilience.refusal";

/// One policy's counters, by its instance.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    #[cfg(feature = "opentelemetry")]
    instance: &'static str,
    #[cfg(feature = "opentelemetry")]
    transitions: Lazy,
    #[cfg(feature = "opentelemetry")]
    retries: Lazy,
    #[cfg(feature = "opentelemetry")]
    refusals: Lazy,
}

/// A counter made at its first use.
#[cfg(feature = "opentelemetry")]
type Lazy = std::sync::OnceLock<opentelemetry::metrics::Counter<u64>>;

impl Counters {
    pub(crate) fn new(instance: &'static str) -> Self {
        #[cfg(not(feature = "opentelemetry"))]
        let _ = instance;
        Self {
            #[cfg(feature = "opentelemetry")]
            instance,
            ..Self::default()
        }
    }

    pub(crate) fn transition(&self, state: &'static str) {
        #[cfg(feature = "opentelemetry")]
        self.add(
            &self.transitions,
            TRANSITIONS,
            "{transition}",
            "The circuit breakers' transitions, by the state each entered.",
            Some((STATE_ATTRIBUTE, state)),
        );
        #[cfg(not(feature = "opentelemetry"))]
        let _ = state;
    }

    pub(crate) fn retry(&self) {
        #[cfg(feature = "opentelemetry")]
        self.add(
            &self.retries,
            RETRIES,
            "{retry}",
            "The calls the policies sent again.",
            None,
        );
    }

    pub(crate) fn refusal(&self, refusal: Refusal) {
        #[cfg(feature = "opentelemetry")]
        self.add(
            &self.refusals,
            REFUSALS,
            "{refusal}",
            "The runs the policies ended on their own refusal, by the refusal.",
            Some((REFUSAL_ATTRIBUTE, refusal.as_str())),
        );
        #[cfg(not(feature = "opentelemetry"))]
        let _ = refusal;
    }

    #[cfg(feature = "opentelemetry")]
    fn add(
        &self,
        counter: &Lazy,
        name: &'static str,
        unit: &'static str,
        description: &'static str,
        attribute: Option<(&'static str, &'static str)>,
    ) {
        let instance = opentelemetry::KeyValue::new(INSTANCE_ATTRIBUTE, self.instance);
        let counter = counter.get_or_init(|| {
            opentelemetry::global::meter("nestrs")
                .u64_counter(name)
                .with_unit(unit)
                .with_description(description)
                .build()
        });
        match attribute {
            Some((key, value)) => {
                counter.add(1, &[instance, opentelemetry::KeyValue::new(key, value)])
            }
            None => counter.add(1, &[instance]),
        }
    }
}
