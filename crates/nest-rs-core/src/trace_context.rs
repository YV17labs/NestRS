//! W3C Trace Context — the framework's correlation primitive
//! (`.claude/decisions/trace-context-over-request-id.md`).
//!
//! | Field | What it names | Present |
//! |---|---|---|
//! | `trace_id` | the whole distributed operation | **always** |
//! | `span_id` | *this* unit of work inside it | **always** |
//! | `actor_id` | who it is being served for | once a principal is resolved |
//!
//! Whoever accepts the work continues a trace only from a source it trusts; an
//! untrusted or malformed `traceparent` **restarts** it, the spec's front-gate
//! mutation.

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use uuid::Uuid;

use crate::request_scope::current_request_ctx;

/// The identifier of one distributed operation — 16 bytes, spelled as 32
/// lowercase hex characters on the wire.
///
/// A minted id is a UUID v7, so it sorts by time; its right-most 7 bytes fall
/// in the v7's random tail, which is what lets [`TraceFlags::RANDOM`] be set
/// (Trace Context Level 2).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct TraceId(Uuid);

impl TraceId {
    /// Start a new trace.
    pub fn mint() -> Self {
        Self(Uuid::now_v7())
    }

    /// Parse the 32-hex form carried by `traceparent`; `None` for a wrong
    /// length, a non-hex or upper-case character, or the all-zero value.
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 32 || !hex.bytes().all(is_lower_hex) {
            return None;
        }
        let parsed = Uuid::parse_str(hex).ok()?;
        (!parsed.is_nil()).then_some(Self(parsed))
    }

    /// The wire spelling: 32 lowercase hex characters, no separators — what the
    /// `trace_id` log field carries.
    pub fn to_hex(self) -> String {
        self.to_string()
    }

    /// The raw bytes, for an SDK that takes them directly.
    pub fn to_bytes(self) -> [u8; 16] {
        *self.0.as_bytes()
    }
}

impl fmt::Display for TraceId {
    /// One `write_str`, not a nested `write!`: every log line renders this.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0u8; uuid::fmt::Simple::LENGTH];
        f.write_str(self.0.simple().encode_lower(&mut buf))
    }
}

/// The identifier of one unit of work inside a trace — 8 bytes, 16 hex
/// characters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SpanId([u8; 8]);

impl SpanId {
    /// A fresh span id.
    ///
    /// A v4's two halves are XORed because a v4 fixes six version and variant
    /// bits; one half taken raw would carry predictable bits.
    pub fn mint() -> Self {
        let random = Uuid::new_v4().as_u128();
        Self((((random >> 64) as u64) ^ (random as u64)).to_be_bytes())
    }

    /// Parse the 16-hex form carried by `traceparent`; `None` for a wrong
    /// length, a non-hex or upper-case character, or the all-zero value.
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 16 || !hex.bytes().all(is_lower_hex) {
            return None;
        }
        let mut bytes = [0_u8; 8];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
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
        f.write_str(__private::hex(&self.0, &mut [0; 16]))
    }
}

/// The one byte of `trace-flags`, reserved bits included: the spec requires an
/// unknown bit be forwarded untouched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceFlags(u8);

impl TraceFlags {
    /// The caller's recording decision; set on a started trace, since with no
    /// sampler installed everything is recorded.
    pub const SAMPLED: u8 = 0x01;
    /// Asserts the right-most 7 bytes of the trace id are uniformly random.
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
        f.write_str(__private::hex(&[self.0], &mut [0; 2]))
    }
}

/// Vendor-specific trace state, held opaquely and forwarded verbatim (§4.1
/// makes forwarding a MUST), dropped only with a `traceparent` not continued.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TraceState(Option<Arc<str>>);

impl TraceState {
    /// §3.3.1.5's floor on what must survive, used as the cap: a longer value
    /// is truncated, never dropped.
    const MAX_LEN: usize = 512;

    /// A list-member longer than this is the first thing §3.3.1.5 drops.
    const OVERSIZE_MEMBER: usize = 128;

    /// §3.3.1.2's member cap, enforced only on a value this framework truncates.
    const MAX_MEMBERS: usize = 32;

    /// Adopt a `tracestate` header from a source the caller has decided to
    /// trust.
    ///
    /// Refused whole when empty, when it carries a byte that would end a header
    /// or a JSON string, or when truncation leaves nothing. A value that fits is
    /// forwarded verbatim, wider than §3.3.1's grammar on purpose (§4.1); an
    /// over-long one is truncated by whole list-members.
    pub fn adopt(raw: &str) -> Self {
        let raw = raw.trim();
        if raw.is_empty()
            || !raw
                .bytes()
                .all(|b| b.is_ascii_graphic() || b == b' ' || b == b'\t')
        {
            return Self(None);
        }
        if raw.len() <= Self::MAX_LEN {
            return Self(Some(Arc::from(raw)));
        }
        Self(Self::truncate(raw).map(Arc::from))
    }

    /// Truncate by whole list-members in §3.3.1.5's order: oversize members
    /// first, then from the end.
    fn truncate(raw: &str) -> Option<String> {
        let mut members: Vec<&str> = raw
            .split(',')
            .map(str::trim)
            .filter(|member| !member.is_empty())
            .collect();

        // Oversize members go only until the value fits: they are not illegal.
        // One pass, since the client chose the header's size.
        let mut excess = (members.iter().map(|member| member.len()).sum::<usize>()
            + members.len().saturating_sub(1))
        .saturating_sub(Self::MAX_LEN);
        members.retain(|member| {
            let drop_it = excess > 0 && member.len() > Self::OVERSIZE_MEMBER;
            if drop_it {
                excess = excess.saturating_sub(member.len() + 1);
            }
            !drop_it
        });

        // `len` counts each kept member plus its trailing separator, so the
        // joined length is `len - 1`.
        let mut len = 0;
        let mut kept = 0;
        for member in &members {
            if kept == Self::MAX_MEMBERS || len + member.len() > Self::MAX_LEN {
                break;
            }
            len += member.len() + 1;
            kept += 1;
        }

        (kept > 0).then(|| members[..kept].join(","))
    }

    /// The header value to forward, or `None` when there is no state to carry.
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// The canonical correlation field names.
///
/// [`operation_span!`](crate::operation_span) spells them as literals, since
/// `tracing`'s macro grammar takes no constant there; every other site uses these.
pub mod field {
    /// The whole distributed operation. On every span and every log line.
    pub const TRACE_ID: &str = "trace_id";
    /// This unit of work inside the trace. On every span and every log line.
    pub const SPAN_ID: &str = "span_id";
    /// The unit of work that caused this one; a span field, never a line field.
    pub const PARENT_SPAN_ID: &str = "parent_span_id";
    /// Who the work is being served for — an audit identity, never an
    /// authorization input; absent means anonymous.
    pub const ACTOR_ID: &str = "actor_id";
    /// The sampling byte, on JSON lines only.
    pub const TRACE_FLAGS: &str = "trace_flags";
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

/// The one version this framework writes; higher versions are still read.
const VERSION_00: &str = "00";
/// `00-` + 32 + `-` + 16 + `-` + 2.
const VERSION_00_LEN: usize = 55;

impl TraceParent {
    /// Parse a `traceparent` header value, to the letter of the specification.
    ///
    /// - **version `ff`** is reserved and invalid;
    /// - **version `00`** must be *exactly* 55 characters;
    /// - **a higher version** is read as `00` for its first 55 characters, and
    ///   character 55 must be the end of the string or a `-` (§3.2.4);
    /// - **an all-zero trace-id or parent-id** is invalid.
    ///
    /// `None` means *restart the trace*, never *fail the request*.
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
        } else if raw.len() < VERSION_00_LEN
            || !matches!(raw.as_bytes().get(VERSION_00_LEN), None | Some(&b'-'))
        {
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
/// The actor and the flags are shared with clones: the actor is filled once a
/// guard runs, after the unit opened, and the flags once a sampler decides,
/// after the span exists.
#[derive(Clone, Debug)]
pub struct Correlation {
    trace_id: TraceId,
    span_id: SpanId,
    parent_id: Option<SpanId>,
    /// Whether the parent ran in **another process**: a backend then looks up
    /// no local span, so an in-process parent never sets it.
    parent_is_remote: bool,
    tracestate: TraceState,
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    flags: AtomicU8,
    actor: OnceLock<Arc<str>>,
}

impl Correlation {
    /// Start a trace: new trace id, new span, no parent.
    ///
    /// `actor` is `Some` only where no guard will re-derive it — a queue job
    /// whose envelope names its actor but carries no usable `traceparent`;
    /// every other caller passes `None`.
    pub fn minted(actor: Option<&str>) -> Self {
        Self::open(
            TraceId::mint(),
            None,
            TraceFlags::started(),
            TraceState::default(),
            actor,
        )
    }

    /// Continue a trace that arrived with the work: same trace id, the sender's
    /// span becomes this one's parent, and a **new span id**, because this is a
    /// new unit of work.
    ///
    /// The sender's flags are inherited whole, so call this only for a source
    /// already trusted: an untrusted `sampled` is the spec's denial-of-service
    /// surface. `actor` is `Some` only for a queue job, whose worker cannot
    /// re-authenticate; every other caller passes `None`.
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
    pub fn inherited() -> Self {
        __private::current_correlation().unwrap_or_else(|| Self::minted(None))
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
            set_actor(&shared, actor);
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

    /// The vendor state to forward untouched.
    pub fn tracestate(&self) -> &TraceState {
        &self.tracestate
    }

    /// What to send to whatever this unit of work calls next, with **this**
    /// span as the parent.
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

/// The trace this task is serving; `None` only where no edge opened one, such
/// as a task spawned outside the framework.
pub fn current_trace_id() -> Option<TraceId> {
    current_request_ctx(|ctx| ctx.correlation.trace_id)
}

/// The unit of work this task is running.
pub fn current_span_id() -> Option<SpanId> {
    current_request_ctx(|ctx| ctx.correlation.span_id)
}

/// The header value to send to whatever this unit of work calls next, for an
/// application to set on its own outbound client.
pub fn current_traceparent() -> Option<TraceParent> {
    current_request_ctx(|ctx| ctx.correlation.traceparent())
}

/// The `tracestate` to forward beside it, when the caller sent one.
pub fn current_tracestate() -> Option<TraceState> {
    current_request_ctx(|ctx| ctx.correlation.tracestate.clone())
}

/// Who this unit of work is being served for — **`None` for an anonymous
/// caller**.
///
/// An audit identity — a log line, an audit row, a `created_by` column — never
/// an authorization input: what a caller may do is the ambient `Ability`,
/// decided in a guard.
pub fn current_actor_id() -> Option<String> {
    current_request_ctx(|ctx| ctx.correlation.actor_id().map(str::to_owned)).flatten()
}

/// The one gate on the write-once actor slot, passed by both writers: an empty
/// actor would burn the slot, and a queue job can never re-derive it.
fn set_actor(shared: &Shared, actor_id: &str) {
    if actor_id.is_empty() {
        return;
    }
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the slot is write-once: the first actor resolved for the work stands"
    )]
    let _ = shared.actor.set(Arc::from(actor_id));
}

/// `trace_context` is public: its tier-2 items live here, reached only
/// through the crate's `__private`.
pub(crate) mod __private {
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    use super::{Correlation, SpanId, TraceFlags, TraceId};
    use crate::request_scope::current_request_ctx;

    /// Record the sampling decision an installed sampler made, leaving the
    /// reserved bits as they arrived.
    pub fn set_sampled(correlation: &Correlation, sampled: bool) {
        let mask = TraceFlags::SAMPLED;
        if sampled {
            correlation.shared.flags.fetch_or(mask, Ordering::Relaxed);
        } else {
            correlation.shared.flags.fetch_and(!mask, Ordering::Relaxed);
        }
    }

    /// Record who is calling, once — by the authentication guard, never by
    /// application code.
    ///
    /// A second call is ignored, and an empty actor is refused.
    pub fn set_actor_id(actor_id: &str) {
        current_request_ctx(|ctx| super::set_actor(&ctx.correlation.shared, actor_id));
    }

    /// The current unit of work's correlation, for an edge that has to carry it
    /// across a boundary the task-local cannot cross.
    pub fn current_correlation() -> Option<Correlation> {
        current_request_ctx(|ctx| ctx.correlation.clone())
    }

    tokio::task_local! {
        /// The ids the span about to be created belongs to — see [`with_pending_ids`].
        static PENDING_IDS: (TraceId, SpanId);
    }

    /// Publish this unit of work's ids for the duration of **creating** its span,
    /// so an SDK's `IdGenerator` adopts them instead of minting its own.
    ///
    /// A scope of its own because the generator runs inside `info_span!`, before
    /// the edge installs the request context.
    pub fn with_pending_ids<T>(correlation: &Correlation, create: impl FnOnce() -> T) -> T {
        PENDING_IDS.sync_scope((correlation.trace_id(), correlation.span_id()), create)
    }

    /// The ids the span being created belongs to, or `None` outside
    /// [`with_pending_ids`].
    pub fn pending_ids() -> Option<(TraceId, SpanId)> {
        PENDING_IDS.try_with(|ids| *ids).ok()
    }

    /// What an observability stack does to a span the framework just opened.
    ///
    /// It needs the `Span` handle: `tracing_opentelemetry::OtelData` is private, so
    /// `OpenTelemetrySpanExt::set_parent` is the only seam.
    type SpanLinker = fn(&crate::tracing::Span, &Correlation);

    static LINKER: OnceLock<SpanLinker> = OnceLock::new();

    /// Seed what runs on every span the framework opens. Called once, at boot, by
    /// the observability stack — never by an application.
    pub fn set_span_linker(linker: SpanLinker) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "seeded once at boot; a second install keeps the first linker"
        )]
        let _ = LINKER.set(linker);
    }

    /// Run the seeded linker, if any.
    pub fn link_span(span: &crate::tracing::Span, correlation: &Correlation) {
        if let Some(linker) = LINKER.get() {
            linker(span, correlation);
        }
    }

    /// Lower-case hex into a caller-owned buffer of exactly twice `bytes`' length,
    /// table-driven rather than `{:02x}` per byte: it runs several times per request.
    pub fn hex<'a>(bytes: &[u8], out: &'a mut [u8]) -> &'a str {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for (byte, pair) in bytes.iter().zip(out.as_chunks_mut::<2>().0) {
            pair[0] = DIGITS[usize::from(byte >> 4)];
            pair[1] = DIGITS[usize::from(byte & 0x0f)];
        }
        // Every byte written came from `DIGITS`, which is ASCII.
        str::from_utf8(out).unwrap_or_default()
    }
}

/// Lower-case only: upper case would give one id two spellings no backend joins.
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
/// `tracing` fixes a span's fields at creation, so recording an undeclared one
/// is a silent no-op; these are always declared:
///
/// - `trace_id`, `span_id` — from the correlation;
/// - `parent_span_id` — recorded only when the work arrived from somewhere;
/// - `actor_id` — filled from an inherited correlation, else by the authn guard;
/// - `error.type`, `otel.status_code` — filled by
///   [`operation_log::record_outcome`](crate::operation_log::record_outcome)
///   when the unit ends in anything but `ok`.
///
/// The first argument is a path to a [`Unit`](crate::operation_log::Unit),
/// giving the span's name, target and `otel.kind`; a literal does not match,
/// and a unit another crate declared is a compile error. The [`Correlation`] is
/// passed rather than read ambiently: an edge opens its span before installing
/// the context. The worked example is `nest_rs_http::unit`'s.
#[macro_export]
macro_rules! operation_span {
    ($unit:path, $correlation:expr $(, $($field:tt)*)?) => {{
        const _: () = ::core::assert!(
            $crate::__private::unit_opened_by(&$unit, ::core::env!("CARGO_PKG_NAME")),
            "a unit is opened only by the crate that declares it",
        );
        let __correlation = &$correlation;
        let __span = $crate::__private::with_pending_ids(__correlation, || {
            $crate::tracing::info_span!(
                target: $unit.target(),
                $unit.name(),
                otel.kind = $unit.kind().as_str(),
                trace_id = %__correlation.trace_id(),
                span_id = %__correlation.span_id(),
                parent_span_id = $crate::tracing::field::Empty,
                actor_id = $crate::tracing::field::Empty,
                "error.type" = $crate::tracing::field::Empty,
                otel.status_code = $crate::tracing::field::Empty,
                $($($field)*)?
            )
        });
        if let Some(parent_span_id) = __correlation.parent_id() {
            __span.record(
                $crate::trace_context::field::PARENT_SPAN_ID,
                $crate::tracing::field::display(parent_span_id),
            );
        }
        // Nothing re-authenticates an inherited unit (a WS message, a job), so
        // the span is filled here or never.
        if let Some(actor_id) = __correlation.actor_id() {
            __span.record($crate::trace_context::field::ACTOR_ID, actor_id);
        }
        $crate::__private::link_span(&__span, __correlation);
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
        let correlation = Correlation::minted(None);
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

    #[test]
    fn a_higher_version_is_read_as_far_as_this_one_understands() {
        let future = "cc-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-what-comes-next";
        let parsed = TraceParent::parse(future).expect("a future version is still readable");
        assert_eq!(parsed.trace_id.to_hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert!(parsed.flags.is_sampled());

        // §3.2.4: a higher version appending nothing ends at its flags.
        let minimal = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert_eq!(minimal.len(), VERSION_00_LEN);
        let parsed = TraceParent::parse(minimal)
            .expect("flags at the end of the string are the spec's own alternative");
        assert_eq!(parsed.parent_id.to_hex(), "00f067aa0ba902b7");
        assert!(parsed.flags.is_sampled());
    }

    #[test]
    fn reserved_flag_bits_survive_a_round_trip() {
        let flags = TraceFlags::parse("fd").expect("valid hex");
        assert_eq!(flags.bits(), 0xfd);
        assert!(flags.is_sampled());
        assert_eq!(flags.to_string(), "fd");
    }

    #[test]
    fn continuing_keeps_the_trace_and_starts_a_new_span() {
        let caller = Correlation::minted(None);
        let continued = Correlation::continued(caller.traceparent(), TraceState::default(), None);

        assert_eq!(continued.trace_id(), caller.trace_id(), "one trace");
        assert_ne!(continued.span_id(), caller.span_id(), "a new unit of work");
        assert_eq!(
            continued.parent_id(),
            Some(caller.span_id()),
            "and it names who caused it — the relation a flat id could not express",
        );
    }

    #[test]
    fn a_child_names_its_parent_and_shares_the_actor() {
        let request = Correlation::minted(None);
        set_actor_on(&request, "alice-42");
        let job = request.child();

        assert_eq!(job.trace_id(), request.trace_id());
        assert_eq!(job.parent_id(), Some(request.span_id()));
        assert_eq!(job.actor_id(), Some("alice-42"), "nothing re-authenticates");
    }

    fn set_actor_on(correlation: &Correlation, actor: &str) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the test fills a slot it just created empty"
        )]
        let _ = correlation.shared.actor.set(Arc::from(actor));
    }

    #[test]
    fn a_sampling_decision_is_recorded_without_touching_reserved_bits() {
        let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-fd")
            .expect("valid");
        let correlation = Correlation::continued(parent, TraceState::default(), None);

        __private::set_sampled(&correlation, false);
        assert!(!correlation.flags().is_sampled());
        assert_eq!(correlation.flags().bits(), 0xfc, "reserved bits untouched");

        __private::set_sampled(&correlation, true);
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
            "one member too large to keep leaves nothing to carry",
        );
    }

    #[test]
    fn an_over_long_tracestate_is_truncated_by_whole_members_not_dropped() {
        let small = "a=1,b=2,c=3";
        let oversize = format!("fat={}", "z".repeat(TraceState::OVERSIZE_MEMBER));
        let filler: Vec<String> = (0..20)
            .map(|i| format!("v{i}={}", "y".repeat(40)))
            .collect();
        let raw = format!("{small},{oversize},{}", filler.join(","));
        assert!(raw.len() > TraceState::MAX_LEN);

        let adopted = TraceState::adopt(&raw);
        let kept = adopted.as_str().expect("vendor state survives truncation");

        assert!(kept.len() <= TraceState::MAX_LEN);
        assert!(
            kept.starts_with(small),
            "members are dropped from the end, so the front survives: {kept}",
        );
        assert!(
            !kept.contains("fat="),
            "the oversize member goes first: {kept}",
        );
        for member in kept.split(',') {
            assert!(
                raw.split(',')
                    .map(str::trim)
                    .any(|original| original == member),
                "a kept member is a whole original member, never a slice: {member}",
            );
        }
        assert!(
            kept.split(',').count() <= TraceState::MAX_MEMBERS,
            "§3.3.1.2 caps a list this framework authors at 32 members: {kept}",
        );
    }

    #[test]
    fn an_empty_actor_arriving_with_the_work_is_refused_too() {
        let parent = TraceParent {
            trace_id: TraceId::mint(),
            parent_id: SpanId::mint(),
            flags: TraceFlags::started(),
        };
        let continued = Correlation::continued(parent, TraceState::default(), Some(""));
        assert_eq!(continued.actor_id(), None);

        let named = Correlation::continued(parent, TraceState::default(), Some("alice"));
        assert_eq!(named.actor_id(), Some("alice"));
    }

    #[tokio::test]
    async fn an_empty_actor_is_refused_and_does_not_burn_the_write_once_slot() {
        let correlation = Correlation::minted(None);
        crate::request_scope::with_request_scope(None, correlation, async {
            __private::set_actor_id("");
            assert_eq!(
                current_actor_id(),
                None,
                "an empty actor is absence, not an actor named `\"\"`",
            );

            __private::set_actor_id("alice");
            assert_eq!(
                current_actor_id().as_deref(),
                Some("alice"),
                "the slot must still be free for the real principal",
            );
        })
        .await;
    }

    #[test]
    fn an_oversize_member_is_dropped_only_while_the_value_still_does_not_fit() {
        let raw = format!("big={},mid={}", "x".repeat(400), "y".repeat(200));
        assert!(raw.len() > TraceState::MAX_LEN);

        let adopted = TraceState::adopt(&raw);
        let kept = adopted
            .as_str()
            .expect("dropping the larger member leaves one that fits");

        assert!(kept.starts_with("mid="), "{kept}");
        assert!(!kept.contains("big="), "{kept}");
        assert!(kept.len() <= TraceState::MAX_LEN);
    }

    #[test]
    fn truncating_a_hostile_header_costs_one_pass_not_one_per_member() {
        // A quadratic truncate returns the right answer, so only a clock catches
        // it; the bound is loose enough not to flake on a loaded machine.
        let mut hostile = "a,".repeat(262_144);
        hostile.pop();
        assert!(hostile.len() > 500_000, "{} bytes", hostile.len());

        let started = std::time::Instant::now();
        let adopted = TraceState::adopt(&hostile);
        let elapsed = started.elapsed();

        let kept = adopted
            .as_str()
            .expect("a hostile header still yields state");
        assert!(kept.len() <= TraceState::MAX_LEN);
        // The only fixture where the 32-member cap, not the length, binds.
        assert_eq!(kept.split(',').count(), TraceState::MAX_MEMBERS);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "truncation went superlinear again: {elapsed:?} for {} bytes",
            hostile.len(),
        );
    }

    #[test]
    fn there_is_no_ambient_trace_outside_a_unit_of_work() {
        assert!(current_trace_id().is_none());
        assert!(current_span_id().is_none());
        assert!(current_traceparent().is_none());
    }
}
