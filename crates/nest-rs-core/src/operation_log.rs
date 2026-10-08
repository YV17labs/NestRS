//! One line per unit of work, and the vocabulary every edge files it with.
//!
//! A unit's canonical name is `<edge>.<unit>`, declared once as a [`Unit`] through
//! [`unit!`](crate::unit) by the crate that owns the edge, and read by
//! [`operation_span!`](crate::operation_span) and
//! [`operation_line!`](crate::operation_line).
//!
//! Field names on a line are flat, never dotted: `tracing`'s grammar is
//! ambiguous on a dotted name beside a path target such as [`TARGET`], so
//! `ws.event = %event` does not compile on the line.

use std::time::Instant;

/// The target every operation line is emitted on, whichever edge files it;
/// `nest_rs::operation=off` silences every edge's line at once.
///
/// It may not prefix another target: `EnvFilter` matches with `starts_with` on
/// the raw string (`.claude/decisions/operation-target.md`).
pub const TARGET: &str = "nest_rs::operation";

/// OpenTelemetry's `SpanKind`, which [`operation_span!`](crate::operation_span)
/// records as `otel.kind`; only the kinds this framework emits are declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Work this process accepted from somewhere else — an HTTP request, a WS
    /// message, an MCP operation, a GraphQL subscription.
    Server,
    /// Work taken off a queue and run here.
    Consumer,
    /// Work with no caller and no wire — a scheduled tick, an in-process event.
    Internal,
}

impl Kind {
    /// The word `otel.kind` is read as.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Consumer => "consumer",
            Self::Internal => "internal",
        }
    }
}

/// The closed edge vocabulary: the only namespaces a [`Unit`] may be declared
/// under.
pub const EDGES: [&str; 7] = [
    "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
];

/// One unit of work an edge opens: its canonical `<edge>.<unit>` name, the
/// target its span is emitted on, and its [`Kind`].
///
/// Declared only through [`unit!`](crate::unit); the accessors are `const` so
/// the reading macros can put them in `tracing`'s static callsite metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    name: &'static str,
    target: &'static str,
    kind: Kind,
}

impl Unit {
    /// Evaluated in a `const` by [`unit!`](crate::unit), which passes the
    /// declaring crate as `owner`.
    #[doc(hidden)]
    #[expect(
        clippy::panic,
        reason = "only ever evaluated in a const, where a panic is the compile error naming the refused fact"
    )]
    pub const fn __declare(
        owner: &str,
        target: &'static str,
        kind: Kind,
        name: &'static str,
    ) -> Self {
        let bytes = name.as_bytes();
        let mut i = 0;
        let mut dots = 0;
        let mut dot = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'a'..=b'z' | b'_' => {}
                b'.' => {
                    dots += 1;
                    dot = i;
                }
                _ => panic!("a unit name is lowercase ASCII words, `<edge>.<unit>`"),
            }
            i += 1;
        }
        if dots != 1 || dot == 0 || dot == bytes.len() - 1 {
            panic!("a unit name is `<edge>.<unit>`: one dot, a word on each side");
        }
        let mut known = false;
        let mut e = 0;
        while e < EDGES.len() {
            known |= EDGES[e].len() == dot && eq_bytes(EDGES[e].as_bytes(), 0, bytes, 0, dot);
            e += 1;
        }
        if !known {
            panic!(
                "a unit's edge is one of the closed edge vocabulary in \
                 `nest_rs_core::operation_log::EDGES`"
            );
        }
        if !is_edge_root(target, "nest_rs::", name) {
            panic!("a unit is emitted on its edge's target, `nest_rs::<edge>`");
        }
        if !is_edge_root(owner, "nest-rs-", name) {
            panic!("a unit is declared by the crate that owns its edge, `nest-rs-<edge>`");
        }
        Self { name, target, kind }
    }

    /// Whether `krate`, the crate a reading macro expands in, declared this unit.
    #[doc(hidden)]
    pub const fn __opened_by(&self, krate: &str) -> bool {
        is_edge_root(krate, "nest-rs-", self.name)
    }

    /// The canonical `<edge>.<unit>` name: the span's name, the line's `name:`
    /// and its `message`.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The edge's span target, `nest_rs::<edge>`.
    pub const fn target(&self) -> &'static str {
        self.target
    }

    /// OpenTelemetry's span kind for this unit.
    pub const fn kind(&self) -> Kind {
        self.kind
    }
}

/// `whole == prefix + <the edge of name>`; `name` must hold a dot.
const fn is_edge_root(whole: &str, prefix: &str, name: &str) -> bool {
    let (whole, prefix, name) = (whole.as_bytes(), prefix.as_bytes(), name.as_bytes());
    let mut edge = 0;
    while name[edge] != b'.' {
        edge += 1;
    }
    whole.len() == prefix.len() + edge
        && eq_bytes(whole, 0, prefix, 0, prefix.len())
        && eq_bytes(whole, prefix.len(), name, 0, edge)
}

/// `a[at_a..at_a + len] == b[at_b..at_b + len]`; the caller checks both lengths.
const fn eq_bytes(a: &[u8], at_a: usize, b: &[u8], at_b: usize, len: usize) -> bool {
    let mut i = 0;
    while i < len {
        if a[at_a + i] != b[at_b + i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The unit of work completed as asked.
pub const OK: &str = "ok";
/// It returned an error, or was refused. The edge's own fields say which.
pub const ERROR: &str = "error";
/// Developer code unwound.
pub const PANIC: &str = "panic";
/// It was stopped before it settled — by its caller, at the shutdown signal, or
/// when the shutdown window closed — and what it had not yet done stays undone.
///
/// No handler's return says it: an edge files it from a guard dropped with the
/// unit's future.
pub const CANCELLED: &str = "cancelled";

/// Record on a unit's span how the unit ended, where OpenTelemetry reads it.
///
/// Anything but [`OK`] becomes the span's `error.type` and an `Error` status.
/// [`operation_line!`](crate::operation_line) calls it; an edge calls it directly
/// only where it ends a unit without filing a line.
pub fn record_outcome(span: &tracing::Span, outcome: &str) {
    if outcome != OK {
        record_error(span, outcome);
    }
}

/// Record on a unit's span that it ended in an error of class `error_type`.
///
/// For a class finer than the outcome word: an HTTP `5xx` records its status
/// code, as OpenTelemetry's HTTP conventions ask.
pub fn record_error(span: &tracing::Span, error_type: &str) {
    span.record(ERROR_TYPE, error_type);
    span.record(STATUS_CODE, STATUS_ERROR);
}

const ERROR_TYPE: &str = "error.type";

const STATUS_CODE: &str = "otel.status_code";

/// `tracing-opentelemetry` reads it case-insensitively; the conventions spell it `Error`.
const STATUS_ERROR: &str = "error";

/// The field name every operation line files [`duration_ms`] under.
pub const DURATION_MS: &str = "duration_ms";

/// Digits after the decimal point when [`DURATION_MS`] is rendered for a human;
/// equal to [`duration_ms`]'s resolution.
pub const DURATION_DECIMALS: usize = 3;

/// Elapsed milliseconds at microsecond resolution.
pub fn duration_ms(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// Declare one [`Unit`] of the edge this crate owns —
/// `pub const REQUEST: Unit = nest_rs_core::unit!("http.request", target:
/// crate::target::HTTP, kind: Server);`, compiled in `nest_rs_http::unit`'s
/// example.
///
/// Refused at compile time: a name that is not lowercase `<edge>.<unit>`, an
/// edge outside [`EDGES`], a target other than `nest_rs::<edge>`, and a
/// declaring crate (`CARGO_PKG_NAME`) other than `nest-rs-<edge>`.
#[macro_export]
macro_rules! unit {
    ($name:literal, target: $target:expr, kind: $kind:ident $(,)?) => {{
        const __UNIT: $crate::operation_log::Unit = $crate::operation_log::Unit::__declare(
            ::core::env!("CARGO_PKG_NAME"),
            $target,
            $crate::operation_log::Kind::$kind,
            $name,
        );
        __UNIT
    }};
}

/// File the operation line of `$unit` — the one line per unit of work — and
/// record its outcome on the unit's span — compiled in `nest_rs_http::unit`'s
/// example.
///
/// `outcome` is one of [`OK`], [`ERROR`], [`PANIC`], [`CANCELLED`], or an
/// `Option` of one; the fields after `started` are the edge's own. A unit
/// another crate declared is refused at compile time.
///
/// Call it inside the unit's correlation (`RequestContinuation::enter`) and
/// outside its span, so the line carries the unit's ids and is nobody's child.
#[macro_export]
macro_rules! operation_line {
    ($unit:path, span: $span:expr, outcome: $outcome:expr, started: $started:expr $(, $($field:tt)*)?) => {{
        const _: () = ::core::assert!(
            $unit.__opened_by(::core::env!("CARGO_PKG_NAME")),
            "a unit's line is filed only by the crate that declares the unit",
        );
        let __outcome: ::core::option::Option<&'static str> = ::core::convert::Into::into($outcome);
        if let ::core::option::Option::Some(__outcome) = __outcome {
            $crate::operation_log::record_outcome($span, __outcome);
        }
        $crate::tracing::info!(
            name: $unit.name(),
            target: $crate::operation_log::TARGET,
            message = $unit.name(),
            outcome = __outcome,
            { $crate::operation_log::DURATION_MS } = $crate::operation_log::duration_ms($started),
            $($($field)*)?
        );
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_duration_keeps_microseconds_rather_than_rounding_to_zero() {
        let start = Instant::now();
        let elapsed = duration_ms(start);
        assert!(elapsed >= 0.0, "{elapsed}");
        assert!(elapsed < 1000.0, "a no-op cannot take a second: {elapsed}");
        assert_eq!(
            elapsed,
            (elapsed * 1000.0).round() / 1000.0,
            "resolution is the microsecond",
        );
    }

    #[test]
    fn the_span_kinds_are_spelled_as_otel_reads_them() {
        assert_eq!(Kind::Server.as_str(), "server");
        assert_eq!(Kind::Consumer.as_str(), "consumer");
        assert_eq!(Kind::Internal.as_str(), "internal");
    }

    #[test]
    fn the_outcome_words_are_distinct() {
        let words = [OK, ERROR, PANIC, CANCELLED];
        let distinct: std::collections::BTreeSet<&str> = words.into_iter().collect();
        assert_eq!(distinct.len(), words.len());
    }

    /// Run at run time so each refusal is asserted on its own; the compile-time
    /// refusal is `nest-rs-macro-hygiene`'s trybuild suite.
    fn declare(owner: &str, target: &'static str, name: &'static str) -> Result<Unit, String> {
        std::panic::catch_unwind(|| Unit::__declare(owner, target, Kind::Server, name))
            .map_err(|payload| crate::panic_message(payload.as_ref()))
    }

    #[test]
    fn every_edge_declares_a_unit_from_its_own_crate_on_its_own_target() {
        for edge in EDGES {
            let owner = format!("nest-rs-{edge}");
            let target: &'static str = format!("nest_rs::{edge}").leak();
            let name: &'static str = format!("{edge}.some_unit").leak();
            let unit = declare(&owner, target, name).expect("a well-formed unit");
            assert_eq!(
                (unit.name(), unit.target(), unit.kind()),
                (name, target, Kind::Server)
            );
            assert!(unit.__opened_by(&owner));
            assert!(!unit.__opened_by("nest-rs-redis"));
            assert!(!unit.__opened_by(&format!("{owner}-tests")));
        }
    }

    #[test]
    fn a_unit_off_the_grammar_is_refused_naming_the_fact() {
        for (owner, target, name, fact) in [
            ("nest-rs-queue", "nest_rs::queue", "Queue.job", "lowercase"),
            ("nest-rs-queue", "nest_rs::queue", "queue job", "lowercase"),
            ("nest-rs-queue", "nest_rs::queue", "queue", "one dot"),
            (
                "nest-rs-queue",
                "nest_rs::queue",
                "queue.job.attempt",
                "one dot",
            ),
            ("nest-rs-queue", "nest_rs::queue", ".job", "one dot"),
            ("nest-rs-queue", "nest_rs::queue", "queue.", "one dot"),
            (
                "nest-rs-grpc",
                "nest_rs::grpc",
                "grpc.call",
                "closed edge vocabulary",
            ),
            (
                "nest-rs-queue",
                "nest_rs::queu",
                "queue.job",
                "`nest_rs::<edge>`",
            ),
            (
                "nest-rs-queue",
                "nest_rs::queues",
                "queue.job",
                "`nest_rs::<edge>`",
            ),
            (
                "nest-rs-queue",
                "nest_rs::http",
                "queue.job",
                "`nest_rs::<edge>`",
            ),
            (
                "nest-rs-redis",
                "nest_rs::queue",
                "queue.job",
                "`nest-rs-<edge>`",
            ),
            (
                "nest-rs-queue-macros",
                "nest_rs::queue",
                "queue.job",
                "`nest-rs-<edge>`",
            ),
            (
                "nest-rs-http",
                "nest_rs::ws",
                "ws.message",
                "`nest-rs-<edge>`",
            ),
        ] {
            let refused = declare(owner, target, name).expect_err(name);
            assert!(
                refused.contains(fact),
                "{name} from {owner} on {target}: {refused}"
            );
        }
    }
}
