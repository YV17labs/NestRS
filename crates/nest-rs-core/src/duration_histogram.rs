//! The histogram a duration is recorded in — a unit's, a client operation's —
//! as its telemetry conventions declare it: its name, its description, the
//! bucket boundaries they advise and the attributes that key it.

use crate::operation_log::OTHER;

/// The bucket boundaries, in seconds, the HTTP and messaging conventions
/// advise for a duration.
pub const REQUEST_BOUNDARIES: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// A duration histogram, as its conventions declare it; `error.type` keys every
/// failed duration besides the attributes it names.
///
/// A [`Unit`](crate::operation_log::Unit) declares one with
/// [`unit!`](macro@crate::unit)'s `duration:`, keyed by fields of its span; a
/// [`ClientOperation`](crate::client_operation::ClientOperation) names one,
/// keyed by the attributes its driver hands over.
#[derive(Clone, Copy, Debug)]
pub struct DurationHistogram {
    name: &'static str,
    description: &'static str,
    boundaries: &'static [f64],
    attributes: &'static [MetricAttribute],
}

impl DurationHistogram {
    /// The histogram `name`, described by `description`, bucketed at
    /// `boundaries` seconds and keyed by `attributes`.
    pub const fn new(
        name: &'static str,
        description: &'static str,
        boundaries: &'static [f64],
        attributes: &'static [MetricAttribute],
    ) -> Self {
        Self {
            name,
            description,
            boundaries,
            attributes,
        }
    }

    /// The instrument's name, as the conventions spell it
    /// (`http.server.request.duration`).
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// What the instrument measures, as the conventions describe it.
    pub const fn description(&self) -> &'static str {
        self.description
    }

    /// The bucket boundaries, in seconds, the conventions advise.
    pub const fn boundaries(&self) -> &'static [f64] {
        self.boundaries
    }

    /// The attributes each recorded duration is keyed by.
    pub const fn attributes(&self) -> &'static [MetricAttribute] {
        self.attributes
    }
}

/// Equal declarations, boundaries compared bit for bit: they are constants,
/// never `NaN`.
impl PartialEq for DurationHistogram {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.description == other.description
            && self.attributes == other.attributes
            && self.boundaries.len() == other.boundaries.len()
            && self
                .boundaries
                .iter()
                .zip(other.boundaries)
                .all(|(one, another)| one.to_bits() == another.to_bits())
    }
}

impl Eq for DurationHistogram {}

/// One attribute a [`DurationHistogram`] is keyed by: a field of a unit's span,
/// or an attribute a client operation carries.
///
/// Each value of a metric attribute is a series a backend keeps, so only a
/// value from a set the server bounds keys one: a field holding what a client
/// sent is read through the set it may take, anything else as [`OTHER`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricAttribute {
    field: &'static str,
    known: Option<&'static [&'static str]>,
}

impl MetricAttribute {
    /// `field` as it is held: a value the server chose from a set it bounds —
    /// a route template, a method the conventions know, a status code.
    pub const fn bounded(field: &'static str) -> Self {
        Self { field, known: None }
    }

    /// `field` where its value is one of `known`, and [`OTHER`] otherwise: a
    /// field the conventions record as the client sent it.
    pub const fn known(field: &'static str, known: &'static [&'static str]) -> Self {
        Self {
            field,
            known: Some(known),
        }
    }

    /// The field, and the metric attribute's key.
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Whether a value keys a duration as it is held, the field being one the
    /// server bounds.
    pub const fn is_bounded(&self) -> bool {
        self.known.is_none()
    }

    /// For a field read through a known set, the member `value` equals, or
    /// [`OTHER`]; `None` for a [`bounded`](Self::bounded) field, whose value
    /// keys a duration as it is held.
    pub fn known_value(&self, value: &str) -> Option<&'static str> {
        let known = self.known?;
        Some(
            known
                .iter()
                .find(|member| **member == value)
                .copied()
                .unwrap_or(OTHER),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOOK: DurationHistogram = DurationHistogram::new(
        "lifecycle.hook.duration",
        "Duration of a hook.",
        REQUEST_BOUNDARIES,
        &[
            MetricAttribute::bounded("hook.phase"),
            MetricAttribute::known("hook.verb", &["read", "write"]),
        ],
    );

    #[test]
    fn a_bounded_attribute_keys_a_duration_with_its_own_value() {
        let [phase, _] = HOOK.attributes() else {
            panic!("two attributes");
        };
        assert!(phase.is_bounded());
        assert_eq!(phase.known_value("init"), None);
    }

    #[test]
    fn a_known_attribute_keys_a_value_outside_its_set_as_other() {
        let [_, verb] = HOOK.attributes() else {
            panic!("two attributes");
        };
        assert!(!verb.is_bounded());
        assert_eq!(verb.known_value("read"), Some("read"));
        assert_eq!(verb.known_value("anything-a-caller-sends"), Some(OTHER));
    }

    #[test]
    fn two_declarations_are_equal_by_every_part_their_boundaries_included() {
        let shorter = DurationHistogram::new(
            HOOK.name(),
            HOOK.description(),
            &REQUEST_BOUNDARIES[1..],
            HOOK.attributes(),
        );
        assert_eq!(HOOK, HOOK);
        assert_ne!(HOOK, shorter);
    }
}
