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
/// exactly one per unit of work and it carries an [`OK`]/[`ERROR`]/[`PANIC`] and
/// a [`DURATION_MS`] — an operation, which is the word the operation span, the
/// MCP edge's `operation served` and this module already use. Through 5.1 it was
/// `nest_rs::access`: HTTP's word for its own line, left behind when the concept
/// generalised to six edges. A clock tick has no caller, so nothing accesses
/// anything.
///
/// **The string may not prefix another target**, and that is mechanical rather
/// than aesthetic: `EnvFilter` matches a directive's target with `starts_with`
/// on the raw string, not on `::` segments. `nest_rs::access` prefixed
/// `nest_rs::access_graph`, so the toggle documented above *also* silenced the
/// boot `warn` naming resolvers unreachable from the GraphQL schema — an
/// operator lost a startup diagnostic with nothing to show it had gone. The
/// `filters` join in `nest-rs-conformance` derives every target both workspaces
/// emit and fails on the next such pair.
pub const TARGET: &str = "nest_rs::operation";

/// The canonical name of one unit of work — the vocabulary [`TARGET`] carries.
///
/// **One constant, three slots, so nothing can drift.** Each name is written
/// once here and read by the [`operation_span!`](crate::operation_span) that
/// opens the unit, by that line's `name:` (the event's metadata identity), and
/// by its `message` (what a console shows). Two of those used to be two
/// different vocabularies: the spans said `http.request` and `mcp.operation`
/// while the lines said `request served` and `operation served`, and two names
/// for one thing is what an operator has to learn twice and a query gets wrong
/// once. `nest-rs-ws` had already hit the wall and written the workaround —
/// "`lifecycle` rather than the span's name because `tracing` offers no way to
/// read one back". A shared constant is that way.
///
/// **The shape is `<edge>.<unit>`, and it is the norm rather than ours.**
/// Lowercase, dot-separated, namespace first: the form OpenTelemetry gives an
/// event name, whose whole job is to identify a *class* of event at low
/// cardinality while the human wording stays in the body. Four of this
/// framework's six edges already spelled their span that way before any of this
/// — `http.request`, `ws.message`, `mcp.operation`, `graphql.subscription` —
/// so the work here was to retire two strings of prose (`"scheduled job"`,
/// `"process job"`), not to invent a scheme.
///
/// **Depending on it is not depending on OpenTelemetry.** The alignment is free:
/// the constants are `&'static str` in the kernel, an app that exports nothing
/// pays nothing, and a bridge that does export reads `name:` as `event.name`
/// without the framework naming an exporter. Neither W3C Trace Context — scoped,
/// in its own words, to "enable trace correlation" — nor anything else in reach
/// names this concept.
///
/// **The namespace is the closed edge vocabulary** (`architecture.md`), which is
/// what stops a new transport from inventing a seventh word: `grpc.call` fails
/// the shape test below until `grpc` is opened as an edge deliberately. `events`
/// is absent because an event listener files no operation line today, and a name
/// nothing emits under is the same defect as a span field nothing fills.
pub mod unit {
    /// One HTTP request, filed once the response body has been written.
    pub const HTTP_REQUEST: &str = "http.request";
    /// One WS socket opening.
    pub const WS_CONNECT: &str = "ws.connect";
    /// One WS socket closing.
    pub const WS_DISCONNECT: &str = "ws.disconnect";
    /// One WS message.
    pub const WS_MESSAGE: &str = "ws.message";
    /// One queue job attempt.
    pub const QUEUE_JOB: &str = "queue.job";
    /// One scheduled tick.
    pub const SCHEDULE_TICK: &str = "schedule.tick";
    /// One MCP operation — a request or a notification.
    pub const MCP_OPERATION: &str = "mcp.operation";
    /// One GraphQL subscription; the connection is the unit of work.
    pub const GRAPHQL_SUBSCRIPTION: &str = "graphql.subscription";
}

/// OpenTelemetry's `SpanKind`, which is the vocabulary
/// [`operation_span!`](crate::operation_span) records as `otel.kind`.
///
/// Constants rather than strings because the vocabulary is closed by the
/// specification: a typo can only ever be a value no backend groups on, and
/// nothing at the call site would say so. The kind decides how a tracing
/// backend reads the span, so a wrong one is a wrong service map rather than a
/// cosmetic slip.
///
/// **Only the kinds this framework emits are declared.** The specification also
/// defines `client` and `producer`; nothing here opens a span for an outbound
/// call or for handing work to a queue, and a constant nothing fills is the
/// same defect as a span field nothing records. Whichever edge needs one adds
/// it in the line it needs it.
pub mod kind {
    /// Work this process accepted from somewhere else — an HTTP request, a WS
    /// message, an MCP operation, a GraphQL subscription.
    pub const SERVER: &str = "server";
    /// Work taken off a queue and run here.
    pub const CONSUMER: &str = "consumer";
    /// Work with no caller and no wire — a scheduled tick.
    pub const INTERNAL: &str = "internal";
}

/// The unit of work completed as asked.
pub const OK: &str = "ok";
/// It returned an error, or was refused. The edge's own fields say which.
pub const ERROR: &str = "error";
/// Developer code unwound. Distinct from [`ERROR`] because the two are read
/// differently under incident: one is a handled path, the other is not.
pub const PANIC: &str = "panic";

/// The field name every edge files [`duration_ms`] under.
///
/// A field name is a literal token in `tracing`'s macro grammar, so an edge
/// spells it rather than referencing this; what reads the constant is the
/// console formatter, which owes this one field a fixed width. It is here
/// because the vocabulary of the line is here.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabularies these tests police.
    ///
    /// Test-local, all three: the `units` join derives its population from the
    /// source rather than from a list, so nothing outside this file ever read
    /// them — and an array published for no caller is surface to keep in step
    /// for nothing.
    const UNITS: [&str; 8] = [
        unit::HTTP_REQUEST,
        unit::WS_CONNECT,
        unit::WS_DISCONNECT,
        unit::WS_MESSAGE,
        unit::QUEUE_JOB,
        unit::SCHEDULE_TICK,
        unit::MCP_OPERATION,
        unit::GRAPHQL_SUBSCRIPTION,
    ];

    /// The closed edge vocabulary (`architecture.md`), which is the only set a
    /// canonical name may take its namespace from.
    const EDGES: [&str; 7] = [
        "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
    ];

    /// The kinds this framework emits — see `kind` for why the specification's
    /// other two are not declared.
    const KINDS: [&str; 3] = [kind::SERVER, kind::CONSUMER, kind::INTERNAL];

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

    /// The shape a canonical name must have, checked here because this is the
    /// one place a new transport touches when it adds a unit of work.
    ///
    /// `grpc.call` fails on the namespace until `grpc` is opened as an edge in
    /// `architecture.md` — a deliberate, reviewed act, and exactly the
    /// difference between easy to do on purpose and possible to do by accident.
    #[test]
    fn every_canonical_name_is_an_edge_namespace_and_one_lowercase_unit() {
        for name in UNITS {
            let (namespace, tail) = name
                .split_once('.')
                .unwrap_or_else(|| panic!("{name} is not `<edge>.<unit>`"));
            assert!(
                EDGES.contains(&namespace),
                "{name} takes its namespace from outside the closed edge \
                 vocabulary; open `{namespace}` as an edge first",
            );
            assert!(
                !tail.is_empty() && !tail.contains('.'),
                "{name} must carry exactly one dot",
            );
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "{name} must be lowercase",
            );
        }
    }

    #[test]
    fn no_two_units_share_a_canonical_name() {
        let distinct: std::collections::BTreeSet<&str> = UNITS.into_iter().collect();
        assert_eq!(
            distinct.len(),
            UNITS.len(),
            "two units of work under one name cannot be told apart",
        );
    }

    /// The words are the specification's, so what is left to check is that they
    /// are distinct and spelled the way `otel.kind` is read.
    #[test]
    fn the_span_kinds_are_distinct_and_spelled_as_otel_reads_them() {
        let distinct: std::collections::BTreeSet<&str> = KINDS.into_iter().collect();
        assert_eq!(distinct.len(), KINDS.len(), "a kind is declared twice");
        for k in KINDS {
            assert!(
                k.chars().all(|c| c.is_ascii_lowercase()),
                "{k} must be lowercase, as `otel.kind` is read",
            );
        }
    }

    #[test]
    fn the_outcome_words_are_distinct() {
        // A new edge picking a fourth word is what this vocabulary exists to
        // prevent; that they differ from each other is the cheap half of it.
        assert_ne!(OK, ERROR);
        assert_ne!(ERROR, PANIC);
    }
}
