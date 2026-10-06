//! Every error the boot can fail with, and [`DecodeError`] — the one error the
//! kernel lends every edge rather than raises.
//!
//! They are here rather than beside the pass that raises them because half of
//! them are not the access graph's at all — `DuplicateProviderError`,
//! `ContestedDeclarationError`, `UnresolvedFactoryError`, `LateFactoryError` and
//! `FactoryCycleError` are constructed only in [`app`](crate::app), by the
//! registration and factory phases, `ProviderCycleError` by the `#[module]`
//! expansion's register phase, `UncollectedImportError` by a dynamic import's
//! register, and `BudgetPastNetError` by the budget check the boot ends on. Filed
//! under `access.rs` the file's name was a claim about all of them and false
//! for half: from the type a reader derived the wrong file, and from the file
//! they were offered errors its own pass never raises.
//!
//! This is the role table's own row — *domain error → `error.rs`* — and eleven
//! `nest-rs-*` crates already carry one; the kernel was the outlier.
//!
//! What stays in [`access`](crate::access) is the graph vocabulary and the
//! validators: the descriptors the `#[module]` macro submits, the reachability
//! set, and the passes themselves.

use std::borrow::Cow;

use serde_json::error::Category;
use thiserror::Error;

/// A provider depends on something its module does not import and that is not
/// global infrastructure. Raised at boot by the access-graph validation.
#[derive(Debug, Error)]
#[error(
    "module access violation: `{consumer}` (in module `{module}`) depends on `{dependency}`, \
     but `{module}` imports no module that provides it. `{dependency}` is provided by `{owner}` \
     — add `{owner}` to `#[module(imports = [...])]` of `{module}`, or route the dependency \
     through a module `{module}` already imports."
)]
pub struct AccessGraphError {
    /// Module that owns the offending consumer and whose imports fall short.
    pub module: &'static str,
    /// Provider that reached for a dependency its module cannot see.
    pub consumer: &'static str,
    /// The dependency that was out of reach.
    pub dependency: &'static str,
    /// Module that actually provides `dependency` — the one to import to fix it.
    pub owner: &'static str,
}

/// A provider depends on something **no module provides** — not global
/// infrastructure, not in its import closure, not registered anywhere. Raised at
/// boot so a lazily-built scoped/transient provider fails cleanly here instead
/// of panicking at its first `get(...).expect(...)` resolution. An *eager*
/// provider's missing dependency lands here too: the register phase defers it
/// to this check rather than panicking ahead of it, so every wiring failure is
/// one `Result`.
#[derive(Debug, Error)]
#[error(
    "unmet dependency: `{consumer}` (in module `{module}`) depends on `{dependency}`, but no \
     module provides it and it is not global infrastructure (a seed or factory output). Add a \
     provider for `{dependency}` to a module reachable from the root, or seed it at \
     `App::builder()`."
)]
pub struct MissingDependencyError {
    /// Module that owns the consumer whose dependency is unmet.
    pub module: &'static str,
    /// Provider whose dependency no module supplies.
    pub consumer: &'static str,
    /// The dependency that is registered nowhere and is not global infra.
    pub dependency: &'static str,
}

/// A **singleton** provider `#[inject]`s one the container never places in the
/// singleton map — a `#[injectable(scope = request)]` or `scope = transient`
/// provider.
///
/// Raised at boot because the container cannot honour it and, before this error,
/// did not say so: the register phase gates readiness on the singleton map, so
/// such a provider never became ready, was classified unprovided, and was
/// **dropped** — with everything downstream of it — while the boot returned
/// `Ok` and emitted nothing. The symptom surfaced far away, as a service
/// missing at its first `Container::get`, or as an inert-host `warn` offering
/// five causes none of which was this one.
///
/// **The remedy names the concept, never the edge crates**, and that is the
/// same law [`target`](crate::target) and [`operation_log`](crate::operation_log)
/// state for themselves: the kernel holds no name for a concern it does not know
/// exists. It listed three `Scoped<T>` paths for one round — `nest_rs_http`,
/// `nest_rs_graphql`, `nest_rs_mcp` — copied from a prose list that was itself
/// three of four, so a developer who hit this on a WS gateway was handed three
/// paths none of which was theirs while `nest_rs_ws::Scoped<T>` existed.
/// Nothing compiles against a message, so the fourth would never have been
/// added; every future edge would have inherited the same wrong remedy.
///
/// **The reason is worded per arm, because the two arms are not the same fact.**
/// A request-scoped provider genuinely has no instance outside a request. A
/// transient one does — `Container::get` opens a throwaway scope and builds it
/// ([`Discoverable`](crate::Discoverable)'s own table says so). What is true of
/// both, and is what this check reads, is that neither is ever in the singleton
/// map the register phase gates readiness on.
#[derive(Debug, Error)]
#[error(
    "scope violation: `{consumer}` (in module `{module}`) is a singleton and injects \
     `{dependency}`, which is request-scoped or transient. Neither is ever placed in \
     the singleton map a singleton's dependencies are resolved from, so there is \
     nothing for `{consumer}` to hold once at boot. Reach it through the request \
     boundary of the edge that dispatches the work — the `Scoped<T>` its crate \
     exports — or make `{consumer}` request-scoped too."
)]
pub struct ScopeViolationError {
    /// Module that owns the offending consumer.
    pub module: &'static str,
    /// The singleton provider whose `#[inject]` cannot be honoured.
    pub consumer: &'static str,
    /// The request-scoped or transient dependency it named.
    pub dependency: &'static str,
}

/// The failure modes of the bare (non-keyed) access-graph pass: a cross-module
/// reach that no import covers, or a dependency no module provides.
///
/// `pub(crate)`, unlike every other error here, and [`into_anyhow`](Self::into_anyhow) is why: the
/// wrapper is discarded before a boot failure leaves the crate, so no public
/// signature can hand a caller one and nothing could downcast to it.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum AccessError {
    /// A provider reached across modules for something no import covers.
    #[error(transparent)]
    CrossModule(#[from] AccessGraphError),
    /// A provider depends on something no module provides at all.
    #[error(transparent)]
    Missing(#[from] MissingDependencyError),
    /// A singleton injected a provider that only exists inside a request.
    #[error(transparent)]
    Scope(#[from] ScopeViolationError),
}

impl AccessError {
    /// Flatten into an `anyhow::Error` carrying the **concrete** inner error,
    /// discarding the enum wrapper, so a boot failure downcasts to
    /// `AccessGraphError` / `MissingDependencyError` directly — the wrapper is an
    /// internal detail of the pass, not part of the boot-error contract.
    /// `anyhow::Error::new` (over the concrete type) is what preserves the
    /// downcast; boxing to `dyn Error` first would lose it.
    pub(crate) fn into_anyhow(self) -> anyhow::Error {
        match self {
            AccessError::CrossModule(e) => anyhow::Error::new(e),
            AccessError::Missing(e) => anyhow::Error::new(e),
            AccessError::Scope(e) => anyhow::Error::new(e),
        }
    }
}

/// A concrete type or a trait object was registered more than once — two
/// modules, or a seed and a module, providing the same type. Raised at boot
/// rather than silently last-write-wins, uniform with every other wiring
/// error. The test override path is exempt: it is the *intended* replacement.
#[derive(Debug, Error)]
#[error(
    "duplicate provider: `{type_name}` is registered more than once. Two modules (or a seed and a \
     module) provide the same type — remove the redundant registration; a test replaces a \
     provider with `override_value` or `override_dyn`."
)]
pub struct DuplicateProviderError {
    /// The type registered more than once.
    pub type_name: &'static str,
}

/// Two import sites each *declared* a value for the same type, and one of them
/// would have to lose. Raised by `AppBuilder::build` before any factory runs.
///
/// The framework refuses to pick because both call sites are deliberate: the
/// container resolves an ordinary collision by keeping the first factory
/// queued, which would make the surviving value a function of `imports = [..]`
/// order — silently dropped, on the wrong side of *no silent failure*. Only a
/// **declaration** contests (`ContainerBuilder::provide_declared_factory`): a
/// pinned config base, or a module binding an implementation a sibling module
/// also binds — and the trait object a `provide_factory_dyn` binds, which one
/// implementation holds. A module queuing the same default, or the same
/// binding, twice never does, so a diamond import stays legal.
///
/// Both declarations are named — each as the import that made it and the
/// module whose `imports = [..]` lists it — so the reader goes to the two lines
/// to reconcile instead of searching the tree for them. `remedy` comes from the
/// declaring seam, which knows what a declaration of its type means.
#[derive(Debug, Error)]
#[error(
    "contested declaration: `{type_name}` is declared twice — by {first}, and by {second}. {remedy}"
)]
pub struct ContestedDeclarationError {
    /// The type declared more than once.
    pub type_name: &'static str,
    /// The declaration made first, as the import that made it.
    pub first: String,
    /// The declaration that contested it, the same way.
    pub second: String,
    /// What the reader should do instead, supplied by the declaring seam.
    pub remedy: &'static str,
}

/// A module queued an async factory, but the boot went through the synchronous
/// [`App::new`](crate::App::new), which has no factory phase to drain it.
///
/// The value would simply never exist: a `Module::for_root(cfg)` whose config
/// resolves to nothing, a pool nobody opened. Injecting it fails the access
/// graph, but reading it through `Container::get` would just return `None` —
/// so the boot refuses instead of leaving the hole open.
#[derive(Debug, Error)]
#[error(
    "`{type_name}` is provided by an async factory, which the synchronous `App::new` never runs. \
     A module's `for_root(..)` and `ConfigModule::for_feature` both queue one. Boot with \
     `App::builder().module::<M>().build().await` instead."
)]
pub struct UnresolvedFactoryError {
    /// The type whose factory nothing would drain.
    pub type_name: &'static str,
}

/// Every factory left in the queue waits on a factory output still in it — one
/// another's, or its own — so none can run first: a cycle in the `*_after`
/// declarations. Raised by `AppBuilder::build` instead of running them in queue
/// order, which would hand one of them a snapshot missing what it declared it
/// reads.
#[derive(Debug, Error)]
#[error(
    "factory cycle: {type_names:?} — each waits on a factory output that a member \
     of this set (itself included) would provide, so no order satisfies them. \
     Drop one `_after` declaration."
)]
pub struct FactoryCycleError {
    /// The types whose factories wait on each other.
    pub type_names: Vec<&'static str>,
}

/// A module queued an async factory during the register phase, once the
/// collect phase every factory is queued in had ended: no boot drains it, so
/// the value would never exist. Raised by both boot paths as the register
/// phase ends; a default whose output is already present is discarded, as the
/// factory phase discards it, while a declaration is refused all the same — the
/// value present is not the one it chose.
///
/// The usual cause is a [`DynamicModule`](crate::DynamicModule) whose `collect`
/// leaves out its module's own [`Module::collect`](crate::Module::collect): the
/// imports that module lists then collect in the register phase, too late.
#[derive(Debug, Error)]
#[error(
    "`{type_name}` is provided by an async factory queued during the register phase, which no \
     boot drains. Queue it in `collect` — a `DynamicModule` whose `collect` leaves out its \
     module's own `Module::collect` makes that module's imports queue theirs in `register`."
)]
pub struct LateFactoryError {
    /// The type whose factory was queued too late to run.
    pub type_name: &'static str,
}

/// A `#[module]`'s dynamic import reached the register phase with no value its
/// collect phase built: something marked the module collected without running
/// its `Module::collect`, which builds each dynamic import. Raised by the
/// register phase rather than leaving the import out.
#[derive(Debug, Error)]
#[error(
    "{site} reached the register phase uncollected: its module was marked collected without \
     running its `Module::collect`, which builds every dynamic import — call that `collect` \
     instead of `ContainerBuilder::mark_collected` for a module `#[module]` expands"
)]
pub(crate) struct UncollectedImportError {
    /// The import, as a boot error names its site.
    pub(crate) site: String,
}

/// Providers of one module each wait on another of them, injected by its
/// concrete type, so none can be built first. Raised by the register phase,
/// since the access graph sees every dependency met.
#[derive(Debug, Error)]
#[error(
    "module `{module}`: dependency cycle among provider(s) {type_names:?} — each waits on another \
     provider in the same module; break it by injecting `Arc<dyn Trait>` instead of the concrete \
     type"
)]
pub struct ProviderCycleError {
    /// The module whose providers wait on each other.
    pub module: &'static str,
    /// The providers on the cycle.
    pub type_names: Vec<&'static str>,
}

/// A resource's [`Budget`](crate::Budget) at or past the [`Net`](crate::Net) of
/// a port that waits on it. Raised by the boot's last pass, once every
/// provider is built and each budget can be read off its own.
///
/// The port would stop waiting first: a call the resource was still answering
/// would be cut, and its cause — which endpoint, which bound — replaced by the
/// port's bare timeout. It would fail that way on every outage, so the boot
/// refuses it rather than let the first incident discover it.
#[derive(Debug, Error)]
#[error(
    "{resource}'s budget ({budget:?}) must be shorter than {port}'s net ({net:?}), which would \
     otherwise give up on a call still answering and lose its cause: lower {setting}"
)]
pub struct BudgetPastNetError {
    /// The resource, as a sentence names it.
    pub resource: &'static str,
    /// What the resource waits, at most, for one answer.
    pub budget: std::time::Duration,
    /// The port whose net reaches the resource, as a sentence names it.
    pub port: &'static str,
    /// What the port waits before it gives up.
    pub net: std::time::Duration,
    /// What lowers the budget: the variable, and the field that pins it in code.
    pub setting: String,
}

/// A provider's `#[inject(key = "…")]` keyed dependency has no keyed provider
/// registered as global infrastructure (a seed or a factory output). Raised at
/// boot by the keyed pass of the access-graph validation. Unlike a bare
/// dependency — deferred to the register-phase fixpoint when genuinely missing —
/// a keyed dependency is validated here so the failure is a clean boot error
/// naming **both** the type and the key, not a `get_keyed(...).expect(...)`
/// panic during construction.
#[derive(Debug, Error)]
#[error(
    "keyed dependency unreachable: `{consumer}` (in module `{module}`) injects `{type_name}` \
     keyed `{key}`, but no keyed provider for that (type, key) is registered. Register it as \
     global infrastructure — `App::builder().provide_keyed::<{type_name}>(\"{key}\", …)` or a \
     `ContainerBuilder::provide_keyed`/factory in a module reachable from the root."
)]
pub struct KeyedDependencyError {
    /// Module that owns the consumer with the unreachable keyed dependency.
    pub module: &'static str,
    /// Provider whose `#[inject(key = "…")]` has no keyed provider registered.
    pub consumer: &'static str,
    /// The injected type of the keyed dependency.
    pub type_name: &'static str,
    /// The requested key — named alongside the type so both appear in the error.
    pub key: &'static str,
}

/// A payload that did not decode, as every edge reports it: the category of the
/// failure, the line and column when the payload was text, the kind of value
/// found and the type expected — never the value.
///
/// serde's own sentences quote what the payload held: ``invalid type: string
/// "sk_live_…", expected u64``, ``unknown variant `4242…`, expected `Visa` ``,
/// ``unknown field `sk_live_…`, expected `amount` ``. A payload is somebody's data,
/// and every edge that reports a failure to decode one puts that sentence where it
/// is read by more people, and kept longer, than the payload ever was — a log
/// line, a dead-letter record, a reply. So the framework reports it the way
/// `Valid` and `Header<T>` already do.
///
/// **Fail-secure by construction.** The sentence is rebuilt from the shapes serde
/// words; any other — a type's own `custom` message, which may quote anything — is
/// reported by its category alone. What a rebuilt sentence keeps is the type's
/// own: its expected description, a field it requires, a count. A key or a
/// variant the payload spelled is the payload's, and is dropped like a value. What
/// is kept is bounded ([`MAX_LEN`](Self::MAX_LEN)), since a visitor's `expecting`
/// may describe itself at any length.
///
/// Built from the error a decode returned — serde_json's, or serde's own
/// [`value::Error`](serde::de::value::Error), which `serde_urlencoded` and every
/// `IntoDeserializer` reader return: `DecodeError::from(&error)`. Carried where
/// that error would have been: as the `source` of the edge's own error, or
/// displayed in its sentence. A text already rendered *from* an error — a reply's
/// detail, a wrapper's sentence — is said without the values it quotes by
/// [`redact`](Self::redact).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{sentence}")]
pub struct DecodeError {
    sentence: String,
}

impl DecodeError {
    /// The longest a report runs before its position, in bytes. What a rebuilt
    /// sentence keeps is the type's — an `expected` description, a field name —
    /// and a hand-written visitor's `expecting` may describe itself at any length;
    /// past this the sentence is cut at a character boundary and ends in `…`.
    pub const MAX_LEN: usize = 512;

    /// The report of `error`, without the value it quoted.
    pub fn new(error: &serde_json::Error) -> Self {
        let rendered = error.to_string();
        // serde_json appends the position to every message read from text, and
        // to none read from a value.
        let position = (error.line() > 0)
            .then(|| format!(" at line {} column {}", error.line(), error.column()));
        let message = position
            .as_deref()
            .and_then(|position| rendered.strip_suffix(position))
            .unwrap_or(&rendered);
        let sentence = match error.classify() {
            Category::Data => data(message),
            // serde_json's own wording: one fixed sentence per fault of the text
            // or the reader, naming no byte of the input.
            Category::Syntax | Category::Eof | Category::Io => message.to_owned(),
        };
        let mut sentence = bounded(&sentence).into_owned();
        if let Some(position) = position {
            sentence.push_str(&position);
        }
        Self { sentence }
    }

    /// `link` as the report it is, when it is a decode failure — serde_json's
    /// error, or serde's own value error.
    pub(crate) fn of(link: &(dyn std::error::Error + 'static)) -> Option<Self> {
        if let Some(error) = link.downcast_ref::<serde_json::Error>() {
            return Some(Self::new(error));
        }
        link.downcast_ref::<serde::de::value::Error>()
            .map(Self::from)
    }

    /// `text` said without the values a decode failure quotes in it — a reply's
    /// detail, a frame, any sentence rendered from an error before the edge that
    /// reports it held it.
    ///
    /// Two readings, both applied. Each decode failure in `error`'s chain is said
    /// as its report wherever `text` spells it — the exact reading, and the only
    /// one that reaches a type's `custom` message. Then any sentence still in one
    /// of serde's quoting shapes (`invalid type:`, `invalid value:`, ``unknown
    /// variant ` ``, ``unknown field ` ``) is rebuilt without its value: those are
    /// `serde::de::Error`'s own defaults, worded alike by every format, and they
    /// survive a wrapper that hides its cause from `source()` — thiserror's
    /// `#[error(transparent)]`, anyhow's own box, a library that kept only the
    /// text. From the first such sentence to the end of `text` is the failure's,
    /// since nothing marks where it ends, and is bounded like a report.
    ///
    /// A `validator` failure is a refusal of input too, and its own wording lists
    /// the rejected input among a rule's parameters: each
    /// `Validation error: <code> [<params>]` in `text` is said as
    /// `Validation error: <code>`, whatever wrapper spelled it.
    ///
    /// Borrowed when nothing in `text` was a decode or validation failure's.
    pub fn redact<'t>(
        text: &'t str,
        error: Option<&(dyn std::error::Error + 'static)>,
    ) -> Cow<'t, str> {
        DecodeFailures::of(error).redact(text)
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(&error)
    }
}

impl From<&serde_json::Error> for DecodeError {
    fn from(error: &serde_json::Error) -> Self {
        Self::new(error)
    }
}

/// serde's own error for a value read through `IntoDeserializer` — the one
/// `serde_urlencoded` returns for a query string or a form. Every message it
/// carries is a data error, with no position: the reader has no text to point
/// into.
impl From<&serde::de::value::Error> for DecodeError {
    fn from(error: &serde::de::value::Error) -> Self {
        Self {
            sentence: bounded(&data(&error.to_string())).into_owned(),
        }
    }
}

impl From<serde::de::value::Error> for DecodeError {
    fn from(error: serde::de::value::Error) -> Self {
        Self::from(&error)
    }
}

/// The decode failures an error's chain holds, each as the text it displays and
/// the report it is said as — gathered once, so every text rendered from the
/// chain is redacted against the same pairs.
pub(crate) struct DecodeFailures(Vec<(String, String)>);

impl DecodeFailures {
    /// Every link of `error`'s chain that is a decode failure saying something
    /// its report does not.
    pub(crate) fn of(error: Option<&(dyn std::error::Error + 'static)>) -> Self {
        let mut pairs = Vec::new();
        let mut link = error;
        while let Some(current) = link {
            if let Some(report) = DecodeError::of(current) {
                let displayed = current.to_string();
                if !displayed.is_empty() && displayed != report.sentence {
                    pairs.push((displayed, report.sentence));
                }
            }
            link = current.source();
        }
        Self(pairs)
    }

    /// `text`, with each failure it spells said as its report, and any sentence
    /// left in serde's quoting shapes rebuilt without its value.
    pub(crate) fn redact<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let mut text = Cow::Borrowed(text);
        for (displayed, report) in &self.0 {
            if text.contains(displayed.as_str()) {
                text = Cow::Owned(text.replace(displayed.as_str(), report));
            }
        }
        // validator's wording first: a value it lists may spell what serde's
        // shapes look for, and serde's reading keeps what follows them.
        let validated = match validation_sentences(&text) {
            Cow::Owned(validated) => Some(validated),
            Cow::Borrowed(_) => None,
        };
        let text = validated.map_or(text, Cow::Owned);
        let rebuilt = match quoting_sentences(&text) {
            Cow::Owned(rebuilt) => Some(rebuilt),
            Cow::Borrowed(_) => None,
        };
        rebuilt.map_or(text, Cow::Owned)
    }
}

/// `text` within [`DecodeError::MAX_LEN`] bytes, cut at a character boundary,
/// and whether it was cut.
fn within_bound(text: &str) -> (&str, bool) {
    if text.len() <= DecodeError::MAX_LEN {
        return (text, false);
    }
    let mut end = DecodeError::MAX_LEN;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// `text` within [`DecodeError::MAX_LEN`] bytes, ending in `…` when it was cut.
fn bounded(text: &str) -> Cow<'_, str> {
    match within_bound(text) {
        (kept, true) => Cow::Owned(format!("{kept}…")),
        (kept, false) => Cow::Borrowed(kept),
    }
}

/// What a data error — a value that is valid JSON and not the type — is reported
/// as, rebuilt from the shapes serde words.
fn data(message: &str) -> String {
    if let Some(rebuilt) = quoting(message) {
        return rebuilt;
    }
    // Sentences naming a count, or a field of the type's own, and nothing the
    // payload holds as a value. `missing field` and `duplicate field` name a
    // field the derive spelled (`&'static str`); `unknown field` names the
    // payload's key, and is a quoting shape above.
    const KEPT: [&str; 5] = [
        "invalid length ",
        "missing field `",
        "duplicate field `",
        "data did not match any variant of untagged enum ",
        "no variant of enum ",
    ];
    if KEPT.iter().any(|kept| message.starts_with(kept)) {
        return message.to_owned();
    }
    "a value its type does not accept".to_owned()
}

/// A sentence in one of serde's quoting shapes, rebuilt without what the payload
/// spelled; `None` for any other.
fn quoting(sentence: &str) -> Option<String> {
    if let Some(rest) = sentence.strip_prefix("invalid type: ") {
        return Some(found("invalid type", rest));
    }
    if let Some(rest) = sentence.strip_prefix("invalid value: ") {
        return Some(found("invalid value", rest));
    }
    // The variant and the key are spelled by whoever wrote the payload; the list
    // the type expected is the type's.
    if let Some(rest) = sentence.strip_prefix("unknown variant ") {
        return Some(unknown("unknown variant", rest, ", there are no variants"));
    }
    if let Some(rest) = sentence.strip_prefix("unknown field ") {
        return Some(unknown("unknown field", rest, ", there are no fields"));
    }
    None
}

/// The kinds serde's `Unexpected` quotes a value after, and what each is said as
/// — a string's opening quote also as `Debug` escapes it, which is how a wrapper
/// formatting its cause with `{:?}` spells it.
const QUOTING_KINDS: [(&str, &str); 6] = [
    ("boolean `", "a boolean"),
    ("integer `", "an integer"),
    ("floating point `", "a floating point number"),
    ("character `", "a character"),
    ("string \"", "a string"),
    ("string \\\"", "a string"),
];

/// Where one of serde's quoting sentences opens in `text`, read by serde's own
/// wording: `invalid type:` or `invalid value:` then a kind that quotes a value,
/// or `` unknown variant ` `` / `` unknown field ` ``, each closed later by the
/// tail serde always writes. Anything else — `invalid value: must be positive`,
/// a sentence this module already rebuilt — is somebody's own words, and read as
/// they are.
fn quoting_opens_at(text: &str) -> Option<usize> {
    const FOUND: [&str; 2] = ["invalid type: ", "invalid value: "];
    const UNKNOWN: [&str; 2] = ["unknown variant `", "unknown field `"];
    // Where the last tail opens, found once: an opening is closed when a tail
    // follows it, which is when the last one does. Asking each opening whether
    // its rest held one was quadratic in a text a client can fill with openings.
    let expected = text.rfind(", expected ");
    let closed = expected.max(text.rfind(", there are no "));
    let found = FOUND.iter().filter_map(|open| {
        text.match_indices(open)
            .map(|(at, _)| at)
            .take_while(|at| expected.is_some_and(|tail| tail >= at + open.len()))
            .find(|at| {
                let rest = &text[at + open.len()..];
                QUOTING_KINDS.iter().any(|(head, _)| rest.starts_with(head))
            })
    });
    let unknown = UNKNOWN.iter().filter_map(|open| {
        text.find(open)
            .filter(|at| closed.is_some_and(|tail| tail >= *at))
    });
    found.chain(unknown).min()
}

/// `text` with each of serde's quoting sentences in it rebuilt, from the first
/// one to the end of `text` — nothing marks where a failure ends in free text,
/// so all that follows it is the failure's. Bounded before it is read, so the
/// walk over the `expected` tails it recurses into is bounded too.
fn quoting_sentences(text: &str) -> Cow<'_, str> {
    let Some(at) = quoting_opens_at(text) else {
        return Cow::Borrowed(text);
    };
    let (failure, cut) = within_bound(&text[at..]);
    // Every opening `quoting_opens_at` finds is one `quoting` rebuilds; the
    // fallback is the report for a shape it does not know, which says nothing
    // the payload held.
    let rebuilt = quoting(failure).unwrap_or_else(|| "a value its type does not accept".to_owned());
    let mut redacted = String::with_capacity(at + rebuilt.len() + '…'.len_utf8());
    redacted.push_str(&text[..at]);
    redacted.push_str(&bounded(&rebuilt));
    if cut && !redacted.ends_with('…') {
        redacted.push('…');
    }
    Cow::Owned(redacted)
}

/// What opens `validator`'s wording of a failing rule: `Validation error: <code>
/// [<params>]`, its `Display` for a rule given no message of its own.
const VALIDATION_OPENING: &str = "Validation error: ";

/// `text` with each `validator` failure in it said without its parameters.
///
/// The parameters hold the rejected input (`value`, `must_match`'s `other`, a
/// bound read from another field), so they are dropped up to their closing
/// bracket, read past every quoted string so a value cannot close them early —
/// or to the end of `text`, marked `…`, when they never close. The code is the
/// rule's own and is kept. A rule given its own `message` displays that message
/// alone, which is its author's. Each stretch of `text` is read a bounded number
/// of times, however many openings a client packs into it.
fn validation_sentences(text: &str) -> Cow<'_, str> {
    if !text.contains(VALIDATION_OPENING) {
        return Cow::Borrowed(text);
    }
    let mut said = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(VALIDATION_OPENING) {
        let after = &rest[at + VALIDATION_OPENING.len()..];
        // The code ends at ` [`, before the next opening and on the same line.
        let next = after.find(VALIDATION_OPENING).unwrap_or(after.len());
        let line = after[..next].split('\n').next().unwrap_or_default();
        let Some(open) = line.find(" [") else {
            said.push_str(&rest[..at + VALIDATION_OPENING.len()]);
            rest = after;
            continue;
        };
        said.push_str(&rest[..at + VALIDATION_OPENING.len() + open]);
        match past_closing_bracket(&after[open + 2..]) {
            Some(tail) => rest = tail,
            None => {
                said.push('…');
                rest = "";
            }
        }
    }
    said.push_str(rest);
    Cow::Owned(said)
}

/// What follows the `]` closing a bracket already open, read past quoted
/// strings and their escapes; `None` when it never closes.
fn past_closing_bracket(text: &str) -> Option<&str> {
    let mut depth = 1_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[at + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

/// `invalid type` or `invalid value`, said with the kind of value found and the
/// type expected.
fn found(what: &str, rest: &str) -> String {
    let kind = kind(rest);
    match expected(rest) {
        Some(expected) => format!("{what}: {kind}, {}", quoting_sentences(expected)),
        None => format!("{what}: {kind}"),
    }
}

/// `unknown variant` or `unknown field`, said with what the type expected and
/// without what the payload spelled.
fn unknown(what: &str, rest: &str, none_expected: &str) -> String {
    match expected(rest) {
        Some(expected) => format!("{what}, {}", quoting_sentences(expected)),
        None if rest.ends_with(none_expected) => format!("{what}{none_expected}"),
        None => what.to_owned(),
    }
}

/// The `expected …` tail serde closes a refusal with. The last one: a value
/// spelled before it may itself contain the words, the type's description after
/// it does not.
fn expected(rest: &str) -> Option<&str> {
    rest.rfind(", expected ").map(|at| &rest[at + 2..])
}

/// The kind of value serde's `Unexpected` names at the head of `rest`, without
/// the value it quotes after the kind.
fn kind(rest: &str) -> &'static str {
    // The kinds that quote nothing.
    const BARE: [(&str, &str); 12] = [
        ("unit value", "null"),
        ("byte array", "a byte array"),
        ("Option value", "an optional value"),
        ("newtype struct", "a newtype struct"),
        ("sequence", "a sequence"),
        ("map", "a map"),
        ("enum", "an enum"),
        ("unit variant", "a unit variant"),
        ("newtype variant", "a newtype variant"),
        ("tuple variant", "a tuple variant"),
        ("struct variant", "a struct variant"),
        ("null", "null"),
    ];
    if let Some((_, kind)) = QUOTING_KINDS
        .iter()
        .find(|(head, _)| rest.starts_with(head))
    {
        return kind;
    }
    BARE.iter()
        .find(|(head, _)| {
            rest.strip_prefix(head)
                .is_some_and(|tail| tail.is_empty() || tail.starts_with(", expected "))
        })
        .map_or("a value", |(_, kind)| kind)
}

#[cfg(test)]
mod decode_error_tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize)]
    enum Card {
        Visa,
    }

    #[derive(Debug, Deserialize)]
    #[expect(
        dead_code,
        reason = "the fields exist for serde to read; the test asserts the error, never a value"
    )]
    struct Charge {
        amount: u64,
        card: Card,
    }

    fn from_value(value: serde_json::Value) -> String {
        let error = serde_json::from_value::<Charge>(value).expect_err("does not decode");
        DecodeError::new(&error).to_string()
    }

    fn from_str(text: &str) -> String {
        let error = serde_json::from_str::<Charge>(text).expect_err("does not decode");
        DecodeError::new(&error).to_string()
    }

    /// The audit's two probes: a secret where a number was expected, a card
    /// number where a variant was. serde quotes both; the report quotes neither.
    #[test]
    fn a_refused_value_is_said_by_its_kind_and_never_quoted() {
        let secret = from_value(json!({ "amount": "sk_live_51HsecretTOKEN", "card": "Visa" }));
        assert_eq!(secret, "invalid type: a string, expected u64");
        let card = from_value(json!({ "amount": 1, "card": "4242424242424242" }));
        assert_eq!(card, "unknown variant, expected `Visa`");
        for report in [secret, card] {
            assert!(
                !report.contains("sk_live") && !report.contains("4242"),
                "{report}"
            );
        }
    }

    /// Read from text, the report keeps the position, which is *where*.
    #[test]
    fn a_payload_read_from_text_keeps_its_line_and_column() {
        assert_eq!(
            from_str(r#"{"amount": true, "card": "Visa"}"#),
            "invalid type: a boolean, expected u64 at line 1 column 15"
        );
        assert_eq!(
            from_str(r#"{"amount": 1, "card": "Visa""#),
            "EOF while parsing an object at line 1 column 28",
            "a fault of the text is serde_json's own sentence, which quotes nothing",
        );
    }

    /// A value spelling serde's own separator cannot move the cut into the type's
    /// half of the sentence.
    #[test]
    fn a_value_spelling_the_separator_is_still_not_quoted() {
        let report = from_value(json!({ "amount": "x, expected u64, leaked", "card": "Visa" }));
        assert_eq!(report, "invalid type: a string, expected u64");
    }

    /// Every kind serde_json can find where a number was expected, and the
    /// sentences that name only the type's own fields or a count, kept whole.
    #[test]
    fn kinds_and_the_type_s_own_sentences_are_kept() {
        assert_eq!(
            from_value(json!({ "amount": null, "card": "Visa" })),
            "invalid type: null, expected u64"
        );
        assert_eq!(
            from_value(json!({ "amount": [1], "card": "Visa" })),
            "invalid type: a sequence, expected u64"
        );
        assert_eq!(
            from_value(json!({ "amount": -5, "card": "Visa" })),
            "invalid value: an integer, expected u64"
        );
        assert_eq!(
            from_value(json!({ "amount": 1.5, "card": "Visa" })),
            "invalid type: a floating point number, expected u64"
        );
        assert_eq!(
            from_value(json!({ "card": "Visa" })),
            "missing field `amount`"
        );
    }

    /// `unknown field` names the key the *payload* spelled, so it is dropped like
    /// a variant: what is kept is the list the type expected.
    #[test]
    fn an_unknown_key_is_the_payload_s_and_is_dropped() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Strict {
            amount: u64,
        }
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(
            dead_code,
            reason = "the fields exist for serde to read; the test asserts the error, never a value"
        )]
        struct Pair {
            amount: u64,
            currency: String,
        }
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Empty {}

        let text = serde_json::from_str::<Strict>(r#"{"amount":1,"sk_live_KEY":1}"#)
            .expect_err("an unknown key");
        assert_eq!(
            DecodeError::new(&text).to_string(),
            "unknown field, expected `amount` at line 1 column 25"
        );
        let value = serde_json::from_value::<Strict>(json!({ "amount": 1, "sk_live_KEY": 1 }))
            .expect_err("an unknown key");
        assert_eq!(
            DecodeError::new(&value).to_string(),
            "unknown field, expected `amount`"
        );
        let pair = serde_json::from_value::<Pair>(json!({ "sk_live_KEY": 1 }))
            .expect_err("an unknown key");
        assert_eq!(
            DecodeError::new(&pair).to_string(),
            "unknown field, expected `amount` or `currency`"
        );
        let empty =
            serde_json::from_value::<Empty>(json!({ "sk_live_KEY": 1 })).expect_err("no fields");
        assert_eq!(
            DecodeError::new(&empty).to_string(),
            "unknown field, there are no fields"
        );
    }

    /// What a report keeps is the type's, and a hand-written visitor describes
    /// itself at whatever length it likes: the report stops at the bound, at a
    /// character boundary, and still carries its position.
    #[test]
    fn what_a_report_keeps_is_bounded() {
        #[derive(Debug)]
        struct Verbose;
        impl<'de> Deserialize<'de> for Verbose {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct Visitor;
                impl serde::de::Visitor<'_> for Visitor {
                    type Value = Verbose;
                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(&"é".repeat(2 * DecodeError::MAX_LEN))
                    }
                }
                d.deserialize_u64(Visitor)
            }
        }
        let error = serde_json::from_str::<Verbose>("true").expect_err("not a number");
        let report = DecodeError::new(&error).to_string();
        let (sentence, position) = report
            .rsplit_once('…')
            .expect("a report past the bound says it was cut");
        assert!(sentence.len() <= DecodeError::MAX_LEN, "{}", sentence.len());
        assert!(sentence.starts_with("invalid type: a boolean, expected é"));
        assert_eq!(position, " at line 1 column 4");
    }

    /// serde's own value error — `serde_urlencoded`'s, for a query string or a
    /// form — carries the same sentences, with no position, and is reported the
    /// same way.
    #[test]
    fn serde_s_value_error_is_reported_like_serde_json_s() {
        use serde::de::IntoDeserializer;
        use serde::de::value::Error as ValueError;

        let quoted = u64::deserialize(IntoDeserializer::<ValueError>::into_deserializer(
            "sk_live_51HsecretTOKEN",
        ))
        .expect_err("not a number");
        assert!(
            quoted.to_string().contains("sk_live"),
            "serde quotes it: {quoted}"
        );
        assert_eq!(
            DecodeError::from(&quoted).to_string(),
            "invalid type: a string, expected u64"
        );
        let custom = <ValueError as serde::de::Error>::custom("token ya29.secret refused");
        assert_eq!(
            DecodeError::from(custom).to_string(),
            "a value its type does not accept"
        );
    }

    /// A text rendered from an error before the edge held it — a reply, a
    /// wrapper's sentence — is said without the values it quotes, by both
    /// readings: the chain's failures exactly, and serde's quoting shapes
    /// wherever they appear.
    #[test]
    fn a_rendered_text_is_redacted_by_the_chain_and_by_serde_s_wording() {
        let secret =
            serde_json::from_str::<u64>(r#""sk_live_51HsecretTOKEN""#).expect_err("not a number");
        let detail = format!("parse error: {secret}");
        assert_eq!(
            DecodeError::redact(&detail, Some(&secret)),
            "parse error: invalid type: a string, expected u64 at line 1 column 24"
        );
        assert_eq!(
            DecodeError::redact(&detail, None),
            "parse error: invalid type: a string, expected u64 at line 1 column 24",
            "serde's wording is read without the chain too",
        );
        assert!(
            matches!(
                DecodeError::redact("not found", Some(&secret)),
                Cow::Borrowed(_)
            ),
            "a text quoting nothing is handed back as it was",
        );

        // A type's `custom` message has no shape to read; only the chain knows it.
        let custom = <serde_json::Error as serde::de::Error>::custom("bad token sk_live_51H");
        let detail = format!("could not read: {custom}");
        assert_eq!(
            DecodeError::redact(&detail, Some(&custom)),
            "could not read: a value its type does not accept"
        );

        // The quoted value may spell anything, the separator and a second
        // sentence included; every shape after the first is rebuilt as well.
        let text = r#"x: invalid type: string "a, expected u64, b", expected u64; unknown field `sk_live`, there are no fields"#;
        let redacted = DecodeError::redact(text, None);
        assert!(
            !redacted.contains("sk_live") && !redacted.contains("\"a"),
            "{redacted}"
        );
        assert_eq!(
            redacted,
            "x: invalid type: a string, expected u64; unknown field, there are no fields"
        );

        // Somebody's own words in serde's vocabulary are not serde's sentence.
        for own in [
            "invalid value: must be positive",
            "invalid type: expected an admin token",
            "unknown field `limit` in the request",
        ] {
            assert_eq!(DecodeError::redact(own, None), own);
        }

        // A report read again reads the same, so the two readings compose.
        for report in [
            "invalid type: a string, expected u64 at line 1 column 24",
            "invalid value: an integer, expected u64",
            "unknown variant, expected `Visa`",
            "unknown field, there are no fields",
        ] {
            assert_eq!(DecodeError::redact(report, None), report);
        }

        // A client can fill a text with openings; reading them is linear.
        let crowded = format!(
            "{}, expected u64",
            "invalid type: string \"".repeat(200_000)
        );
        let started = std::time::Instant::now();
        let redacted = DecodeError::redact(&crowded, None);
        assert!(
            redacted.len() < 2 * DecodeError::MAX_LEN,
            "{}",
            redacted.len()
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?} for {} bytes",
            started.elapsed(),
            crowded.len(),
        );

        // Nothing marks where a failure ends in free text, so what follows the
        // first one is the failure's, and bounded like a report.
        let long = format!(
            "x: invalid type: string \"{}\", expected u64",
            "s".repeat(10_000)
        );
        let redacted = DecodeError::redact(&long, None);
        assert!(
            redacted.len() < 2 * DecodeError::MAX_LEN,
            "{}",
            redacted.len()
        );
        assert!(!redacted.contains("sss"), "{redacted}");
    }

    /// A type's own `custom` message may quote anything, so it is reported by
    /// its category alone.
    #[test]
    fn a_message_of_no_known_shape_is_reported_without_its_text() {
        let error = <serde_json::Error as serde::de::Error>::custom("token ya29.secret refused");
        assert_eq!(
            DecodeError::new(&error).to_string(),
            "a value its type does not accept"
        );
    }

    #[test]
    fn a_validator_failure_keeps_its_rule_and_drops_its_parameters() {
        let text = "signup refused: password: Validation error: length \
                    [{\"min\": Number(32), \"value\": String(\"sk_live_secret\")}]\n\
                    email: Validation error: email [{\"value\": String(\"sk_live_secret\")}]";
        assert_eq!(
            DecodeError::redact(text, None),
            "signup refused: password: Validation error: length\nemail: Validation error: email"
        );
    }

    /// A value spelling the closing bracket, or a quote, cannot end the
    /// parameters early; parameters that never close run to the end of the text.
    #[test]
    fn a_validator_parameter_cannot_close_itself_early() {
        let spelled =
            r#"x: Validation error: must_match [{"other": String("a]\"b] sk_live_secret")}] tail"#;
        assert_eq!(
            DecodeError::redact(spelled, None),
            "x: Validation error: must_match tail"
        );
        let cut = r#"x: Validation error: length [{"value": String("sk_live_sec"#;
        assert_eq!(
            DecodeError::redact(cut, None),
            "x: Validation error: length…"
        );
    }

    /// serde's reading keeps the text after the last `, expected `, which a value
    /// validator lists can spell; validator's wording is read first.
    #[test]
    fn a_validator_value_cannot_hide_behind_a_serde_sentence() {
        let text = r#"body: invalid type: string "x", expected u64; pw: Validation error: length [{"value": String("zz, expected sk_live_secret")}]"#;
        let redacted = DecodeError::redact(text, None);
        assert!(!redacted.contains("sk_live"), "{redacted}");
    }

    /// Openings that never open parameters are left as they are.
    #[test]
    fn openings_without_parameters_are_left_as_they_are() {
        let crowded = "Validation error: x ".repeat(10_000);
        assert_eq!(DecodeError::redact(&crowded, None), crowded);
    }

    #[test]
    fn a_validator_rule_with_its_own_message_is_left_to_its_author() {
        let text = "password: must be at least 32 characters";
        assert!(matches!(DecodeError::redact(text, None), Cow::Borrowed(_)));
    }
}
