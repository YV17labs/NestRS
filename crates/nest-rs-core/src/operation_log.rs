//! One line per unit of work, and the vocabulary every edge files it with.
//!
//! A unit's canonical name is `<edge>.<unit>` for a framework edge's,
//! `lifecycle.<unit>` for the kernel's and `<crate>.<unit>` for any other
//! crate's, declared once as a [`Unit`] through [`unit!`](crate::unit) by the
//! crate it belongs to, and read by [`operation_span!`](crate::operation_span)
//! and [`operation_line!`](crate::operation_line).
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

/// A framework edge: the closed vocabulary a framework [`Unit`] is declared
/// under, the folder an edge adapter lives in, and the first segment of its
/// unit's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Edge {
    /// An HTTP route, self-mount or fallback.
    Http,
    /// A GraphQL root field or subscription.
    Graphql,
    /// A WebSocket gateway's socket and its messages.
    Ws,
    /// A queue job attempt.
    Queue,
    /// A scheduled tick.
    Schedule,
    /// An MCP operation.
    Mcp,
    /// An in-process event listener.
    Events,
}

impl Edge {
    /// Every framework edge, in the vocabulary's order.
    pub const ALL: &'static [Edge] = &[
        Self::Http,
        Self::Graphql,
        Self::Ws,
        Self::Queue,
        Self::Schedule,
        Self::Mcp,
        Self::Events,
    ];

    /// The word a unit name, a span target and a config namespace spell this
    /// edge with (`"http"`, `"graphql"`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Graphql => "graphql",
            Self::Ws => "ws",
            Self::Queue => "queue",
            Self::Schedule => "schedule",
            Self::Mcp => "mcp",
            Self::Events => "events",
        }
    }
}

/// One unit of work: its canonical name, the target its span is emitted on, its
/// [`Kind`] and, for a framework edge's, its [`Edge`].
///
/// Declared only through [`unit!`](crate::unit); the accessors are `const` so
/// the reading macros can put them in `tracing`'s static callsite metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    name: &'static str,
    target: &'static str,
    kind: Kind,
    edge: Option<Edge>,
}

impl Unit {
    /// The canonical `<edge>.<unit>`, `lifecycle.<unit>` or `<crate>.<unit>`
    /// name: the span's name, the line's `name:` and its `message`.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// The span target: `nest_rs::<edge>` for an edge's unit,
    /// `nest_rs::lifecycle` for the kernel's, the declaring crate's own for any
    /// other.
    pub const fn target(&self) -> &'static str {
        self.target
    }

    /// OpenTelemetry's span kind for this unit.
    pub const fn kind(&self) -> Kind {
        self.kind
    }

    /// The framework edge that dispatches this unit, read off its name when
    /// [`unit!`](crate::unit) declared it; `None` for a unit no edge
    /// dispatches.
    pub const fn edge(&self) -> Option<Edge> {
        self.edge
    }
}

/// The crate that declares the kernel's units.
const KERNEL: &str = "nest-rs-core";

/// The first segment of every kernel unit, and the tail of its target.
const KERNEL_WORD: &str = "lifecycle";

/// `operation_log` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use super::{
        Edge, KERNEL, KERNEL_WORD, Kind, Unit, eq_bytes, is_crate_root, is_edge_root, is_lib_name,
    };

    /// Evaluated in a `const` by [`unit!`](crate::unit), which passes the
    /// declaring crate as `owner`: a framework edge's unit, a kernel unit, or
    /// any other crate's under its own name.
    #[expect(
        clippy::panic,
        reason = "only ever evaluated in a const, where a panic is the compile error naming the refused fact"
    )]
    pub const fn declare_unit(
        owner: &str,
        target: &'static str,
        kind: Kind,
        name: &'static str,
    ) -> Unit {
        let bytes = name.as_bytes();
        let mut i = 0;
        let mut dots = 0;
        let mut dot = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'a'..=b'z' | b'_' => {}
                // A crate name carries digits (`nestrs-s3`); a unit word does not.
                b'0'..=b'9' if dots == 0 => {}
                b'.' => {
                    dots += 1;
                    dot = i;
                }
                _ => panic!(
                    "a unit name is lowercase ASCII words, `<edge|crate>.<unit>`, with digits \
                     only in the crate's segment"
                ),
            }
            i += 1;
        }
        if dots != 1 || dot == 0 || dot == bytes.len() - 1 {
            panic!("a unit name is `<edge|crate>.<unit>`: one dot, a word on each side");
        }
        if !bytes[0].is_ascii_lowercase() {
            panic!(
                "a unit name is lowercase ASCII words, `<edge|crate>.<unit>`, with digits only in \
                 the crate's segment"
            );
        }

        if dot == KERNEL_WORD.len() && eq_bytes(bytes, 0, KERNEL_WORD.as_bytes(), 0, dot) {
            if !is_lib_name(owner, KERNEL) || owner.len() != KERNEL.len() {
                panic!("the `lifecycle` units are the kernel's, declared by `nest-rs-core`");
            }
            if !is_edge_root(target, "nest_rs::", name) {
                panic!("a kernel unit is emitted on `nest_rs::lifecycle`");
            }
            return Unit {
                name,
                target,
                kind,
                edge: None,
            };
        }

        let mut known = None;
        let mut e = 0;
        while e < Edge::ALL.len() {
            let word = Edge::ALL[e].as_str();
            if word.len() == dot && eq_bytes(word.as_bytes(), 0, bytes, 0, dot) {
                known = Some(Edge::ALL[e]);
            }
            e += 1;
        }
        let framework = is_framework_crate(owner);
        if known.is_some() {
            if !framework {
                panic!(
                    "the edge vocabulary is the framework's: name the unit after your crate, \
                     `<crate>.<unit>`"
                );
            }
            if !is_edge_root(target, "nest_rs::", name) {
                panic!("a unit is emitted on its edge's target, `nest_rs::<edge>`");
            }
            if !is_edge_root(owner, "nest-rs-", name) {
                panic!("a unit is declared by the crate that owns its edge, `nest-rs-<edge>`");
            }
            return Unit {
                name,
                target,
                kind,
                edge: known,
            };
        }

        let named_after_owner = is_lib_name(owner, name) && owner.len() == dot;
        if framework {
            if named_after_owner {
                panic!(
                    "the `nest-rs-` prefix is the framework's: a package is named \
                     `nestrs-<subject>` and declares `<its crate>.<unit>`"
                );
            }
            panic!(
                "a unit's edge is one of the closed edge vocabulary in \
                 `nest_rs_core::Edge::ALL`"
            );
        }
        if !named_after_owner {
            panic!("a unit is declared by the crate it is named after, `<crate>.<unit>`");
        }
        if !is_crate_root(target, name, dot) {
            panic!(
                "a package's unit is emitted on its crate's own target, `<crate>` or \
                 `<crate>::<concern>`"
            );
        }
        Unit {
            name,
            target,
            kind,
            edge: None,
        }
    }

    /// Whether `krate`, the crate a reading macro expands in, declared `unit`.
    pub const fn unit_opened_by(unit: &Unit, krate: &str) -> bool {
        if unit.edge.is_some() {
            return is_edge_root(krate, "nest-rs-", unit.name);
        }
        let mut dot = 0;
        while unit.name.as_bytes()[dot] != b'.' {
            dot += 1;
        }
        if dot == KERNEL_WORD.len()
            && eq_bytes(unit.name.as_bytes(), 0, KERNEL_WORD.as_bytes(), 0, dot)
        {
            return krate.len() == KERNEL.len() && is_lib_name(krate, KERNEL);
        }
        krate.len() == dot && is_lib_name(krate, unit.name)
    }

    /// Whether `owner`, read as a library name (`-` as `_`), is the
    /// framework's: `nest_rs` itself or `nest_rs_<subject>`.
    const fn is_framework_crate(owner: &str) -> bool {
        let (owner, framework) = (owner.as_bytes(), b"nest_rs");
        if owner.len() < framework.len() {
            return false;
        }
        let mut i = 0;
        while i < owner.len() {
            let byte = if owner[i] == b'-' { b'_' } else { owner[i] };
            if i == framework.len() {
                return byte == b'_';
            }
            if byte != framework[i] {
                return false;
            }
            i += 1;
        }
        true
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

/// Whether `krate`, read as a library name (`-` as `_`), spells the start of
/// `lib`; the caller checks the length it means.
const fn is_lib_name(krate: &str, lib: &str) -> bool {
    let (krate, lib) = (krate.as_bytes(), lib.as_bytes());
    if krate.len() > lib.len() {
        return false;
    }
    let mut i = 0;
    while i < krate.len() {
        let byte = if krate[i] == b'-' { b'_' } else { krate[i] };
        let want = if lib[i] == b'-' { b'_' } else { lib[i] };
        if byte != want {
            return false;
        }
        i += 1;
    }
    true
}

/// Whether `target` is the crate `name[..dot]` names, or a path under it —
/// `nestrs_kafka` or `nestrs_kafka::consumer`, never `nestrs_kafka_streams`.
const fn is_crate_root(target: &str, name: &str, dot: usize) -> bool {
    let (target, name) = (target.as_bytes(), name.as_bytes());
    if target.len() < dot || !eq_bytes(target, 0, name, 0, dot) {
        return false;
    }
    target.len() == dot
        || (target.len() > dot + 2 && target[dot] == b':' && target[dot + 1] == b':')
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

/// Declare one [`Unit`] of this crate —
/// `pub const REQUEST: Unit = nest_rs_core::unit!("http.request", target:
/// crate::target::HTTP, kind: Server);`, compiled in `nest_rs_http::unit`'s
/// example.
///
/// The declaring crate (`CARGO_PKG_NAME`, `-` read as `_`) decides the tier:
///
/// - **a framework edge**, `nest-rs-<edge>`, declares `<edge>.<unit>` on
///   `nest_rs::<edge>`, with `<edge>` in [`Edge::ALL`];
/// - **the kernel**, `nest-rs-core`, declares `lifecycle.<unit>` on
///   `nest_rs::lifecycle`;
/// - **any other crate** declares `<its library name>.<unit>` on its own
///   target, the library name or a path under it: `nestrs-kafka` declares
///   `nestrs_kafka.message` on `nestrs_kafka` or `nestrs_kafka::consumer`.
///
/// Refused at compile time, each with its own sentence: a name off the grammar
/// (lowercase words, one dot, digits only in the crate's segment), a target
/// outside the tier's, an edge word or `lifecycle` from a crate that does not
/// own it, a unit named after another crate, and a package taking the
/// framework's `nest-rs-` prefix, in either spelling, or its `nest-rs` name.
#[macro_export]
macro_rules! unit {
    ($name:literal, target: $target:expr, kind: $kind:ident $(,)?) => {{
        const __UNIT: $crate::operation_log::Unit = $crate::__private::declare_unit(
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
/// `Option` of one; the fields after `started` are the declaring crate's own.
/// A unit another crate declared is refused at compile time. Whichever crate
/// declared it, the line is filed on [`TARGET`].
///
/// Call it inside the unit's correlation (`RequestContinuation::enter`) and
/// outside its span, so the line carries the unit's ids and is nobody's child.
#[macro_export]
macro_rules! operation_line {
    ($unit:path, span: $span:expr, outcome: $outcome:expr, started: $started:expr $(, $($field:tt)*)?) => {{
        const _: () = ::core::assert!(
            $crate::__private::unit_opened_by(&$unit, ::core::env!("CARGO_PKG_NAME")),
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
        std::panic::catch_unwind(|| __private::declare_unit(owner, target, Kind::Server, name))
            .map_err(|payload| crate::panic_message(payload.as_ref()))
    }

    #[test]
    fn every_edge_declares_a_unit_from_its_own_crate_on_its_own_target() {
        for edge in Edge::ALL.iter().map(|edge| edge.as_str()) {
            let owner = format!("nest-rs-{edge}");
            let target: &'static str = format!("nest_rs::{edge}").leak();
            let name: &'static str = format!("{edge}.some_unit").leak();
            let unit = declare(&owner, target, name).expect("a well-formed unit");
            assert_eq!(
                (unit.name(), unit.target(), unit.kind()),
                (name, target, Kind::Server)
            );
            assert!(__private::unit_opened_by(&unit, &owner));
            assert!(!__private::unit_opened_by(&unit, "nest-rs-redis"));
            assert!(!__private::unit_opened_by(&unit, &format!("{owner}-tests")));
        }
    }

    #[test]
    fn every_edge_spells_the_vocabulary_in_order() {
        let spelled: Vec<&str> = Edge::ALL.iter().map(|edge| edge.as_str()).collect();
        assert_eq!(
            spelled,
            [
                "http", "graphql", "ws", "queue", "schedule", "mcp", "events"
            ]
        );
    }

    #[test]
    fn a_declared_unit_knows_its_edge() {
        for &edge in Edge::ALL {
            let owner = format!("nest-rs-{}", edge.as_str());
            let target: &'static str = format!("nest_rs::{}", edge.as_str()).leak();
            let name: &'static str = format!("{}.some_unit", edge.as_str()).leak();
            let unit = declare(&owner, target, name).expect("a well-formed unit");
            assert_eq!(unit.edge(), Some(edge), "{name}");
        }
    }

    #[test]
    fn any_other_crate_declares_a_unit_under_its_own_name_on_its_own_target() {
        for (owner, target, name) in [
            ("nestrs-kafka", "nestrs_kafka", "nestrs_kafka.message"),
            (
                "nestrs-kafka",
                "nestrs_kafka::consumer",
                "nestrs_kafka.message",
            ),
            ("nestrs-s3", "nestrs_s3", "nestrs_s3.upload"),
            ("features", "features::sync", "features.sync"),
        ] {
            let unit = declare(owner, target, name).expect(name);
            assert_eq!((unit.name(), unit.target()), (name, target));
            assert_eq!(unit.edge(), None, "no edge dispatches {name}");
        }
    }

    #[test]
    fn the_kernel_declares_the_lifecycle_units_on_its_own_target() {
        let unit = declare("nest-rs-core", crate::target::LIFECYCLE, "lifecycle.hook")
            .expect("the kernel's unit");
        assert_eq!(unit.target(), crate::target::LIFECYCLE);
        assert_eq!(unit.edge(), None, "no edge dispatches a lifecycle hook");
        assert!(__private::unit_opened_by(&unit, "nest-rs-core"));
        assert!(!__private::unit_opened_by(&unit, "nest-rs-http"));
        assert!(!__private::unit_opened_by(&unit, "lifecycle"));
    }

    #[test]
    fn a_package_unit_is_opened_by_its_crate_alone() {
        let unit = declare("nestrs-kafka", "nestrs_kafka", "nestrs_kafka.message").expect("a unit");
        assert!(__private::unit_opened_by(&unit, "nestrs-kafka"));
        assert!(__private::unit_opened_by(&unit, "nestrs_kafka"));
        assert!(!__private::unit_opened_by(&unit, "nestrs-kafka-macros"));
        assert!(!__private::unit_opened_by(&unit, "nestrs-kafk"));
        assert!(!__private::unit_opened_by(&unit, "nest-rs-queue"));
    }

    /// A crate named after an edge word is still not that edge's crate.
    #[test]
    fn an_edge_unit_is_not_opened_by_a_crate_named_after_its_edge() {
        let unit = declare("nest-rs-http", "nest_rs::http", "http.request").expect("a unit");
        assert!(!__private::unit_opened_by(&unit, "http"));
    }

    #[test]
    fn a_package_unit_off_its_crate_is_refused_naming_the_fact() {
        for (owner, target, name, fact) in [
            (
                "nestrs-kafka",
                "nestrs_kafka",
                "kafka.message",
                "declared by the crate it is named after",
            ),
            (
                "queue-tools",
                "queue_tools",
                "queue.job",
                "the edge vocabulary is the framework's",
            ),
            (
                "nest-rs-grpc",
                "nest_rs_grpc",
                "nest_rs_grpc.call",
                "the `nest-rs-` prefix is the framework's",
            ),
            (
                "nestrs-kafka",
                "nest_rs::kafka",
                "nestrs_kafka.message",
                "its crate's own target",
            ),
            (
                "nestrs-kafka",
                "nestrs_kafka_streams",
                "nestrs_kafka.message",
                "its crate's own target",
            ),
            (
                "nestrs-kafka",
                "nestrs_kafka",
                "lifecycle.hook",
                "the `lifecycle` units are the kernel's",
            ),
            (
                "nest-rs-http",
                "nest_rs::lifecycle",
                "lifecycle.hook",
                "the `lifecycle` units are the kernel's",
            ),
            (
                "nest-rs-core",
                "nest_rs::core",
                "lifecycle.hook",
                "`nest_rs::lifecycle`",
            ),
            (
                "nest_rs_grpc",
                "nest_rs_grpc",
                "nest_rs_grpc.call",
                "the `nest-rs-` prefix is the framework's",
            ),
            (
                "nest-rs",
                "nest_rs::http",
                "nest_rs.request",
                "the `nest-rs-` prefix is the framework's",
            ),
            ("nestrs-s3", "nestrs_s3", "nestrs_s3.upload2", "lowercase"),
            ("nestrs-3d", "nestrs_3d", "3d.render", "lowercase"),
        ] {
            let refused = declare(owner, target, name).expect_err(name);
            assert!(
                refused.contains(fact),
                "{name} from {owner} on {target}: {refused}"
            );
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
