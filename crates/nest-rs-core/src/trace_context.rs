//! W3C Trace Context — the framework's correlation primitive, in the kernel.
//!
//! # Why the standard is the primitive, and not a bespoke id beside it
//!
//! This module replaced a homemade `request_id`. That id answered exactly the
//! question `trace-id` answers — *which unit of work is this?* — so the two were
//! duplicates, and the framework carried both. Three arguments were made for
//! keeping the homemade one, and the specification retires all three:
//!
//! - **"an inbound id is forgeable"** — the spec's own answer is the *restart
//!   trace* mutation at a trust boundary, not a second identifier. A gate on
//!   ingress, which [`TraceParent`] and the transports implement.
//! - **"the standard needs a tracing SDK"** — it needs 16 random bytes, 8 random
//!   bytes and a `format!`. Nothing here depends on `nest-rs-opentelemetry`, a
//!   collector, or an exporter, and that is what keeps a primitive unconditional.
//! - **"a trace-id does not sort by time"** — it does if you choose bytes that
//!   do. [`TraceId::mint`] uses a UUID v7's bytes, so the value is a conformant
//!   trace-id *and* an ordered UUID.
//!
//! The deciding fact is the ecosystem's, not ours: OpenTelemetry's semantic
//! conventions have **no request-id concept**. What they have is
//! `http.request.header.x-request-id` — a header *captured* from upstream. That
//! is the standard telling us what `X-Request-Id` is: what the proxy in front of
//! you wrote, worth recording, never your identity.
//!
//! # Three values, and each answers something the others cannot
//!
//! | Field | What it names | Present |
//! |---|---|---|
//! | `trace_id` | the whole distributed operation | **always** |
//! | `span_id` | *this* unit of work inside it | **always** |
//! | `actor_id` | who it is being served for | once a principal is resolved |
//!
//! The split between the first two is not ceremony, and it is what the homemade
//! id could not express: an HTTP request that enqueues a job is **one trace and
//! two spans**, and the job's `parent_id` is the request's `span_id`. Under one
//! flat id that parent/child relation simply did not exist.
//!
//! # Where a trace comes from
//!
//! Whoever **accepts the work** decides, and there are exactly two answers —
//! continue a trace that arrived with the work, or start one. Continuing is only
//! ever from a source the accepting edge has decided to trust: a transport
//! weighs `traceparent` against its proxy list, a queue consumer trusts the
//! envelope its own producer wrote. An untrusted or malformed `traceparent`
//! **restarts** the trace, which the specification names and sanctions:
//!
//! > *Restart trace: All properties (`trace-id`, `parent-id`, `trace-flags`) are
//! > regenerated. This mutation is used in services that are defined as a front
//! > gate into secure networks and eliminates a potential denial-of-service
//! > attack surface.*
//!
//! Nothing is lost by restarting: the caller's claimed trace is recorded as an
//! attribute, so a deployment that wants to join against it still can.

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use uuid::Uuid;

use crate::request_scope::current_request_ctx;

/// The identifier of one distributed operation — 16 bytes, spelled as 32
/// lowercase hex characters on the wire.
///
/// # Why the bytes are a UUID v7
///
/// A `trace-id` is 16 arbitrary non-zero bytes, so the standard leaves the
/// choice open and this one buys two things at no cost to conformance.
///
/// **It sorts by time.** An index over trace ids stays local instead of
/// scattering across the keyspace the way a v4 does, and eyeballing two ids
/// tells you which operation came first — which matters the moment one is
/// stored as a column rather than only shipped to a collector.
///
/// **It still satisfies the `random-trace-id` flag.** Trace Context Level 2
/// requires that *at least the right-most 7 bytes* be random with uniform
/// distribution before an implementation may set that flag. A v7's right-most 7
/// bytes fall inside its 62-bit random tail, so the assertion is true and
/// [`TraceFlags::RANDOM`] is set. That flag is what lets a downstream
/// consistent-probability sampler decide from the low bits alone, which is why
/// the timestamp prefix costs nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct TraceId(Uuid);

impl TraceId {
    /// Start a new trace.
    pub fn mint() -> Self {
        Self(Uuid::now_v7())
    }

    /// Parse the 32-hex form carried by `traceparent`.
    ///
    /// `None` for anything the standard calls invalid: a wrong length, a
    /// non-hex or upper-case character, or the all-zero value the spec singles
    /// out — *"If the `trace-id` value is invalid, vendors MUST ignore the
    /// `traceparent`."*
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 32 || !hex.bytes().all(is_lower_hex) {
            return None;
        }
        let parsed = Uuid::parse_str(hex).ok()?;
        (!parsed.is_nil()).then_some(Self(parsed))
    }

    /// The wire spelling: 32 lowercase hex characters, no separators. This is
    /// also what every tracing backend joins logs and traces on, so it is what
    /// the `trace_id` log field carries.
    ///
    /// One spelling per type: this delegates to [`Display`](fmt::Display) rather
    /// than formatting independently, so the two can never come to disagree.
    pub fn to_hex(self) -> String {
        self.to_string()
    }

    /// The raw bytes, for an SDK that takes them directly.
    pub fn to_bytes(self) -> [u8; 16] {
        *self.0.as_bytes()
    }
}

impl fmt::Display for TraceId {
    /// One `write_str` into a caller-owned buffer, the shape [`SpanId`]'s
    /// `Display` already had. `write!(f, "{}", self.0.simple())` was a second
    /// `Arguments`/`fmt::write` layer nested inside every id write, and since
    /// every log line the framework renders carries a trace id, that layer is
    /// paid per **event** rather than per request — the cost the `hex` helper
    /// below exists to refuse.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; uuid::fmt::Simple::LENGTH];
        f.write_str(self.0.simple().encode_lower(&mut buf))
    }
}

/// The identifier of one unit of work inside a trace — 8 bytes, 16 hex
/// characters. An HTTP request, a WS message, an MCP operation, a queue job, a
/// scheduled tick: each is its own span, and each names the span that caused it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SpanId([u8; 8]);

impl SpanId {
    /// A fresh span id.
    ///
    /// The bytes come from a v4 UUID's two halves XORed together rather than
    /// from one half taken raw: a v4 fixes six bits (version and variant), and
    /// XOR against the other 64 random bits puts uniform values back in those
    /// positions instead of shipping a span id with predictable bits in it.
    pub fn mint() -> Self {
        let random = Uuid::new_v4().as_u128();
        Self((((random >> 64) as u64) ^ (random as u64)).to_be_bytes())
    }

    /// Parse the 16-hex form carried by `traceparent`.
    ///
    /// `None` for the all-zero value, which the spec refuses outright —
    /// *"Vendors MUST ignore the `traceparent` when the `parent-id` is
    /// invalid."*
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 16 || !hex.bytes().all(is_lower_hex) {
            return None;
        }
        let mut bytes = [0_u8; 8];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            *byte = (unhex(pair[0])? << 4) | unhex(pair[1])?;
        }
        (bytes != [0; 8]).then_some(Self(bytes))
    }

    /// The wire spelling: 16 lowercase hex characters.
    pub fn to_hex(self) -> String {
        self.to_string()
    }

    /// The raw bytes, for an SDK that takes them directly.
    pub fn to_bytes(self) -> [u8; 8] {
        self.0
    }
}

impl fmt::Display for SpanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(hex(&self.0, &mut [0; 16]))
    }
}

/// The one byte of `trace-flags`.
///
/// Two bits are defined and the rest are reserved: an implementation **must
/// not** assume an unknown bit means anything, and must forward it untouched,
/// which is why this is a byte rather than a pair of `bool`s.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceFlags(u8);

impl TraceFlags {
    /// The caller's recording decision. Set by default: with no sampler
    /// installed the framework records everything it is told about, and saying
    /// otherwise downstream would be a lie about what this deployment kept.
    pub const SAMPLED: u8 = 0x01;
    /// Asserts the right-most 7 bytes of the trace id are uniformly random —
    /// see [`TraceId`] for why a v7 may say so.
    pub const RANDOM: u8 = 0x02;

    /// What this framework asserts when it starts a trace.
    pub const fn started() -> Self {
        Self(Self::SAMPLED | Self::RANDOM)
    }

    /// Parse the two hex characters, preserving reserved bits verbatim.
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 2 || !hex.bytes().all(is_lower_hex) {
            return None;
        }
        let bytes = hex.as_bytes();
        Some(Self((unhex(bytes[0])? << 4) | unhex(bytes[1])?))
    }

    /// Whether the caller is recording this trace.
    pub const fn is_sampled(self) -> bool {
        self.0 & Self::SAMPLED != 0
    }

    /// The raw byte, reserved bits included.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl fmt::Display for TraceFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(hex(&[self.0], &mut [0; 2]))
    }
}

/// Vendor-specific trace state, carried across the whole trace.
///
/// Held opaquely and forwarded verbatim, because the specification requires it
/// of every participant including the ones that understand none of it:
///
/// > *Vendors receiving a `tracestate` request header MUST send it to outgoing
/// > requests.*
///
/// A framework that dropped it would silently break every vendor whose sampling
/// or routing rides in there, and it would break them *through* us — the kind of
/// failure nobody can attribute. It is dropped in exactly one case, which the
/// spec also names: when the `traceparent` beside it was invalid, the state
/// belongs to a trace we are not continuing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceState(Option<Arc<str>>);

impl TraceState {
    /// The longest `tracestate` accepted. The spec sets no hard maximum but
    /// asks that implementations "propagate at least 512 characters" and may
    /// drop beyond that; refusing an unbounded one is what keeps a caller from
    /// making every outbound request and every job envelope arbitrarily large.
    const MAX_LEN: usize = 512;

    /// Adopt a `tracestate` header from a source the caller has decided to
    /// trust. Refused whole when it is empty, over-long, or carries anything
    /// outside the printable ASCII the grammar allows — a value echoed into
    /// outbound headers and job envelopes is not somewhere to pass a newline on.
    pub fn adopt(raw: &str) -> Self {
        let raw = raw.trim();
        let usable = !raw.is_empty()
            && raw.len() <= Self::MAX_LEN
            && raw
                .bytes()
                .all(|b| b.is_ascii_graphic() || b == b' ' || b == b'\t');
        Self(usable.then(|| Arc::from(raw)))
    }

    /// The header value to forward, or `None` when there is no state to carry.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// One parsed `traceparent`: the trace, the span that sent it, and its flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceParent {
    /// The trace this belongs to.
    pub trace_id: TraceId,
    /// The sender's span — which becomes the receiver's parent.
    pub parent_id: SpanId,
    /// The sender's flags.
    pub flags: TraceFlags,
}

/// The one version this framework writes. Higher versions are still *read* —
/// see [`TraceParent::parse`].
const VERSION_00: &str = "00";
/// `00-` + 32 + `-` + 16 + `-` + 2.
const VERSION_00_LEN: usize = 55;

impl TraceParent {
    /// Parse a `traceparent` header value, to the letter of the specification.
    ///
    /// Four refusals, each one the spec's own and each one a real header seen in
    /// the wild rather than a hypothetical:
    ///
    /// - **version `ff`** is reserved and invalid;
    /// - **version `00`** must be *exactly* 55 characters — trailing data is not
    ///   forward compatibility, it is a malformed header;
    /// - **a higher version** is parsed as version `00` for its first 55
    ///   characters, and only when character 55 is a `-`. That rule is what lets
    ///   a future version add fields without every existing implementation
    ///   dropping the trace, and getting it wrong is how a deployment silently
    ///   starts restarting every trace the day one hop upgrades.
    /// - **an all-zero trace-id or parent-id**, refused by their own parsers.
    ///
    /// `None` means *restart the trace*, never *fail the request*: a caller's
    /// malformed header is not a reason to refuse to serve them.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let version = raw.get(..2)?;
        if !version.bytes().all(is_lower_hex) || version == "ff" {
            return None;
        }
        if version == VERSION_00 {
            if raw.len() != VERSION_00_LEN {
                return None;
            }
        } else if raw.len() < VERSION_00_LEN || raw.as_bytes().get(VERSION_00_LEN) != Some(&b'-') {
            return None;
        }
        let mut fields = raw.get(..VERSION_00_LEN)?.split('-');
        let (_version, trace_id, parent_id, flags) = (
            fields.next()?,
            fields.next()?,
            fields.next()?,
            fields.next()?,
        );
        if fields.next().is_some() {
            return None;
        }
        Some(Self {
            trace_id: TraceId::parse(trace_id)?,
            parent_id: SpanId::parse(parent_id)?,
            flags: TraceFlags::parse(flags)?,
        })
    }
}

impl fmt::Display for TraceParent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{VERSION_00}-{}-{}-{}",
            self.trace_id, self.parent_id, self.flags
        )
    }
}

/// What one unit of work is filed under: its place in a trace, and who — once
/// anyone knows.
///
/// One value rather than five arguments, because they travel together everywhere
/// the framework carries work and an edge that propagated the trace while
/// dropping the actor would reopen exactly the gap this closes.
///
/// Two members are shared rather than owned, and both are forced by the order
/// things happen in. The **actor** is a write-once slot: the edge opens the unit
/// of work before any guard has run, so at that moment nobody knows who is
/// calling. The **flags** are shared and updatable for the mirror reason: with an
/// observability stack installed, its sampler decides *after* the span exists,
/// and an outbound `traceparent` carrying a decision nobody made is a claim about
/// another system's data.
#[derive(Clone, Debug)]
pub struct Correlation {
    trace_id: TraceId,
    span_id: SpanId,
    parent_id: Option<SpanId>,
    /// Whether the parent ran in **another process**. A backend reads it to
    /// know there is no local span to look up, so claiming it for an in-process
    /// parent — a WS message under its connection, an MCP operation under its
    /// request — describes a hop that never happened.
    parent_is_remote: bool,
    tracestate: TraceState,
    shared: Arc<Shared>,
}

/// The two cells a correlation shares with its clones.
///
/// One allocation rather than two, and one refcount rather than two on a value
/// cloned per request, per message, per body poll and per child: they are always
/// created together and always travel together, so nothing is served by keeping
/// them apart.
#[derive(Debug)]
struct Shared {
    flags: AtomicU8,
    actor: OnceLock<Arc<str>>,
}

impl Correlation {
    /// Start a trace: new trace id, new span, no parent, no actor.
    ///
    /// What every edge that accepts work with no upstream writes — a scheduled
    /// tick, a job carrying no envelope trace, an endpoint mounted outside any
    /// transport — and what a transport writes when it *restarts* a trace it was
    /// not willing to continue.
    pub fn mint() -> Self {
        Self::open(
            TraceId::mint(),
            None,
            TraceFlags::started(),
            TraceState::default(),
            None,
        )
    }

    /// Continue a trace that arrived with the work: same trace id, the sender's
    /// span becomes this one's parent, and a **new span id**, because this is a
    /// new unit of work.
    ///
    /// The sender's flags are inherited whole, reserved bits included, which is
    /// only sound because the caller has already decided this source is
    /// trustworthy — inheriting `sampled` from an arbitrary client is the
    /// denial-of-service surface the specification warns about.
    ///
    /// `actor` is `Some` only where it cannot be re-derived — a queue job, whose
    /// worker holds no credential and can never authenticate anyone, so what the
    /// producer knew is the only answer there will ever be. Every other caller
    /// passes `None` and lets its guard fill the slot.
    pub fn continued(parent: TraceParent, tracestate: TraceState, actor: Option<&str>) -> Self {
        Self {
            parent_is_remote: true,
            ..Self::open(
                parent.trace_id,
                Some(parent.parent_id),
                parent.flags,
                tracestate,
                actor,
            )
        }
    }

    /// The unit of work this task is already serving, or a fresh trace where
    /// there is none.
    ///
    /// What every edge that *accepts* work writes: a socket taking its upgrade's
    /// trace, a producer sealing an envelope, an endpoint mounted outside any
    /// transport. Spelled once here because five sites wrote it as
    /// `current_correlation().unwrap_or_else(Correlation::mint)` — five copies of
    /// one decision, in a kernel that owns both halves.
    pub fn inherited() -> Self {
        current_correlation().unwrap_or_else(Self::mint)
    }

    /// A new unit of work inside the trace this one is already serving — a WS
    /// message on an open socket, a job enqueued mid-request. The trace, the
    /// flags, the state and the actor carry; the span is new and its parent is
    /// this one.
    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id,
            span_id: SpanId::mint(),
            parent_id: Some(self.span_id),
            // In-process by construction: the parent is the very value this was
            // called on.
            parent_is_remote: false,
            tracestate: self.tracestate.clone(),
            shared: Arc::clone(&self.shared),
        }
    }

    fn open(
        trace_id: TraceId,
        parent_id: Option<SpanId>,
        flags: TraceFlags,
        tracestate: TraceState,
        actor: Option<&str>,
    ) -> Self {
        let shared = Shared {
            flags: AtomicU8::new(flags.bits()),
            actor: OnceLock::new(),
        };
        if let Some(actor) = actor {
            let _ = shared.actor.set(Arc::from(actor));
        }
        Self {
            trace_id,
            span_id: SpanId::mint(),
            parent_id,
            parent_is_remote: false,
            tracestate,
            shared: Arc::new(shared),
        }
    }

    /// The distributed operation this belongs to.
    pub fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    /// This unit of work.
    pub fn span_id(&self) -> SpanId {
        self.span_id
    }

    /// The unit of work that caused this one, when it arrived with the work.
    pub fn parent_id(&self) -> Option<SpanId> {
        self.parent_id
    }

    /// Whether that parent ran in another process.
    pub fn parent_is_remote(&self) -> bool {
        self.parent_is_remote
    }

    /// The current flags, reserved bits included.
    pub fn flags(&self) -> TraceFlags {
        TraceFlags(self.shared.flags.load(Ordering::Relaxed))
    }

    /// Record the sampling decision an installed sampler actually made.
    ///
    /// Only the observability stack calls this, and only once its own decision
    /// exists. Reserved bits are left exactly as they arrived: this sets one
    /// bit, it does not replace the byte.
    #[doc(hidden)]
    pub fn set_sampled(&self, sampled: bool) {
        let mask = TraceFlags::SAMPLED;
        if sampled {
            self.shared.flags.fetch_or(mask, Ordering::Relaxed);
        } else {
            self.shared.flags.fetch_and(!mask, Ordering::Relaxed);
        }
    }

    /// The vendor state to forward untouched.
    pub fn tracestate(&self) -> &TraceState {
        &self.tracestate
    }

    /// What to send to whatever this unit of work calls next: **this** span is
    /// the parent, which is what makes the callee's span a child of ours rather
    /// than a sibling.
    pub fn traceparent(&self) -> TraceParent {
        TraceParent {
            trace_id: self.trace_id,
            parent_id: self.span_id,
            flags: self.flags(),
        }
    }

    /// Who is calling, if authentication has resolved anyone.
    pub fn actor_id(&self) -> Option<&str> {
        self.shared.actor.get().map(|actor| &**actor)
    }
}

/// The trace this task is serving.
///
/// `None` only where no edge opened one — a hand-built request in a unit test, a
/// task spawned outside the framework's own propagation. Every framework spawn
/// carries it, and every streaming response body, which is what makes the answer
/// usable rather than best-effort.
pub fn current_trace_id() -> Option<TraceId> {
    current_request_ctx(|ctx| ctx.correlation.trace_id)
}

/// The unit of work this task is running.
pub fn current_span_id() -> Option<SpanId> {
    current_request_ctx(|ctx| ctx.correlation.span_id)
}

/// The header value to send to whatever this unit of work calls next.
///
/// The framework ships no general outbound HTTP client by decision, so this is
/// what an application injects into its own — one call, no knowledge of the
/// format, and the trace continues into the service it calls.
pub fn current_traceparent() -> Option<TraceParent> {
    current_request_ctx(|ctx| ctx.correlation.traceparent())
}

/// The `tracestate` to forward beside it, when the caller sent one.
pub fn current_tracestate() -> Option<TraceState> {
    current_request_ctx(|ctx| ctx.correlation.tracestate.clone())
}

/// Who this unit of work is being served for — **`None` for an anonymous
/// caller**, and that is the answer, not a missing one.
///
/// No sentinel is returned for an unauthenticated visitor: `""` or
/// `"anonymous"` would be indistinguishable from an actor genuinely named that,
/// and a query counting anonymous traffic would silently count both.
///
/// # This is an audit identity, never an authorization input
///
/// It answers *who*, for a log line, an audit row, a `created_by` column. It
/// does **not** answer what they may do — that is the ambient `Ability`, decided
/// in a guard. Branching on this value in a service is the defect *no
/// authn/authz decision outside a guard* exists to forbid, and it is a `String`,
/// deliberately: there is nothing here to match a role against.
pub fn current_actor_id() -> Option<String> {
    current_request_ctx(|ctx| ctx.correlation.actor_id().map(str::to_owned)).flatten()
}

/// Record who is calling, once. Called by the authentication guard the moment it
/// resolves a principal — never by application code.
///
/// Write-once: a second call is ignored rather than overwriting. Two different
/// actors on one unit of work is not a state the framework has, and silently
/// taking the later one would make an audit trail depend on layer order.
#[doc(hidden)]
pub fn set_actor_id(actor_id: &str) {
    let _ = current_request_ctx(|ctx| ctx.correlation.shared.actor.set(Arc::from(actor_id)));
}

/// The current unit of work's correlation, for an edge that has to carry it
/// across a boundary the task-local cannot cross.
#[doc(hidden)]
pub fn current_correlation() -> Option<Correlation> {
    current_request_ctx(|ctx| ctx.correlation.clone())
}

tokio::task_local! {
    /// The ids the span about to be created belongs to — see [`with_pending_ids`].
    static PENDING_IDS: (TraceId, SpanId);
}

/// Publish this unit of work's ids for the duration of **creating** its span.
///
/// # Why a second, one-instruction-wide scope exists
///
/// An observability SDK mints its own trace and span ids when a span is created.
/// If it does, the framework has two ids for one unit of work — the exact
/// duplication that retiring `request_id` removed, reintroduced one layer down
/// and this time invisible, because logs would carry ours and the exported trace
/// theirs.
///
/// So the SDK is made to **adopt** these instead, through an `IdGenerator` that
/// reads this. It has to be a scope of its own rather than the request context:
/// the SDK's generator runs *inside* `info_span!`, and an edge creates its span
/// **before** installing the request context — so by the time the ambient
/// context exists, the ids have already been minted.
///
/// [`operation_span!`](crate::operation_span) is the only caller, which is what
/// makes every edge inherit the behaviour without knowing it exists.
#[doc(hidden)]
pub fn with_pending_ids<T>(correlation: &Correlation, create: impl FnOnce() -> T) -> T {
    PENDING_IDS.sync_scope((correlation.trace_id(), correlation.span_id()), create)
}

/// The ids the span being created belongs to, or `None` outside
/// [`with_pending_ids`] — where an SDK falls back to generating its own, which
/// is correct for a span the framework did not open.
#[doc(hidden)]
pub fn pending_ids() -> Option<(TraceId, SpanId)> {
    PENDING_IDS.try_with(|ids| *ids).ok()
}

/// What an observability stack does to a span the framework just opened.
///
/// Two things need doing that only an installed SDK can do — link a **remote
/// parent**, and record the **sampling decision** its own sampler made — and
/// both need a `Span` handle, which is the one thing a generator or a subscriber
/// layer never gets. `tracing_opentelemetry::OtelData` is private, so
/// `OpenTelemetrySpanExt::set_parent` is the only seam, and it takes a span.
type SpanLinker = fn(&crate::tracing::Span, &Correlation);

static LINKER: OnceLock<SpanLinker> = OnceLock::new();

/// Seed what runs on every span the framework opens. Called once, at boot, by
/// the observability stack — never by an application.
///
/// A seeded function pointer rather than a dependency, for the reason the whole
/// correlation design rests on: the kernel must not know an exporter exists. It
/// is the same shape `WsDataPipe` and `GraphqlVariablePipe` already use to reach
/// backwards across a crate boundary the dependency graph runs the other way.
///
/// **The span constructor is the right depth, and an HTTP interceptor was not.**
/// Enrichment bolted to a transport band reaches one edge; here it reaches every
/// edge that opens a unit of work, which is all of them — a queue job's exported
/// span linked to the enqueue that caused it without `nest-rs-queue` learning
/// what OpenTelemetry is.
#[doc(hidden)]
pub fn set_span_linker(linker: SpanLinker) {
    let _ = LINKER.set(linker);
}

/// Run the seeded linker, if any. Called by
/// [`operation_span!`](crate::operation_span) and nowhere else.
#[doc(hidden)]
pub fn link_span(span: &crate::tracing::Span, correlation: &Correlation) {
    if let Some(linker) = LINKER.get() {
        linker(span, correlation);
    }
}

/// Lower-case hex into a caller-owned buffer.
///
/// Table-driven and written in one `write_str` rather than a `write!` per byte:
/// this runs at least three times per HTTP request — the span's `span_id`, the
/// access line's, and the `traceresponse` header's — and once more per continued
/// trace, so the formatting machinery `{:02x}` drags in is paid on the hot path
/// of every request the framework serves.
fn hex<'a>(bytes: &[u8], out: &'a mut [u8]) -> &'a str {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for (byte, pair) in bytes.iter().zip(out.chunks_exact_mut(2)) {
        pair[0] = DIGITS[usize::from(byte >> 4)];
        pair[1] = DIGITS[usize::from(byte & 0x0f)];
    }
    // Every byte written came from `DIGITS`, which is ASCII.
    str::from_utf8(out).unwrap_or_default()
}

/// Lower-case hex only: the grammar is explicit, and accepting upper case would
/// make one trace id have two spellings that no backend joins together.
const fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (byte >= b'a' && byte <= b'f')
}

const fn unhex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Open the framework's **operation span** — one per unit of work, carrying the
/// canonical field vocabulary so every edge declares it identically.
///
/// Three fields are always declared, and declaring them is the whole point:
/// `tracing` fixes a span's field set at creation, so a field nobody declared can
/// never be recorded later. `AuthnGuard`'s
/// `Span::current().record("actor_id", …)` is a **silent no-op** at any site
/// whose span omitted it — which is exactly how `actor_id` came to be recorded
/// nowhere at all while every doc claimed otherwise.
///
/// - `trace_id` and `span_id` — the OpenTelemetry log-record vocabulary, so a
///   backend joins these events to the trace with no per-deployment field
///   mapping;
/// - `parent_span_id` — the unit of work that caused this one, recorded only
///   when the work arrived from somewhere. Across a process boundary it is the
///   *only* place the cause is legible: a worker has no span stack to walk back
///   into the request that enqueued the job;
/// - `actor_id` — declared [`Empty`](tracing::field::Empty), filled by the authn
///   guard if and when it resolves a principal.
///
/// `kind` is **required**, and it is OpenTelemetry's span kind (`"server"`,
/// `"consumer"`, `"internal"`, …). It is an argument rather than a field a call
/// site may add because the kind is canonical vocabulary: a backend classifies a
/// span by it, and an edge that forgot one would export as `internal` and be
/// invisible to every messaging view. Required means a new edge cannot ship
/// unclassified.
///
/// **All three of the caller's slots are constants, and none may be a literal.**
/// The target is the owning crate's (`nest_rs_http::target::HTTP`), the kind is
/// one of [`operation_log::kind`](crate::operation_log::kind), and the name is
/// one of [`operation_log::unit`](crate::operation_log::unit) — the same
/// constant the edge's operation line reads for its `name:` and its message,
/// which is what stops one unit of work from having two spellings. The `units`
/// join in `nest-rs-conformance` fails on a literal at any of the three, and it
/// reads this example too.
///
/// The [`Correlation`] is a **required positional argument** rather than read
/// from the ambient context, for two reasons that both bite: an edge opens its
/// span *before* installing the context (so an ambient read would silently
/// record an empty id), and a required argument is the only spelling a caller
/// cannot forget.
///
/// ```
/// # use nest_rs_core::{Correlation, operation_span};
/// use nest_rs_core::operation_log::{kind, unit};
///
/// // An edge reads its own crate's — `nest_rs_http::target::HTTP` at the real
/// // call site. The kernel cannot name it from inside its own doctest, and a
/// // target is declared exactly once per crate, so the declaration stands in.
/// const TARGET: &str = "nest_rs::http";
///
/// let correlation = Correlation::mint();
/// let span = operation_span!(
///     target: TARGET,
///     kind: kind::SERVER,
///     unit::HTTP_REQUEST,
///     &correlation,
///     http.request.method = "GET",
/// );
/// ```
#[macro_export]
macro_rules! operation_span {
    (target: $target:expr, kind: $kind:expr, $name:expr, $correlation:expr $(, $($field:tt)*)?) => {{
        let __correlation = &$correlation;
        // Created inside the id scope so an installed SDK adopts these ids
        // rather than minting a second pair nobody can join against.
        let __span = $crate::trace_context::with_pending_ids(__correlation, || {
            $crate::tracing::info_span!(
                target: $target,
                $name,
                otel.kind = $kind,
                trace_id = %__correlation.trace_id(),
                span_id = %__correlation.span_id(),
                parent_span_id = $crate::tracing::field::Empty,
                actor_id = $crate::tracing::field::Empty,
                $($($field)*)?
            )
        });
        // The unit of work that caused this one. Only present where the work
        // arrived from somewhere, and the only place it can be read at all once
        // the cause was in another process — a job's parent is not in any span
        // stack the worker has.
        if let Some(parent_span_id) = __correlation.parent_id() {
            __span.record("parent_span_id", $crate::tracing::field::display(parent_span_id));
        }
        // Already known on an inherited unit of work — a WS message on an
        // authenticated socket, a job whose producer knew the caller. Nothing
        // re-authenticates on those paths, so the span would otherwise declare
        // the field and never see it filled.
        if let Some(actor_id) = __correlation.actor_id() {
            __span.record("actor_id", actor_id);
        }
        // Last, and only here: an installed observability stack links the remote
        // parent and records its sampler's verdict. Every edge inherits that by
        // opening its span through this macro, which is why the enrichment is
        // not a transport's layer.
        $crate::trace_context::link_span(&__span, __correlation);
        __span
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_trace_id_is_a_uuid_v7_and_a_conformant_trace_id() {
        let id = TraceId::mint();
        assert_eq!(Uuid::from_bytes(id.to_bytes()).get_version_num(), 7, "{id}",);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 32, "{hex}");
        assert!(hex.bytes().all(is_lower_hex), "{hex}");
        assert_ne!(hex, "0".repeat(32), "the all-zero value is invalid");
    }

    /// The assertion `TraceFlags::RANDOM` makes on every trace we start. Level 2
    /// requires the right-most 7 bytes be uniformly random; a v7's 62-bit tail
    /// covers them, and this is what stops that claim from being a comment.
    #[test]
    fn the_right_most_seven_bytes_of_a_minted_trace_id_vary() {
        let tails: std::collections::HashSet<_> = (0..64)
            .map(|_| TraceId::mint().to_bytes()[9..].to_vec())
            .collect();
        assert_eq!(tails.len(), 64, "the random tail must not repeat");
        assert!(TraceFlags::started().bits() & TraceFlags::RANDOM != 0);
    }

    #[test]
    fn minted_ids_are_distinct() {
        assert_ne!(TraceId::mint(), TraceId::mint());
        assert_ne!(SpanId::mint(), SpanId::mint());
    }

    #[test]
    fn a_span_id_round_trips_through_its_wire_form() {
        let id = SpanId::mint();
        let hex = id.to_hex();
        assert_eq!(hex.len(), 16, "{hex}");
        assert_eq!(SpanId::parse(&hex), Some(id));
    }

    #[test]
    fn a_traceparent_round_trips() {
        let correlation = Correlation::mint();
        let header = correlation.traceparent().to_string();
        assert_eq!(header.len(), VERSION_00_LEN, "{header}");

        let parsed = TraceParent::parse(&header).expect("our own header parses");
        assert_eq!(parsed.trace_id, correlation.trace_id());
        assert_eq!(
            parsed.parent_id,
            correlation.span_id(),
            "our span is what the callee will call its parent",
        );
        assert!(parsed.flags.is_sampled());
    }

    /// Every one of these is a header a real deployment can send, and each is a
    /// rule the specification states. A parser that is lax here silently adopts
    /// a trace nobody started; one that is strict in the wrong place restarts
    /// every trace the day an upstream hop moves to version `01`.
    #[test]
    fn the_specs_refusals_are_all_implemented() {
        let valid = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert!(
            TraceParent::parse(valid).is_some(),
            "the spec's own example"
        );

        for (raw, why) in [
            ("", "empty"),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
                "truncated",
            ),
            (
                "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
                "version ff is reserved",
            ),
            (
                "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
                "an all-zero trace-id is invalid",
            ),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
                "an all-zero parent-id is invalid",
            ),
            (
                "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
                "upper case is not the grammar",
            ),
            (
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
                "version 00 is exactly 55 characters",
            ),
            (
                "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01extra",
                "a higher version must still be delimited at 55",
            ),
        ] {
            assert!(TraceParent::parse(raw).is_none(), "{why}: {raw:?}");
        }
    }

    /// The forward-compatibility rule, and the one most likely to be got wrong:
    /// a version this framework has never heard of is still parsed as far as it
    /// understands, so one upgraded hop upstream does not restart every trace.
    #[test]
    fn a_higher_version_is_read_as_far_as_this_one_understands() {
        let future = "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-what-comes-next";
        let parsed = TraceParent::parse(future).expect("a future version is still readable");
        assert_eq!(parsed.trace_id.to_hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert!(parsed.flags.is_sampled());
    }

    /// Reserved flag bits are somebody else's meaning. Dropping them would
    /// quietly strip a future version's signal from every trace we forward.
    #[test]
    fn reserved_flag_bits_survive_a_round_trip() {
        let flags = TraceFlags::parse("fd").expect("valid hex");
        assert_eq!(flags.bits(), 0xfd);
        assert!(flags.is_sampled());
        assert_eq!(flags.to_string(), "fd");
    }

    #[test]
    fn continuing_keeps_the_trace_and_starts_a_new_span() {
        let caller = Correlation::mint();
        let continued = Correlation::continued(caller.traceparent(), TraceState::default(), None);

        assert_eq!(continued.trace_id(), caller.trace_id(), "one trace");
        assert_ne!(continued.span_id(), caller.span_id(), "a new unit of work");
        assert_eq!(
            continued.parent_id(),
            Some(caller.span_id()),
            "and it names who caused it — the relation a flat id could not express",
        );
    }

    /// The relation that motivated the whole change: a request that enqueues a
    /// job is one trace and two spans, and the job knows which request caused it.
    #[test]
    fn a_child_names_its_parent_and_shares_the_actor() {
        let request = Correlation::mint();
        set_actor_on(&request, "alice-42");
        let job = request.child();

        assert_eq!(job.trace_id(), request.trace_id());
        assert_eq!(job.parent_id(), Some(request.span_id()));
        assert_eq!(job.actor_id(), Some("alice-42"), "nothing re-authenticates");
    }

    fn set_actor_on(correlation: &Correlation, actor: &str) {
        let _ = correlation.shared.actor.set(Arc::from(actor));
    }

    /// A sampler decides after the span exists, so the decision has to be
    /// writable — and it must set one bit rather than replace the byte, or the
    /// reserved bits of whoever we are continuing go with it.
    #[test]
    fn a_sampling_decision_is_recorded_without_touching_reserved_bits() {
        let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-fd")
            .expect("valid");
        let correlation = Correlation::continued(parent, TraceState::default(), None);

        correlation.set_sampled(false);
        assert!(!correlation.flags().is_sampled());
        assert_eq!(correlation.flags().bits(), 0xfc, "reserved bits untouched");

        correlation.set_sampled(true);
        assert_eq!(correlation.flags().bits(), 0xfd);
    }

    #[test]
    fn tracestate_is_carried_verbatim_and_refused_whole_when_unusable() {
        assert_eq!(
            TraceState::adopt("  rojo=00f067aa0ba902b7,congo=t61rcWkgMzE  ").as_str(),
            Some("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
        );
        for unusable in ["", "   ", "vendor=value\nx=1"] {
            assert_eq!(TraceState::adopt(unusable).as_str(), None, "{unusable:?}");
        }
        assert_eq!(
            TraceState::adopt(&"x".repeat(TraceState::MAX_LEN + 1)).as_str(),
            None,
            "an unbounded state would ride into every outbound request",
        );
    }

    /// Off any edge there is no unit of work to name, and saying so is more
    /// useful than inventing an id nothing else will ever mention.
    #[test]
    fn there_is_no_ambient_trace_outside_a_unit_of_work() {
        assert!(current_trace_id().is_none());
        assert!(current_span_id().is_none());
        assert!(current_traceparent().is_none());
    }
}
