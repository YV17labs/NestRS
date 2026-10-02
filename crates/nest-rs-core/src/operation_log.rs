//! One line per unit of work, and the vocabulary every edge files it with.
//!
//! The ids on a log line relate lines to each other; they do not say what the
//! work *was*. A line rendered by
//! [`TextFormat`](crate::logging::TextFormat) carries no span state at all — by
//! design, since a span's attributes are not part of a log record — so the
//! identity of a unit of work has to arrive as **event** attributes, on a line
//! the edge emits once per unit. HTTP has always had one; the other edges did
//! not, and their work was anonymous on the console as a result.
//!
//! What each edge writes is its own — a route, an event name, a job id, a tool
//! name — but the target, the duration formula and the outcome words are shared,
//! so an operator queries the family one way and a new edge cannot invent a
//! fourth word for "it failed".
//!
//! **Field names on a line are flat, never dotted**, and that is not a style
//! choice: `tracing`'s macro grammar is locally ambiguous on a dotted name when
//! the target is a path expression rather than a literal, and [`TARGET`] is a
//! path — so `ws.event = %event` is a compile error on the line while the
//! identical field compiles on the span beside it. The span keeps the dotted,
//! conventions-shaped names for the export; the line uses the edge's short
//! ones.
//!
//! The canonical name of one unit of work is `<edge>.<unit>`, and this module
//! declares the **type** rather than the names: a [`Unit`].
//!
//! **One value, three slots, so nothing can drift.** Each unit is declared
//! once — by the crate that owns the edge, as `<crate>::unit::<UNIT>`, through
//! [`unit!`](crate::unit) — and read three times: by the
//! [`operation_span!`](crate::operation_span) that opens the unit, and by the
//! [`operation_line!`](crate::operation_line) that files its line, as the
//! event's `name:` (its metadata identity) and its `message` (what a console
//! shows). Both macros take the unit as a **path** and read every slot off it,
//! so a literal name, a target that is not the edge's or a kind outside
//! [`Kind`] is not something a call site can write. Two of those slots used to
//! be two different vocabularies: the spans said `http.request` and
//! `mcp.operation` while the lines said `request served` and `operation
//! served`, and two names for one thing is what an operator has to learn twice
//! and a query gets wrong once.
//!
//! **The names live with their edges, and the kernel holds none.** A unit name
//! is per-edge vocabulary exactly as a span target is, so it obeys the rule
//! [`target`](crate::target) states: the crate that owns the concern is the
//! crate that names it. What the kernel holds is the grammar, and it holds it
//! as a `const fn`: [`unit!`](crate::unit) evaluates every declaration at
//! compile time and refuses, as a compile error, a name that is not lowercase
//! `<edge>.<unit>`, an edge outside [`EDGES`], a target other than
//! `nest_rs::<edge>`, and a declaring crate other than `nest-rs-<edge>`. The
//! two macros that read a unit refuse, the same way, a crate that did not
//! declare it.
//!
//! **The shape is `<edge>.<unit>`, and it is the norm rather than ours.**
//! Lowercase, dot-separated, namespace first: the form OpenTelemetry gives an
//! event name, whose whole job is to identify a *class* of event at low
//! cardinality while the human wording stays in the body. Four of this
//! framework's six edges already spelled their span that way before any of this
//! — `http.request`, `ws.message`, `mcp.operation`, `graphql.subscription` —
//! so the work here was to retire two strings of prose (`"scheduled job"`,
//! `"process job"`), not to invent a scheme.
//!
//! **Depending on it is not depending on OpenTelemetry.** The alignment is free:
//! a unit is three `&'static str` and a [`Kind`], an app that exports nothing
//! pays nothing, and a bridge that does export reads `name:` as `event.name`
//! without the framework naming an exporter. Neither W3C Trace Context —
//! scoped, in its own words, to "enable trace correlation" — nor anything else
//! in reach names this concept.
//!
//! **The namespace is the closed edge vocabulary** (`architecture.md`), which is
//! what stops a new transport from inventing an eighth word: `grpc.call` does
//! not compile until `grpc` is added to [`EDGES`] deliberately. All seven are in
//! use: `events` was the last to be unused, and a name nothing emits under is
//! the same defect as a span field nothing fills — which is why the listener,
//! not the `emit`, became its unit. A listener is developer code that logs,
//! writes and can panic; an `emit` is a line inside whatever unit the emitter
//! was already serving.

use std::time::Instant;

/// The target every operation line is emitted on, whichever edge files it.
///
/// **One target is the family's toggle.**
/// `<PREFIX>_LOG=info,nest_rs::operation=off` silences every edge's line at
/// once, which is why no edge grows a `#[config]` field — and the `for_root`
/// seam that one would oblige — to hold a boolean the filter already answers.
/// `NESTRS_HTTP__ACCESS_LOG` predates the family and stays: it is an app's
/// pinned config rather than a deployment's filter, and it names one edge's
/// line rather than the family's.
///
/// **It names the line, not a subsystem.** Every other framework target —
/// `nest_rs::http`, `nest_rs::orm`, `nest_rs::schedule` — is rooted at the code
/// that emits it; this one is the only one naming a *category of line* that
/// crosses all of them, so a subsystem-shaped word here reads as a subsystem and
/// hides what the target is. What distinguishes the line is that there is
/// exactly one per unit of work and it carries an [`OK`]/[`ERROR`]/[`PANIC`]/[`CANCELLED`] and
/// a [`DURATION_MS`] — an operation, which is the word the operation span and
/// this module already use, and the word the MCP edge's line took when
/// `operation served` was retired for it. Through 5.1 it was
/// `nest_rs::access`: HTTP's word for its own line, left behind when the concept
/// generalised to six edges. A clock tick has no caller, so nothing accesses
/// anything.
///
/// **The string may not prefix another target**, and that is mechanical rather
/// than aesthetic: `EnvFilter` matches a directive's target with `starts_with`
/// on the raw string, not on `::` segments. `nest_rs::access` prefixed
/// `nest_rs::access_graph`, so the toggle documented above *also* silenced the
/// boot `warn` naming resolvers unreachable from the GraphQL schema — an
/// operator lost a startup diagnostic with nothing to show it had gone. Neither
/// name survives: that warn belongs to `nest_rs::graphql`, filed by the crate
/// that owns the resolver registry rather than by the kernel. The
/// `filters` join in `nest-rs-conformance` derives every target both workspaces
/// emit and fails on the next such pair.
pub const TARGET: &str = "nest_rs::operation";

/// OpenTelemetry's `SpanKind`, which [`operation_span!`](crate::operation_span)
/// records as `otel.kind`.
///
/// An enum because the vocabulary is closed by the specification: a typo can
/// only ever be a value no backend groups on, and the kind decides how a
/// tracing backend reads the span, so a wrong one is a wrong service map rather
/// than a cosmetic slip. A [`Unit`] carries its kind, so an edge states it once,
/// where it declares the unit, and no call site can open the unit as another.
///
/// **Only the kinds this framework emits are declared.** The specification also
/// defines `client` and `producer`; nothing here opens a span for an outbound
/// call or for handing work to a queue, and a variant nothing uses is the same
/// defect as a span field nothing records. Whichever edge needs one adds it.
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

/// The closed edge vocabulary (`architecture.md`): the only namespaces a
/// [`Unit`] may be declared under. Adding an edge is a framework change, made
/// here.
pub const EDGES: [&str; 7] = [
    "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
];

/// One unit of work an edge opens: its canonical `<edge>.<unit>` name, the
/// target its span is emitted on, and its [`Kind`].
///
/// Declared only through [`unit!`](crate::unit), whose compile-time evaluation
/// is the grammar — see the module doc — and read only by
/// [`operation_span!`](crate::operation_span) and
/// [`operation_line!`](crate::operation_line). The fields are private, so a
/// unit cannot be built around the check, and the accessors are `const` so the
/// two macros can put them in `tracing`'s static callsite metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unit {
    name: &'static str,
    target: &'static str,
    kind: Kind,
}

impl Unit {
    /// Evaluated in a `const` by [`unit!`](crate::unit), which passes the
    /// declaring crate as `owner`; every refusal is a compile error naming the
    /// fact it checks.
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

    /// Whether `krate` — the crate a reading macro expands in — is the one
    /// that declared this unit, which is the only crate that opens it or files
    /// its line.
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

/// `whole == prefix + <the edge of name>`, byte for byte — `name` already
/// holds its one dot.
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

/// `a[at_a..at_a + len] == b[at_b..at_b + len]`; every caller has checked
/// both are that long.
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
/// Developer code unwound. Distinct from [`ERROR`] because the two are read
/// differently under incident: one is a handled path, the other is not.
pub const PANIC: &str = "panic";
/// It was stopped before it settled — its caller gave up on it; the server
/// ended it at the shutdown signal because it has no end of its own, the way
/// its protocol ends one (a stream ends, a socket closes, a subscription
/// completes); or the shutdown window closed on it, and it was dropped where it
/// waited. Distinct from [`ERROR`] because the unit neither completed nor
/// failed, and what it had not yet done stays undone.
///
/// Every edge files it, and every edge has to *build* it, since no handler's
/// return says it: the line is held by a guard dropped with the unit's future,
/// so the end is filed whether or not the edge noticed it. Each edge's own
/// suite files one and asserts it, and [`PANIC`] beside it.
pub const CANCELLED: &str = "cancelled";

/// Record on a unit's span how the unit ended, where OpenTelemetry reads it.
///
/// A unit that did not complete as asked — [`ERROR`], [`PANIC`], [`CANCELLED`] —
/// gets the word its line files under `outcome` as the span's `error.type`, and
/// the span's status set to `Error`; [`OK`] records nothing, since the
/// conventions leave a successful span's status unset. One word for the line and
/// the span, so an operator queries one vocabulary whichever half they hold.
///
/// **[`operation_line!`](crate::operation_line) calls it**, so every line filed
/// — the one a guard files for a dropped unit `cancelled` included — records its
/// outcome on the span in the same word. [`operation_span!`](crate::operation_span)
/// declares both fields on every unit's span, and a declared field nothing
/// records is the defect the macro's docs name. The conventions ask for both on a
/// failed operation — `error.type` is *conditionally required* when the operation
/// ended in error, and the status follows it — and without them a backend shows
/// a request cut at the shutdown window, or a job that panicked, as a span that
/// succeeded. An edge calls it directly only where it ends a unit without filing
/// a line: an HTTP request whose access log is off.
pub fn record_outcome(span: &tracing::Span, outcome: &str) {
    if outcome != OK {
        record_error(span, outcome);
    }
}

/// Record on a unit's span that it ended in an error of class `error_type`.
///
/// [`record_outcome`] is the call an edge makes; this is the form under it for
/// the one edge whose conventions name a finer class than the outcome word: an
/// HTTP request answered `5xx` records its status code, as a string, which is
/// what OpenTelemetry's HTTP conventions put in `error.type` for a response
/// whose status says it failed.
pub fn record_error(span: &tracing::Span, error_type: &str) {
    span.record(ERROR_TYPE, error_type);
    span.record(STATUS_CODE, STATUS_ERROR);
}

/// OpenTelemetry's attribute for the class of error an operation ended with.
const ERROR_TYPE: &str = "error.type";

/// The field `tracing-opentelemetry` reads a span's status from.
const STATUS_CODE: &str = "otel.status_code";

/// The status a failed unit's span carries. `tracing-opentelemetry` reads the
/// word case-insensitively; the conventions' own spelling is `Error`.
const STATUS_ERROR: &str = "error";

/// The field name every operation line files [`duration_ms`] under.
///
/// [`operation_line!`](crate::operation_line) writes it as `{ DURATION_MS }`,
/// the constant-name form `tracing` has accepted since 0.1.39, and the console
/// formatter reads it to give this one field a fixed width — so the writer and
/// the reader name one constant.
pub const DURATION_MS: &str = "duration_ms";

/// Digits after the decimal point when [`DURATION_MS`] is rendered for a human.
///
/// Equal to [`duration_ms`]'s own resolution, which is what makes padding
/// honest: the formula already rounds to the microsecond, so the digits a fixed
/// width adds are zeros the value really has, never precision it never
/// measured.
pub const DURATION_DECIMALS: usize = 3;

/// Elapsed milliseconds at microsecond resolution.
///
/// Rendered in milliseconds because that is the unit an operator compares
/// against a timeout, and measured to the microsecond because a sub-millisecond
/// unit of work is the common case and `0` tells them nothing.
pub fn duration_ms(start: Instant) -> f64 {
    (start.elapsed().as_secs_f64() * 1e6).round() / 1e3
}

/// Declare one [`Unit`] of the edge this crate owns —
/// `pub const REQUEST: Unit = nest_rs_core::unit!("http.request", target:
/// crate::target::HTTP, kind: Server);`, compiled in `nest_rs_http::unit`'s
/// example.
///
/// Evaluated in a `const` whatever the position it is written in, so every
/// refusal [`Unit`] states is a compile error: a name that is not lowercase
/// `<edge>.<unit>`, an edge outside [`EDGES`], a target other than
/// `nest_rs::<edge>` and a crate other than `nest-rs-<edge>` — the crate is
/// read from `CARGO_PKG_NAME`, which is why only the edge's own crate can
/// declare its units. The name is a literal: this is the one place it is
/// written.
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
/// Everything the family shares is written here and nowhere else: the
/// [`TARGET`], the unit's [`name`](Unit::name) as both the event's `name:` and
/// its `message`, `outcome`, and [`DURATION_MS`] measured from `started`. The
/// fields after those are the edge's own. `outcome` is one of [`OK`],
/// [`ERROR`], [`PANIC`], [`CANCELLED`] — or an `Option` of one, for the edge
/// whose answered unit carries none (HTTP, whose `status` says it) — and
/// [`record_outcome`] is called with it on `span`, so the line and the span
/// cannot say two things. Like [`operation_span!`](crate::operation_span), it
/// refuses at compile time a unit another crate declared.
///
/// The line is filed where it is called, so an edge calls it inside the unit's
/// correlation (`RequestContinuation::enter`) and outside its span: the line
/// carries the unit's ids and is nobody's child event.
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
        // The shape that matters: a fast unit of work reports a fraction, not
        // the `0` an integer-millisecond formula would give it.
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
        // A new edge picking a fourth word is what this vocabulary exists to
        // prevent; that they differ from each other is the cheap half of it.
        let words = [OK, ERROR, PANIC, CANCELLED];
        let distinct: std::collections::BTreeSet<&str> = words.into_iter().collect();
        assert_eq!(distinct.len(), words.len());
    }

    /// The grammar `unit!` evaluates at compile time, run here at run time so
    /// each refusal is asserted on its own; the trybuild snapshots in
    /// `tests/integration/diagnostics/` prove the evaluation is a compile error.
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
