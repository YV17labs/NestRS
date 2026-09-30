//! Every error the boot can fail with, and [`DecodeError`] — the one error the
//! kernel lends every edge rather than raises.
//!
//! They are here rather than beside the pass that raises them because four of
//! the nine are not the access graph's at all — `DuplicateProviderError`,
//! `ContestedDeclarationError`, `UnresolvedFactoryError` and `FactoryCycleError`
//! are constructed only in [`app`](crate::app), by the registration and factory
//! phases. Filed under `access.rs` the file's name was a claim about all nine
//! and false for four: from the type a reader derived the wrong file, and from
//! the file they were offered errors its own pass never raises.
//!
//! This is the role table's own row — *domain error → `error.rs`* — and eleven
//! `nest-rs-*` crates already carry one; the kernel was the outlier.
//!
//! What stays in [`access`](crate::access) is the graph vocabulary and the
//! validators: the descriptors the `#[module]` macro submits, the reachability
//! set, and the passes themselves.

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
/// `nest_rs_graphql`, `nest_rs_mcp` — copied from a prose list in
/// `framework.md` that was itself three of four, so a developer who hit this on
/// a WS gateway was handed three paths none of which was theirs while
/// `nest_rs_ws::Scoped<T>` existed. Nothing compiles against a message, so the
/// fourth would never have been added; every future edge would have inherited
/// the same wrong remedy.
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

/// A concrete or keyed provider was registered more than once — two modules,
/// or a seed and a module, providing the same type. Raised at boot rather than
/// silently last-write-wins, uniform with every other wiring error.
/// Trait-object bindings (`provide_dyn`) and the test override path are exempt
/// (they are the *intended* replacement mechanisms).
#[derive(Debug, Error)]
#[error(
    "duplicate provider: `{type_name}` is registered more than once. Two modules (or a seed and a \
     module) provide the same type — remove the redundant registration, or expose it as a \
     `dyn Trait` binding if a deliberate override was intended."
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
/// also binds. A module queuing the same default twice never does, so a diamond
/// import stays legal.
///
/// `remedy` comes from the declaring call site, which is the only place that
/// knows what the two sites were.
#[derive(Debug, Error)]
#[error("contested declaration: `{type_name}` is declared by two import sites. {remedy}")]
pub struct ContestedDeclarationError {
    /// The type declared more than once.
    pub type_name: &'static str,
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
/// serde's own sentences quote the value they refused: ``invalid type: string
/// "sk_live_…", expected u64``, ``unknown variant `4242…`, expected `Visa` ``. A
/// payload is somebody's data, and every edge that reports a failure to decode
/// one puts that sentence where it is read by more people, and kept longer, than
/// the payload ever was — a log line, a dead-letter record, an error frame. So
/// the framework reports it the way `Valid` and `Header<T>` already do.
///
/// **Fail-secure by construction.** The sentence is rebuilt from the shapes serde
/// and serde_json are known to word; any other — a type's own `custom` message,
/// which may quote anything — is reported by its category alone.
///
/// Built from the [`serde_json::Error`] a decode returned
/// (`DecodeError::new(&error)`, or `.map_err(DecodeError::from)`), and carried
/// where that error would have been: as the `source` of the edge's own error, or
/// displayed in its sentence.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{sentence}")]
pub struct DecodeError {
    sentence: String,
}

impl DecodeError {
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
        let mut sentence = match error.classify() {
            Category::Data => data(message),
            // serde_json's own wording: one fixed sentence per fault of the text
            // or the reader, naming no byte of the input.
            Category::Syntax | Category::Eof | Category::Io => message.to_owned(),
        };
        if let Some(position) = position {
            sentence.push_str(&position);
        }
        Self { sentence }
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

/// What a data error — a value that is valid JSON and not the type — is reported
/// as, rebuilt from the shapes serde words.
fn data(message: &str) -> String {
    if let Some(rest) = message.strip_prefix("invalid type: ") {
        return found("invalid type", rest);
    }
    if let Some(rest) = message.strip_prefix("invalid value: ") {
        return found("invalid value", rest);
    }
    // The variant is a value, spelled by whoever wrote the payload; the list of
    // variants is the type's.
    if let Some(rest) = message.strip_prefix("unknown variant ") {
        return match expected(rest) {
            Some(expected) => format!("unknown variant, {expected}"),
            None if rest.ends_with(", there are no variants") => {
                "unknown variant, there are no variants".to_owned()
            }
            None => "unknown variant".to_owned(),
        };
    }
    // Sentences naming a count, or a field of the type's own, and nothing the
    // payload holds as a value.
    const KEPT: [&str; 6] = [
        "invalid length ",
        "missing field `",
        "duplicate field `",
        "unknown field `",
        "data did not match any variant of untagged enum ",
        "no variant of enum ",
    ];
    if KEPT.iter().any(|kept| message.starts_with(kept)) {
        return message.to_owned();
    }
    "a value its type does not accept".to_owned()
}

/// `invalid type` or `invalid value`, said with the kind of value found and the
/// type expected.
fn found(what: &str, rest: &str) -> String {
    let kind = kind(rest);
    match expected(rest) {
        Some(expected) => format!("{what}: {kind}, {expected}"),
        None => format!("{what}: {kind}"),
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
    const QUOTING: [(&str, &str); 5] = [
        ("boolean `", "a boolean"),
        ("integer `", "an integer"),
        ("floating point `", "a floating point number"),
        ("character `", "a character"),
        ("string \"", "a string"),
    ];
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
    if let Some((_, kind)) = QUOTING.iter().find(|(head, _)| rest.starts_with(head)) {
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
    #[allow(dead_code)]
    enum Card {
        Visa,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
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
}
