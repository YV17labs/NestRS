//! The units join: every unit of work the framework opens, against the one
//! place its canonical name is allowed to come from.
//!
//! A unit of work has **one** name — `<edge>.<unit>`, declared as
//! `<owning crate>::unit::<UNIT>` — and that name is read three times: by the
//! `operation_span!` that opens the unit, by its operation line's `name:`, and
//! by that line's `message`. Sharing a constant is what makes the three agree;
//! this join is what stops a fourth site from spelling a fourth string.
//!
//! The names live **with their edges**, not in the kernel: a unit name is
//! per-edge vocabulary exactly as a span target is, so `nest_rs_core` holds the
//! grammar (`operation_log`) and each edge holds its own name. That is what
//! moves the shape and namespace checks here — a hand-written member list in
//! the kernel could only ever police the names the kernel could see.
//!
//! It had to: the spans said `http.request` and `mcp.operation` while the lines
//! said `request served` and `operation served`, two vocabularies for six
//! things, and `nest-rs-ws` carried the workaround in a comment — "`lifecycle`
//! rather than the span's name because `tracing` offers no way to read one
//! back". Nothing failed while that was true, which is the whole argument for
//! checking it here rather than trusting the next author to notice.
//!
//! Members are derived, never listed: every `operation_span!` call site and
//! every `tracing::info!` whose target is `operation_log::TARGET`, under
//! `crates/` and `demo/` — **inside a doctest as well as in an item**, since the
//! example is what a developer copies and it is the site this join reached
//! last.
//!
//! A unit's span says how the unit ended, as its line does: every file that
//! opens one records the outcome through `operation_log::record_outcome` (or
//! `record_error`, the form under it), or the `error.type` and status the span
//! declares export empty for a unit that failed.
//!
//! **What it reads by its spelling**: `operation_span!` and `info!` calls, at a
//! call site and inside a `macro_rules!` transcriber alike — a slot the
//! transcriber's caller fills is a binding, which no site may spell — calls to
//! `record_outcome` and `record_error`, and the `CANCELLED` and `PANIC`
//! constants of `operation_log`, by name, in each edge's shipped source. The
//! `blinds` join refuses a rename of any of them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    Named, crate_dirs, declared_units, doctests, each_source, flatten, is_cfg_test, item_attrs,
    named_at, operation_log_target, parsed, past_value, repo_root, resolve_target, rust_files,
    top_level, value_after,
};
use proc_macro2::{TokenStream, TokenTree};
use syn::Macro;
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![
        Followed::call(OPEN).read_in_transcribers(),
        Followed::call(LINE)
            .through(&["tracing", "log"])
            .read_in_transcribers(),
        Followed::call(RECORD_OUTCOME),
        Followed::call(RECORD_ERROR),
        Followed::type_(BUILT_ENDS[0])
            .through(&["operation_log"])
            .read_in_transcribers(),
        Followed::type_(BUILT_ENDS[1])
            .through(&["operation_log"])
            .read_in_transcribers(),
    ]
}

/// Below this the scan is reading the wrong tree, and a join that finds nothing
/// reads exactly like a join that found nothing wrong.
const FLOOR: usize = 8;

/// The floor for the sites that *open* a unit, which is a different population
/// from the sites that *name* one — a bare literal at its call site until it
/// was named, and the only floor in this module a reader could not review.
const OPENED_FLOOR: usize = 6;

/// The two vocabularies a site may name, and the module each lives in.
///
/// Checking the **module** rather than merely "is it a path" is what makes the
/// failure sentences true: `crate::whatever::REQUEST` used to satisfy a message
/// saying the name must come from a `unit` module. The module is checked and
/// the *crate* is not, deliberately: which crate owns an edge is the edge's
/// question, and the shape check below is what holds the value to the closed
/// namespace whoever declared it.
const UNIT_MODULE: &str = "unit";
const KIND_MODULE: &str = "kind";
/// Not a vocabulary this join classifies — the operation line's `target:` is
/// resolved rather than classified. It labels the one refusal this join owes
/// about a target, so the sentence names the module a target comes from rather
/// than the one a unit name does.
const TARGET_MODULE: &str = "target";

/// What a slot that named nothing at all is recorded as.
///
/// A [`Spelled::Binding`] like any other, so the refusal it lands in is the one
/// worded for a name the join cannot follow — the case an edge that forgot the
/// slot fails on.
const ABSENT: &str = "<absent>";

/// What a slot a `macro_rules!` caller fills is recorded as.
const METAVARIABLE: &str = "<a macro_rules! metavariable>";

/// How a site named its unit, once the shared reader has classified it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Spelled {
    /// A path into a `unit` module — the only conformant form. The payload is
    /// the constant's name.
    Unit(String),
    /// A path that is not one, named as written.
    Elsewhere(String),
    /// A string literal: a second spelling of a name that already exists.
    Literal(String),
    /// A local binding, or nothing at all. Never conformant: a line's `name:` is
    /// baked into its callsite, so a site taking the name as an argument cannot
    /// fill it — which is why each unit files from a callsite of its own.
    Binding(String),
}

/// Whether this `tracing::info!` files on `operation_log::TARGET`.
///
/// Resolved to the **value**, never matched on the constant's name: every crate
/// spells its own concern `TARGET` too, so `target: crate::TARGET` in
/// `nest-rs-social` is an ordinary event, and reading the name alone claimed
/// eight of them as operation lines.
fn is_operation_line(tokens: &[TokenTree], file: &str) -> Membership {
    let Some(at) = value_after(tokens, "target", ':') else {
        // No `target:` at all is an ordinary event — the operation line always
        // names one, so nothing is dropped here.
        return Membership::Outside;
    };
    match named_at(tokens, at) {
        Some((Named::Path(_), segments)) => {
            if resolve_target(&segments, file) == operation_log_target() {
                Membership::Family
            } else {
                Membership::Outside
            }
        }
        // **A literal target used to leave the population instead of failing
        // it**, which is the one direction a conformance join may never go: an
        // `info!(target: "nest_rs::operation", …)` was never classified, so its
        // `name:` and `message` were free to spell a fourth string for a unit of
        // work and this join stayed green. A literal *equal to* the family's
        // target is a member — and is itself reported, since a target the
        // framework interprets is a constant declared by its owner. A literal
        // naming anything else is an ordinary event, and whether it should have
        // been a constant is the `filters` join's subject rather than this
        // one's.
        Some((Named::Literal(text), _)) if Some(text.as_str()) == operation_log_target() => {
            Membership::FamilyByLiteral
        }
        _ => Membership::Outside,
    }
}

/// Whether an `info!` belongs to the operation-line family, and how it said so.
enum Membership {
    /// Not an operation line.
    Outside,
    /// An operation line, naming the family's target through its constant.
    Family,
    /// An operation line whose target is spelled as a literal.
    FamilyByLiteral,
}

/// Classify the value at `at` against the module it is required to come from.
fn spelled_at(tokens: &[TokenTree], at: usize, module: &str) -> Option<Spelled> {
    let (named, segments) = named_at(tokens, at)?;
    Some(match named {
        Named::Literal(text) => Spelled::Literal(text),
        Named::Path(last) if segments.len() == 1 => Spelled::Binding(last),
        Named::Path(last) if segments.iter().any(|s| s == module) => Spelled::Unit(last),
        Named::Path(_) => Spelled::Elsewhere(segments.join("::")),
    })
}

#[derive(Default)]
struct Scan {
    file: String,
    /// `(site, the module it must read, how it named its value)` → the files.
    ///
    /// The module travels in the key rather than being recovered from the site
    /// when the failure is worded: a site's vocabulary is decided once, where
    /// [`spelled_at`] is called, and a second mapping beside the sentence is
    /// what let every refusal say `unit` — including the ones about a span
    /// kind.
    found: BTreeMap<(&'static str, &'static str, Spelled), BTreeSet<String>>,
    /// Every `operation_span!` that names a unit constant: `(constant, the
    /// path's segments before `unit`, joined by `::`)` → the files. That prefix
    /// is what says which crate the site reached for — `crate`, a sibling's
    /// name, or the umbrella's `nest_rs::<crate>` — which is the one fact
    /// [`a_unit_is_opened_only_by_the_crate_that_declares_it`] reads.
    opened: BTreeMap<(String, String), BTreeSet<String>>,
    /// Every file that records a unit's outcome on its span —
    /// [`RECORD_OUTCOME`] or [`RECORD_ERROR`] called.
    recorded: BTreeSet<String>,
}

impl Scan {
    fn record(&mut self, site: &'static str, module: &'static str, named: Spelled) {
        self.found
            .entry((site, module, named))
            .or_default()
            .insert(self.file.clone());
    }

    fn record_opened(&mut self, constant: String, root_segment: String) {
        self.opened
            .entry((constant, root_segment))
            .or_default()
            .insert(self.file.clone());
    }
}

impl<'ast> Visit<'ast> for Scan {
    // A `#[cfg(test)]` emission opens no unit of work — it renders a line so a
    // formatter can be asserted against it, which is what
    // `nest_rs_core::logging`'s tests do and why the kernel spells a fixture
    // name there. Correcting the population, not waiving a member: every real
    // site is compiled into the shipped crate, and a transport that hid one
    // behind `cfg(test)` would be shipping nothing.
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if is_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if is_cfg_test(&node.attrs) {
            return;
        }
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        let name = node
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        // Named before flattened: `top_level` clones the stream, and every
        // `assert!`, `format!` and `quote!` body in both workspaces reaches
        // this visitor. Only these two macro names are ever read — at a call,
        // and inside a `macro_rules!` transcriber, where `syn` stops at the
        // tokens: MCP's notifications file their operation line from one, and
        // it was the one line of the family this join never read.
        if name == "macro_rules" {
            for (called, args) in transcribed_calls(&node.tokens) {
                self.read(&called, &args);
            }
        } else if name == OPEN || name == LINE {
            self.read(&name, &node.tokens);
        }
        syn::visit::visit_macro(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(called) = &*node.func
            && called
                .path
                .segments
                .last()
                .is_some_and(|last| last.ident == RECORD_OUTCOME || last.ident == RECORD_ERROR)
        {
            self.recorded.insert(self.file.clone());
        }
        syn::visit::visit_expr_call(self, node);
    }
}

/// The two calls that record how a unit ended on its span:
/// `operation_log::record_outcome`, and the form under it HTTP calls with a
/// status code.
const RECORD_OUTCOME: &str = "record_outcome";
const RECORD_ERROR: &str = "record_error";

/// The macro that opens a unit, and the one that files its line.
const OPEN: &str = "operation_span";
const LINE: &str = "info";

/// Every `operation_span!(…)` and `info!(…)` a `macro_rules!` body writes, with
/// its argument tokens.
fn transcribed_calls(body: &TokenStream) -> Vec<(String, TokenStream)> {
    let mut flat = Vec::new();
    nest_rs_conformance::sources::flatten(body.clone(), &mut flat);
    flat.windows(3)
        .filter_map(|window| match window {
            [
                TokenTree::Ident(name),
                TokenTree::Punct(bang),
                TokenTree::Group(args),
            ] if bang.as_char() == '!' && (name == OPEN || name == LINE) => {
                Some((name.to_string(), args.stream()))
            }
            _ => None,
        })
        .collect()
}

impl Scan {
    /// One call to `name` with `args`, read for the slots its family fixes.
    fn read(&mut self, name: &str, args: &TokenStream) {
        let tokens = top_level(args);
        // A value a `macro_rules!` caller supplies is no constant this join can
        // read at the definition — and its call site is no `operation_span!`.
        // Recorded as a binding, which no site may spell.
        if tokens
            .iter()
            .any(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == '$'))
        {
            let site = if name == OPEN {
                "operation_span!"
            } else {
                "message"
            };
            self.record(site, UNIT_MODULE, Spelled::Binding(METAVARIABLE.to_owned()));
            return;
        }

        if name == OPEN {
            // `operation_span!(target: …, kind: …, <name>, &correlation, …)`.
            // The unit's name is the positional argument after the kind, and the
            // kind is a path now, so the value has to be *walked* rather than
            // stepped over — a fixed offset read the middle of it.
            if let Some(kind_at) = value_after(&tokens, "kind", ':') {
                if let Some(named) = spelled_at(&tokens, kind_at, KIND_MODULE) {
                    self.record("kind:", KIND_MODULE, named);
                }
                let unit_at = past_value(&tokens, kind_at);
                if let Some(named) = spelled_at(&tokens, unit_at, UNIT_MODULE) {
                    if let Spelled::Unit(constant) = &named
                        && let Some((_, segments)) = named_at(&tokens, unit_at)
                    {
                        let prefix: Vec<String> = segments
                            .iter()
                            .take_while(|s| *s != UNIT_MODULE)
                            .cloned()
                            .collect();
                        self.record_opened(constant.clone(), prefix.join("::"));
                    }
                    self.record("operation_span!", UNIT_MODULE, named);
                }
            }
        } else {
            // Membership first, then the refusal it owes, then the slots — three
            // steps, read as three. Deciding and recording inside one `&&`
            // operand hid a mutation in a condition.
            let membership = is_operation_line(&tokens, &self.file);
            if matches!(membership, Membership::FamilyByLiteral) {
                let target = operation_log_target().unwrap_or_default().to_owned();
                self.record("target:", TARGET_MODULE, Spelled::Literal(target));
            }
            if matches!(membership, Membership::Outside) {
                return;
            }
            // Both slots carry the same value under the same rule, so they are
            // read by one loop: a third one is a row, not a third spelling.
            for (site, key, punct) in [("message", "message", '='), ("name:", "name", ':')] {
                let named = value_after(&tokens, key, punct)
                    .and_then(|at| spelled_at(&tokens, at, UNIT_MODULE))
                    .unwrap_or_else(|| Spelled::Binding(ABSENT.to_owned()));
                self.record(site, UNIT_MODULE, named);
            }
        }
    }
}

/// An operation line a `macro_rules!` writes is read like one at a call site,
/// and a slot its caller fills is a binding — never a line the join skips.
#[test]
fn an_operation_line_in_a_macro_rules_body_is_read() {
    let file: syn::File = syn::parse_quote! {
        macro_rules! files_its_line {
            () => {
                tracing::info!(
                    name: crate::unit::OPERATION,
                    target: nest_rs_core::operation_log::TARGET,
                    message = crate::unit::OPERATION,
                );
            };
        }
        macro_rules! hands_it_on {
            ($unit:expr) => {
                nest_rs_core::operation_span!(target: T, kind: nest_rs_core::operation_log::kind::SERVER, $unit, &c)
            };
        }
    };
    let mut scan = Scan::default();
    scan.visit_file(&file);
    let read: Vec<(&str, Spelled)> = scan
        .found
        .keys()
        .map(|(site, _, spelled)| (*site, spelled.clone()))
        .collect();
    assert!(
        read.contains(&("message", Spelled::Unit("OPERATION".to_owned()))),
        "{read:?}"
    );
    assert!(
        read.contains(&("operation_span!", Spelled::Binding(METAVARIABLE.to_owned()))),
        "{read:?}"
    );
}

fn scan_all(root: &Path) -> Scan {
    let mut scan = Scan::default();
    each_source(root, |rel, ast| {
        scan.file = rel.to_owned();
        scan.visit_file(ast);
        // The example a developer copies is a member of this family too. It was
        // not, and the canonical `operation_span!` doctest spelled all three
        // slots as literals for exactly as long — the one site teaching the
        // grammar was the one site never read.
        scan.file = format!("{rel} (doctest)");
        for example in doctests(ast) {
            scan.visit_file(&example);
        }
    });
    scan
}

/// **A unit of work is opened only by the crate that declares it** — the port
/// owns the semantics, an adapter owns the transport.
///
/// `nest_rs_queue::unit::JOB` is what a job attempt *is*; the span that opens
/// it, the events that classify its outcome and the line that reports it are
/// one body of semantics, and `architecture.md` (*Ports & Adapters*) puts that
/// body in the port. `nest-rs-redis/src/worker/consumer.rs` opened that span
/// itself through 5.1, which meant a second queue adapter would have copied the
/// envelope, the trace continuation, the panic catch and three events — and
/// corrected them twice. The span moved into `nest_rs_queue::consume::attempt`
/// and this join is what keeps it there: an `operation_span!` that reaches for a
/// *sibling's* `unit` module is an adapter taking semantics it does not own.
///
/// The declaring crate's own `*-macros` companion may open it too — a
/// decorator's expansion is the owning crate's code, emitted elsewhere —
/// whether it spells the owner directly (`::nest_rs_queue::unit::JOB`) or
/// through the umbrella (`::nest_rs::queue::unit::JOB`): both name the owner in
/// the path, and both are resolved from it, never from who happens to emit.
/// No baseline: the one hole was closed in the change that wrote this.
#[test]
fn a_unit_is_opened_only_by_the_crate_that_declares_it() {
    let root = repo_root();
    let opened = scan_all(&root).opened;
    baseline::floor(
        opened.len(),
        OPENED_FLOOR,
        "`operation_span!` site(s) naming a unit",
    );

    let mut holes = BTreeSet::new();
    for ((constant, prefix), files) in &opened {
        for file in files {
            // `crate::unit::X`, and a bare `unit::X` reached through a `use`, are
            // by construction the emitting crate's own — and the only way a
            // crate without a `unit` module could write them is not to compile.
            let Some(owner) = owner_of(prefix) else {
                continue;
            };
            let emitter = crate_of(file);
            let allowed = emitter == owner || emitter == format!("{owner}-macros");
            if !allowed {
                holes.insert(format!(
                    "`{constant}` is declared by `{owner}` and opened by `{emitter}` \
                     ({file}) — the span, its events and its line belong to the port; \
                     the adapter calls the port's `consume` function instead",
                ));
            }
        }
    }
    assert!(
        holes.is_empty(),
        "{} unit(s) opened outside the crate that declares them:\n  {}",
        holes.len(),
        holes.iter().cloned().collect::<Vec<_>>().join("\n  "),
    );
}

/// The crate directory a repo-relative source path sits in (`crates/x/src/..`
/// → `x`, `demo/crates/features/src/..` → `features`).
fn crate_of(file: &str) -> String {
    let parts: Vec<&str> = file.split('/').collect();
    parts
        .iter()
        .position(|p| *p == "crates" || *p == "apps")
        .and_then(|i| parts.get(i + 1))
        .map(|s| (*s).to_owned())
        .unwrap_or_default()
}

/// The crate a unit path's prefix (everything before `unit`) names, or `None`
/// when the prefix is the emitting crate's own (`crate::`, `self::`, `super::`,
/// or a bare `unit::X` reached through a `use`). `nest_rs_queue` →
/// `nest-rs-queue`; the umbrella form `nest_rs::queue` → `nest-rs-queue`, read
/// from the path's own second segment — never from who happens to emit, which
/// is the tautology this function replaced.
fn owner_of(prefix: &str) -> Option<String> {
    let mut segments = prefix.split("::").filter(|s| !s.is_empty());
    let first = segments.next()?;
    match first {
        "crate" | "self" | "super" => None,
        "nest_rs" => segments
            .next()
            .map(|second| format!("nest-rs-{}", second.replace('_', "-"))),
        other => Some(other.replace('_', "-")),
    }
}

#[test]
fn every_unit_of_work_is_named_by_the_shared_constant() {
    let root = repo_root();
    let found = scan_all(&root).found;
    baseline::floor(found.len(), FLOOR, "unit-naming site(s)");

    let mut wrong: Vec<String> = Vec::new();
    for ((site, module, named), files) in &found {
        let where_ = files.iter().cloned().collect::<Vec<_>>().join(", ");
        match named {
            Spelled::Unit(_) => {}
            Spelled::Literal(text) => wrong.push(format!(
                "{site} spells `{text}` as a literal ({where_}) — it must read a \
                 constant from its owning crate's `{module}` module",
            )),
            Spelled::Elsewhere(path) => wrong.push(format!(
                "{site} names `{path}` ({where_}), which is not a constant in a \
                 `{module}` module",
            )),
            Spelled::Binding(text) => wrong.push(format!(
                "{site} reads `{text}` ({where_}), which this join cannot trace \
                 back to a `{module}` module",
            )),
        }
    }
    wrong.sort();
    assert!(
        wrong.is_empty(),
        "{} unit-naming site(s) do not read the shared constant, so a unit of \
         work can be called two things:\n  {}",
        wrong.len(),
        wrong.join("\n  "),
    );

    // Every name **used at either site** is opened by a span and filed by a
    // line — which is not quite "every declared name", and the difference is
    // stated because it is a gap: a constant declared in a `unit` module and
    // never used at either site is in neither set and passes here. The sibling
    // test below is the one that reads `declared_units()`, so a name that
    // exists is at least held to the grammar; that it is *emitted* is what this
    // half cannot see. Today the two populations coincide (six declarations,
    // six used), which is exactly why it is invisible.
    let by_site = |site: &str| -> BTreeSet<String> {
        found
            .keys()
            .filter(|(s, _, _)| *s == site)
            .filter_map(|(_, _, n)| match n {
                Spelled::Unit(c) => Some(c.clone()),
                _ => None,
            })
            .collect()
    };
    let spans = by_site("operation_span!");
    let messages = by_site("message");

    let unopened: Vec<String> = messages.difference(&spans).cloned().collect();
    let unfiled: Vec<String> = spans.difference(&messages).cloned().collect();
    assert!(
        unopened.is_empty(),
        "unit(s) filed on a line with no `operation_span!` opening them: {}",
        unopened.join(", "),
    );
    // Keyed on the constant's **name**, which is what a site spells, and that is
    // a limit worth stating: two crates may each declare a `MESSAGE`, and the
    // namespace check in the sibling test forces their *values* apart
    // (`ws.message` vs `events.message`) while placing no constraint on the
    // identifier. A future `nest_rs_events::unit::MESSAGE` opened as a span with
    // no line would be cross-satisfied by `nest_rs_ws::unit::MESSAGE`'s line —
    // verbatim the scenario this half exists to catch. The disambiguating pair
    // is `declared_units()`'s `(crate, constant)`; using it needs the *site* to
    // carry the declaring crate too, which `spelled_at` does not resolve, so it
    // is stated here rather than half-built.
    //
    let anonymous = unfiled;
    assert!(
        anonymous.is_empty(),
        "unit(s) opened as a span that file no operation line, so their work is \
         anonymous on the console: {}",
        anonymous.join(", "),
    );
}

/// Below this the scan is reading the wrong tree — six edges declare a unit
/// today and one of them declares three.
const DECLARED_FLOOR: usize = 6;

/// Every declared name is `<edge>.<unit>`, and it says who owns it.
///
/// This lived in `nest-rs-core` as a hand-written array for as long as the
/// kernel held the names — the shape `testing.md` names as the defect, and one
/// that could only ever police the members whoever typed it remembered. The
/// names moved to their edges; the rule moved here, over a population read out
/// of the source.
#[test]
fn every_declared_unit_name_is_an_edge_namespace_that_names_its_owner() {
    let declared = declared_units();
    baseline::floor(declared.len(), DECLARED_FLOOR, "declared unit name(s)");

    let mut wrong: Vec<String> = Vec::new();
    for (name, krate, konst) in &declared {
        let at = format!("`{konst}` in {krate}");
        let Some((namespace, tail)) = name.split_once('.') else {
            wrong.push(format!(
                "{at} declares `{name}`, which is not `<edge>.<unit>`"
            ));
            continue;
        };
        if !crate::EDGES.contains(&namespace) {
            wrong.push(format!(
                "{at} declares `{name}`, whose namespace is outside the closed edge \
                 vocabulary; open `{namespace}` as an edge in `architecture.md` first",
            ));
        }
        // The namespace names the edge, and the crate that owns the edge is the
        // crate that declares the name — so the two cannot disagree without one
        // of them lying about who a unit of work belongs to. Derived, unlike the
        // vocabulary above: it is read off the path the declaration sits at.
        else if krate.strip_prefix("nest-rs-") != Some(namespace) {
            wrong.push(format!(
                "{at} declares `{name}`, but `{namespace}` is another crate's edge — a unit \
                 name is declared by the crate that owns it",
            ));
        }
        if tail.is_empty() || tail.contains('.') {
            wrong.push(format!(
                "{at} declares `{name}`, which must carry exactly one dot"
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
        {
            wrong.push(format!("{at} declares `{name}`, which must be lowercase"));
        }
    }
    let distinct: BTreeSet<&String> = declared.iter().map(|(name, _, _)| name).collect();
    if distinct.len() != declared.len() {
        wrong.push(format!(
            "two units of work share one canonical name, so nothing can tell them apart: {} \
             declarations for {} names",
            declared.len(),
            distinct.len(),
        ));
    }

    wrong.sort();
    assert!(
        wrong.is_empty(),
        "{} declared unit name(s) are off the grammar `nest_rs_core::operation_log` \
         states:\n  {}",
        wrong.len(),
        wrong.join("\n  "),
    );
}

/// **A unit's span says how it ended, wherever one is opened.**
///
/// `operation_span!` declares `error.type` and `otel.status_code` on every
/// unit's span, and an edge fills them through `operation_log::record_outcome`
/// where it files the unit's line. A declared field nothing records is the
/// defect the macro's own docs name, and here it is silent twice over: the span
/// exports, and a backend shows a request cut at the shutdown window, or a job
/// that panicked, as an operation that succeeded. Read per file, which is the
/// granularity every edge keeps — the file that opens a unit is the file that
/// settles it, HTTP's span helpers included — so an edge that opens a unit and
/// never records one fails here the day it lands.
#[test]
fn every_file_that_opens_a_unit_records_how_it_ended() {
    let scan = scan_all(&repo_root());
    let opening: BTreeSet<&String> = scan
        .opened
        .values()
        .flatten()
        .filter(|file| !file.ends_with("(doctest)"))
        .collect();
    baseline::floor(opening.len(), OPENING_FLOOR, "file(s) opening a unit");
    let silent: Vec<&str> = opening
        .into_iter()
        .filter(|file| !scan.recorded.contains(*file))
        .map(String::as_str)
        .collect();
    assert!(
        silent.is_empty(),
        "{} file(s) open a unit of work and never record how it ended on its span — call \
         `nest_rs_core::operation_log::record_outcome` where the line is filed, or the span \
         exports a failed unit as a success:\n  {}",
        silent.len(),
        silent.join("\n  "),
    );
}

/// Below this the scan is reading the wrong tree: eight files open a unit today.
const OPENING_FLOOR: usize = 8;

/// The reading itself: a file that opens a unit and records it is told apart
/// from one that opens a unit and records nothing.
#[test]
fn a_recorded_outcome_is_read_off_the_call() {
    let records: syn::File = syn::parse_quote! {
        fn open() {
            let span = nest_rs_core::operation_span!(target: T, kind: nest_rs_core::operation_log::kind::SERVER, crate::unit::JOB, &c);
            nest_rs_core::operation_log::record_outcome(&span, outcome);
        }
    };
    let silent: syn::File = syn::parse_quote! {
        fn open() {
            let span = nest_rs_core::operation_span!(target: T, kind: nest_rs_core::operation_log::kind::SERVER, crate::unit::TICK, &c);
        }
    };
    let mut scan = Scan {
        file: "records.rs".to_owned(),
        ..Scan::default()
    };
    scan.visit_file(&records);
    scan.file = "silent.rs".to_owned();
    scan.visit_file(&silent);

    assert_eq!(scan.recorded, BTreeSet::from(["records.rs".to_owned()]));
    let opening: BTreeSet<&String> = scan.opened.values().flatten().collect();
    assert_eq!(opening.len(), 2, "{:?}", scan.opened);
}

/// The two ends of a unit an edge has to build, by the `operation_log` constant
/// each is filed under.
const BUILT_ENDS: [&str; 2] = ["CANCELLED", "PANIC"];

/// Every edge can file a unit stopped before it settled, and a unit that
/// unwound.
///
/// `ok` and `error` come with a handler's return; `cancelled` and `panic` do
/// not — one is a guard dropped with the unit's future, the other a containment
/// at the dispatch — so they are the two ends an edge leaves out, and leaving
/// them out is silent: the unit a shutdown or a panic ended files nothing, and
/// the family still reads as whole. Three edges had neither for a round.
///
/// **Per edge, not per unit**, and that is this cell's stated limit: what makes
/// a member is a crate declaring a `unit` module, and what it owes is both
/// constants named in its shipped source — outside `#[cfg(test)]`, so a test
/// asserting the word does not fill the cell. Which of an edge's units files
/// each end is that edge's own suite to prove.
#[test]
fn every_edge_can_file_a_unit_stopped_and_a_unit_unwound() {
    let edges: BTreeSet<String> = declared_units()
        .into_iter()
        .map(|(_, krate, _)| krate)
        .collect();
    baseline::floor(edges.len(), EDGES_FLOOR, "edge(s) declaring a unit");
    let mut holes = BTreeSet::new();
    for dir in crate_dirs() {
        let Some(edge) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !edges.contains(edge) {
            continue;
        }
        let named = shipped_idents(&dir.join("src"));
        for end in BUILT_ENDS {
            if !named.contains(end) {
                holes.insert(format!("{edge} {}", end.to_ascii_lowercase()));
            }
        }
    }
    baseline::gate(
        "units-ends-baseline.txt",
        &holes,
        edges.len(),
        "edge(s)",
        "edge × built end",
        "an edge whose shipped source cannot file that end — build it (a guard dropped with \
         the unit for `cancelled`, a containment at the dispatch for `panic`) or record why the \
         edge never ends a unit that way",
    );
}

/// The six edges that declare a unit today.
const EDGES_FLOOR: usize = 6;

/// Every identifier the crate's shipped source spells — outside any item, and
/// any member of an `impl` or a `trait`, that `#[cfg(test)]` gates.
///
/// Tokens rather than a visitor, because the constants this cell asks for are
/// as often spelled inside a `tracing` call as in an expression, and a macro's
/// arguments are tokens to `syn`.
fn shipped_idents(src: &Path) -> BTreeSet<String> {
    fn tokens(tokens: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
        let mut flat = Vec::new();
        flatten(tokens, &mut flat);
        out.extend(flat.into_iter().filter_map(|tree| match tree {
            TokenTree::Ident(ident) => Some(ident.to_string()),
            _ => None,
        }));
    }
    fn walk(items: &[syn::Item], out: &mut BTreeSet<String>) {
        use quote::ToTokens;
        for item in items {
            if is_cfg_test(item_attrs(item)) {
                continue;
            }
            match item {
                syn::Item::Mod(module) => {
                    if let Some((_, inner)) = &module.content {
                        walk(inner, out);
                    }
                }
                syn::Item::Impl(block) => {
                    for member in &block.items {
                        let attrs = match member {
                            syn::ImplItem::Const(m) => &m.attrs,
                            syn::ImplItem::Fn(m) => &m.attrs,
                            syn::ImplItem::Type(m) => &m.attrs,
                            syn::ImplItem::Macro(m) => &m.attrs,
                            _ => &Vec::new(),
                        };
                        if !is_cfg_test(attrs) {
                            tokens(member.to_token_stream(), out);
                        }
                    }
                }
                syn::Item::Trait(declared) => {
                    for member in &declared.items {
                        let attrs = match member {
                            syn::TraitItem::Const(m) => &m.attrs,
                            syn::TraitItem::Fn(m) => &m.attrs,
                            syn::TraitItem::Type(m) => &m.attrs,
                            syn::TraitItem::Macro(m) => &m.attrs,
                            _ => &Vec::new(),
                        };
                        if !is_cfg_test(attrs) {
                            tokens(member.to_token_stream(), out);
                        }
                    }
                }
                _ => tokens(item.to_token_stream(), out),
            }
        }
    }
    let mut out = BTreeSet::new();
    for file in rust_files(src) {
        if let Some(ast) = parsed(&file) {
            walk(&ast.items, &mut out);
        }
    }
    out
}
