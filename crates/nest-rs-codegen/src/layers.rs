//! The layer grammar: one closed table of which layer attribute every
//! decorator pair takes, on its host and on its methods, and the sentence a
//! refused one is answered with.
//!
//! A site reading its layers through [`take_layers`] answers an attribute it
//! does not run with a named compile error, never a silent no-op nor rustc's
//! `cannot find attribute`. An edge that starts running a family flips its cell
//! here, in the change that builds it.
//!
//! ```
//! use nest_rs_codegen::layers::{LayerScope, take_layers};
//! use nest_rs_codegen::pair;
//!
//! let mut attrs: Vec<syn::Attribute> = vec![syn::parse_quote!(#[use_guards(AuthnGuard)])];
//! let layers = take_layers(&pair::HTTP, LayerScope::Host, &mut attrs).unwrap();
//! assert_eq!(layers.guards.len(), 1);
//! assert!(attrs.is_empty());
//!
//! let mut attrs: Vec<syn::Attribute> = vec![syn::parse_quote!(#[use_guards(AuthnGuard)])];
//! let refused = take_layers(&pair::HOOKS, LayerScope::Method, &mut attrs);
//! assert!(refused.is_err_and(|e| e.to_string().contains("no caller")));
//! ```

use quote::ToTokens;
use syn::{Attribute, Expr, Path};

use crate::attrs::take_path_list;
use crate::meta::take_meta;
use crate::pair::{DecoratorPair, PairKind};

/// A layer attribute: one layer family's binding, or `#[meta]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayerAttribute {
    /// `#[use_guards(...)]`.
    UseGuards,
    /// `#[force_guards(...)]`: guards re-run at a method though a broader
    /// scope ran them.
    ForceGuards,
    /// `#[use_pipes(...)]`: global pipes scoped to a site.
    UsePipes,
    /// `#[use_interceptors(...)]`.
    UseInterceptors,
    /// `#[use_exception_filters(...)]`.
    UseExceptionFilters,
    /// `#[use_filters(...)]`: the untyped error mapper an exception filter
    /// catching `dyn Error + Send + Sync` replaced, refused everywhere by name.
    UseFilters,
    /// `#[meta(EXPR)]`.
    Meta,
}

impl LayerAttribute {
    /// Every layer attribute, in the order a site reads them.
    pub const ALL: &'static [Self] = &[
        Self::UseGuards,
        Self::ForceGuards,
        Self::UsePipes,
        Self::UseInterceptors,
        Self::UseExceptionFilters,
        Self::UseFilters,
        Self::Meta,
    ];

    /// The attribute's name, as written (`"use_guards"`).
    pub const fn name(self) -> &'static str {
        match self {
            Self::UseGuards => "use_guards",
            Self::ForceGuards => "force_guards",
            Self::UsePipes => "use_pipes",
            Self::UseInterceptors => "use_interceptors",
            Self::UseExceptionFilters => "use_exception_filters",
            Self::UseFilters => "use_filters",
            Self::Meta => "meta",
        }
    }

    /// The layer attribute `attr` is, by its last path segment.
    pub(crate) fn of(attr: &Attribute) -> Option<Self> {
        let last = attr.path().segments.last()?;
        Self::ALL
            .iter()
            .copied()
            .find(|candidate| last.ident == candidate.name())
    }
}

/// Where on a pair a layer attribute is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayerScope {
    /// The host: an edge pair's struct (`#[controller]`), or the impl block a
    /// provider pair decorates (`#[processor]`).
    Host,
    /// One dispatched method.
    Method,
}

/// What one cell of the table answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cell {
    /// Taken, and emitted into the site.
    Built,
    /// The grammar's, but the edge does not run the family yet: refused as a
    /// silent no-op until the change that builds it flips the cell.
    Pending,
    /// Refused for good, with the fact that makes it meaningless here.
    Refused(&'static str),
}

/// A guard runs per unit; re-running one is a decision about one operation.
const FORCE_ON_A_HOST: &str = "re-running a guard is a decision about one operation, not a \
     posture a whole host takes — write `#[force_guards]` on the method";

/// Nothing calls a lifecycle hook but the boot and the shutdown.
const HOOK: &str = "a lifecycle hook has no caller and no site, so no layer runs around it and \
     nothing reads a value declared on it";

/// A probe is one part of the health endpoint's answer.
const INDICATOR: &str = "an indicator runs inside the health endpoint, whose layers are the \
     endpoint's — no layer runs around one probe, and nothing reads a value declared on it";

/// Guards gate a caller; a worker's unit has none.
const NO_CALLER: &str =
    "a job, a tick and an event have no caller to gate — guard where the work is produced";

/// A worker's input is validated on its argument.
const WORKER_PIPES: &str = "a job's payload, a tick and an event take per-argument pipes alone \
     — bind `Piped<P, T>` or `Valid<T>` on the argument";

/// GraphQL's variables meet the global pipes at the request.
const GRAPHQL_PIPES: &str = "a GraphQL operation's variables are folded once per request — bind \
     `Piped<P, T>` or `Valid<T>` on the argument";

/// MCP's arguments are one value.
const MCP_PIPES: &str = "an MCP operation's arguments are decoded once per call — bind \
     `Piped<P, T>` or `Valid<T>` inside `Parameters<…>`";

/// `#[use_filters]` bound HTTP's untyped `Filter`, which an exception filter
/// replaced.
const FILTERS: &str = "an exception filter whose `Exception` is `dyn Error + Send + Sync` \
     catches every error — bind that with `#[use_exception_filters]`";

/// What `attribute` answers at `scope` on `pair`.
///
/// Exhaustive over [`PairKind`]: a pair added without its row does not
/// compile.
pub fn cell(pair: &DecoratorPair, scope: LayerScope, attribute: LayerAttribute) -> Cell {
    use Cell::{Built, Pending, Refused};
    use LayerAttribute as A;
    use LayerScope::Host;
    use PairKind as P;
    match (pair.kind(), scope, attribute) {
        (P::Hooks, _, _) => Refused(HOOK),
        (P::Indicators, _, _) => Refused(INDICATOR),

        (P::Http | P::Graphql | P::Ws | P::Mcp, Host, A::ForceGuards) => Refused(FORCE_ON_A_HOST),
        (P::Http | P::Graphql | P::Ws | P::Mcp, _, A::UseGuards | A::ForceGuards | A::Meta) => {
            Built
        }

        (P::Http, _, A::UsePipes | A::UseInterceptors | A::UseExceptionFilters) => Built,
        (_, _, A::UseFilters) => Refused(FILTERS),

        (P::Graphql, _, A::UsePipes) => Refused(GRAPHQL_PIPES),
        (P::Mcp, _, A::UsePipes) => Refused(MCP_PIPES),
        (P::Ws, _, A::UsePipes) => Pending,
        (P::Graphql | P::Ws | P::Mcp, _, A::UseInterceptors | A::UseExceptionFilters) => Pending,

        (P::Processor | P::Scheduled | P::Listeners, _, A::UseGuards | A::ForceGuards) => {
            Refused(NO_CALLER)
        }
        (P::Processor | P::Scheduled | P::Listeners, _, A::UsePipes) => Refused(WORKER_PIPES),
        (
            P::Processor | P::Scheduled | P::Listeners,
            _,
            A::UseInterceptors | A::UseExceptionFilters | A::Meta,
        ) => Built,
    }
}

/// Where `scope` is on `pair`, as a refusal names it.
fn written_at(pair: &DecoratorPair, scope: LayerScope) -> String {
    match (scope, pair.on_a_provider()) {
        (LayerScope::Host, false) => format!("beside `{}`", pair.host()),
        (LayerScope::Host, true) => format!("on a `{}` block", pair.operations()),
        (LayerScope::Method, _) => format!("on a `{}` method", pair.operations()),
    }
}

/// The edge a pending cell waits on, as a reader names it.
fn edge_name(pair: &DecoratorPair) -> &'static str {
    match pair.kind() {
        PairKind::Http => "HTTP",
        PairKind::Graphql => "GraphQL",
        PairKind::Ws => "WebSockets",
        PairKind::Mcp => "MCP",
        PairKind::Hooks => "lifecycle hooks",
        PairKind::Processor => "the queue",
        PairKind::Indicators => "health indicators",
        PairKind::Scheduled => "scheduled tasks",
        PairKind::Listeners => "event listeners",
    }
}

/// The sentence a cell other than [`Cell::Built`] refuses `attribute` with.
pub fn refusal(
    pair: &DecoratorPair,
    scope: LayerScope,
    attribute: LayerAttribute,
) -> Option<String> {
    refusal_at(pair, scope, attribute, &written_at(pair, scope))
}

/// [`refusal`], naming the site as `at` — an edge's impl block refuses the
/// host's cell, but the attribute is written on the block.
fn refusal_at(
    pair: &DecoratorPair,
    scope: LayerScope,
    attribute: LayerAttribute,
    at: &str,
) -> Option<String> {
    let name = attribute.name();
    match cell(pair, scope, attribute) {
        Cell::Built => None,
        Cell::Pending => Some(format!(
            "`#[{name}]` does not run on {edge} yet, so {at} it would be a silent no-op. \
             Remove it, or bind the layer on an HTTP `#[controller]` / `#[routes]`, where it \
             runs",
            edge = edge_name(pair),
        )),
        Cell::Refused(fact) => Some(format!("`#[{name}]` is not taken {at}: {fact}")),
    }
}

/// The layers one site declares, taken off its attributes by [`take_layers`].
#[derive(Default)]
pub struct Layers {
    /// `#[use_guards]`, then each `#[force_guards]` entry: a forced guard is
    /// bound at its method as well as re-run.
    pub guards: Vec<Path>,
    /// `#[force_guards]`: the guards a broader scope's run does not dedupe.
    pub force_guards: Vec<Path>,
    /// `#[use_pipes]`.
    pub pipes: Vec<Path>,
    /// `#[use_interceptors]`.
    pub interceptors: Vec<Path>,
    /// `#[use_exception_filters]`.
    pub exception_filters: Vec<Path>,
    /// Every `#[meta]` value, in the order written.
    pub meta: Vec<Expr>,
}

/// Take every layer attribute off `attrs`, a site at `scope` on `pair`: each
/// cell the table builds into [`Layers`], and the first one it does not as a
/// compile error spanning it, worded by [`refusal`].
///
/// A guard listed by both `#[use_guards]` and `#[force_guards]` is refused:
/// `#[force_guards]` alone binds it and re-runs it.
pub fn take_layers(
    pair: &DecoratorPair,
    scope: LayerScope,
    attrs: &mut Vec<Attribute>,
) -> syn::Result<Layers> {
    for attr in attrs.iter() {
        let Some(attribute) = LayerAttribute::of(attr) else {
            continue;
        };
        if let Some(sentence) = refusal(pair, scope, attribute) {
            return Err(syn::Error::new_spanned(attr, sentence));
        }
    }
    let mut taken = |attribute: LayerAttribute| match cell(pair, scope, attribute) {
        Cell::Built => take_path_list(attrs, attribute.name()),
        Cell::Pending | Cell::Refused(_) => Ok(Vec::new()),
    };
    let mut layers = Layers {
        guards: taken(LayerAttribute::UseGuards)?,
        force_guards: taken(LayerAttribute::ForceGuards)?,
        pipes: taken(LayerAttribute::UsePipes)?,
        interceptors: taken(LayerAttribute::UseInterceptors)?,
        exception_filters: taken(LayerAttribute::UseExceptionFilters)?,
        meta: Vec::new(),
    };
    if cell(pair, scope, LayerAttribute::Meta) == Cell::Built {
        layers.meta = take_meta(attrs)?;
    }
    let rendered = |path: &Path| path.to_token_stream().to_string();
    for forced in &layers.force_guards {
        let guard = rendered(forced);
        if layers.guards.iter().any(|bound| rendered(bound) == guard) {
            return Err(syn::Error::new_spanned(
                forced,
                format!(
                    "`{guard}` is listed by both `#[use_guards]` and `#[force_guards]` — \
                     `#[force_guards]` alone binds it here and re-runs it; drop it from \
                     `#[use_guards]`",
                    guard = guard.replace(' ', ""),
                ),
            ));
        }
    }
    layers.guards.extend(layers.force_guards.iter().cloned());
    Ok(layers)
}

/// Refuse a layer attribute on a pair's operations block: an edge's host
/// layers go on the struct, and one the host does not take is refused with the
/// host's fact, naming the block it is written on.
pub(crate) fn reject_on_operations(pair: &DecoratorPair, attrs: &[Attribute]) -> syn::Result<()> {
    let at = format!("on a `{}` block", pair.operations());
    for attr in attrs {
        let Some(attribute) = LayerAttribute::of(attr) else {
            continue;
        };
        if attribute == LayerAttribute::Meta {
            continue;
        }
        let sentence = refusal_at(pair, LayerScope::Host, attribute, &at).unwrap_or_else(|| {
            format!(
                "`#[{name}(...)]` belongs on the {subject} beside `{host}`, not on this \
                 `{operations}` block — the struct half declares the host's layers, and this \
                 half declares its {collects}. A layer scoped to one operation goes on that \
                 operation's method.",
                name = attribute.name(),
                subject = pair.subject(),
                host = pair.host(),
                operations = pair.operations(),
                collects = pair.collects(),
            )
        });
        return Err(syn::Error::new_spanned(attr, sentence));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;
    use crate::pair::ALL;

    const SCOPES: [LayerScope; 2] = [LayerScope::Host, LayerScope::Method];

    #[test]
    fn every_cell_is_built_or_refused_with_a_sentence_naming_its_attribute() {
        for pair in ALL {
            for scope in SCOPES {
                for &attribute in LayerAttribute::ALL {
                    let sentence = refusal(pair, scope, attribute);
                    match cell(pair, scope, attribute) {
                        Cell::Built => assert!(sentence.is_none()),
                        Cell::Pending | Cell::Refused(_) => {
                            let sentence = sentence.unwrap_or_default();
                            assert!(
                                sentence.starts_with(&format!("`#[{}]`", attribute.name())),
                                "{} {scope:?}: {sentence}",
                                pair.operations(),
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_hook_and_an_indicator_take_no_layer_and_no_meta() {
        for pair in [&crate::pair::HOOKS, &crate::pair::INDICATORS] {
            for scope in SCOPES {
                for &attribute in LayerAttribute::ALL {
                    assert!(
                        matches!(cell(pair, scope, attribute), Cell::Refused(_)),
                        "{} {scope:?} {attribute:?}",
                        pair.operations(),
                    );
                }
            }
        }
    }

    #[test]
    fn a_worker_unit_takes_its_layers_but_no_guard_and_no_scoped_pipe() {
        use crate::pair::{LISTENERS, PROCESSOR, SCHEDULED};
        for pair in [&PROCESSOR, &SCHEDULED, &LISTENERS] {
            for scope in SCOPES {
                for attribute in [
                    LayerAttribute::UseGuards,
                    LayerAttribute::ForceGuards,
                    LayerAttribute::UsePipes,
                ] {
                    assert!(matches!(cell(pair, scope, attribute), Cell::Refused(_)));
                }
                for attribute in [
                    LayerAttribute::UseInterceptors,
                    LayerAttribute::UseExceptionFilters,
                    LayerAttribute::Meta,
                ] {
                    assert_eq!(cell(pair, scope, attribute), Cell::Built);
                }
            }
        }
    }

    #[test]
    fn every_edge_takes_guards_and_meta_and_forces_a_guard_on_its_methods_alone() {
        use crate::pair::{GRAPHQL, HTTP, MCP, WS};
        for pair in [&HTTP, &GRAPHQL, &WS, &MCP] {
            for scope in SCOPES {
                assert_eq!(cell(pair, scope, LayerAttribute::UseGuards), Cell::Built);
                assert_eq!(cell(pair, scope, LayerAttribute::Meta), Cell::Built);
            }
            assert_eq!(
                cell(pair, LayerScope::Method, LayerAttribute::ForceGuards),
                Cell::Built,
            );
            assert_eq!(
                cell(pair, LayerScope::Host, LayerAttribute::ForceGuards),
                Cell::Refused(FORCE_ON_A_HOST),
            );
        }
    }

    #[test]
    fn a_built_family_is_taken_off_the_site() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[doc = "kept"]),
            parse_quote!(#[use_guards(A, B)]),
            parse_quote!(#[use_interceptors(I)]),
            parse_quote!(#[use_exception_filters(F)]),
            parse_quote!(#[use_pipes(P)]),
            parse_quote!(#[meta(Tag)]),
        ];
        let layers = take_layers(&crate::pair::HTTP, LayerScope::Host, &mut attrs)
            .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(attrs.len(), 1);
        assert_eq!(
            (
                layers.guards.len(),
                layers.interceptors.len(),
                layers.exception_filters.len(),
                layers.pipes.len(),
                layers.meta.len(),
            ),
            (2, 1, 1, 1, 1),
        );
    }

    #[test]
    fn use_filters_is_refused_on_every_pair_naming_the_exception_filter() {
        use crate::pair::{GRAPHQL, HTTP, MCP, WS};
        for pair in [&HTTP, &GRAPHQL, &WS, &MCP] {
            for scope in SCOPES {
                let mut attrs: Vec<Attribute> = vec![parse_quote!(#[use_filters(L)])];
                let refused = take_layers(pair, scope, &mut attrs)
                    .err()
                    .map(|err| err.to_string())
                    .unwrap_or_default();
                assert!(refused.contains("#[use_exception_filters]"), "{refused}");
            }
        }
    }

    #[test]
    fn a_forced_guard_is_bound_at_its_method() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[use_guards(A)]),
            parse_quote!(#[force_guards(Fresh)]),
        ];
        let layers = take_layers(&crate::pair::WS, LayerScope::Method, &mut attrs)
            .unwrap_or_else(|err| panic!("{err}"));
        let guards: Vec<String> = layers
            .guards
            .iter()
            .map(|path| path.to_token_stream().to_string())
            .collect();
        assert_eq!(guards, ["A", "Fresh"]);
        assert_eq!(layers.force_guards.len(), 1);
    }

    #[test]
    fn a_guard_both_bound_and_forced_is_refused() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[use_guards(Fresh)]),
            parse_quote!(#[force_guards(Fresh)]),
        ];
        let refused = take_layers(&crate::pair::HTTP, LayerScope::Method, &mut attrs)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        assert!(refused.contains("`#[force_guards]` alone"), "{refused}");
    }

    #[test]
    fn a_refused_family_names_its_fact_and_a_pending_one_the_silent_no_op() {
        let mut attrs: Vec<Attribute> = vec![parse_quote!(#[use_pipes(P)])];
        let refused = take_layers(&crate::pair::GRAPHQL, LayerScope::Method, &mut attrs)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        assert!(refused.contains("folded once per request"), "{refused}");

        let mut attrs: Vec<Attribute> = vec![parse_quote!(#[use_interceptors(I)])];
        let pending = take_layers(&crate::pair::MCP, LayerScope::Host, &mut attrs)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        assert!(pending.contains("does not run on MCP yet"), "{pending}");
    }

    #[test]
    fn a_layer_on_an_operations_block_names_the_struct_or_the_hosts_refusal() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[use_interceptors(I)])];
        let moved = reject_on_operations(&crate::pair::HTTP, &attrs)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        assert!(moved.contains("beside `#[controller]`"), "{moved}");

        let attrs: Vec<Attribute> = vec![parse_quote!(#[use_pipes(P)])];
        let refused = reject_on_operations(&crate::pair::MCP, &attrs)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();
        assert!(refused.contains("inside `Parameters<…>`"), "{refused}");
        assert!(refused.contains("on a `#[tools]` block"), "{refused}");
    }

    #[test]
    fn a_host_cell_refused_on_an_operations_block_names_that_block() {
        let cases: [(&DecoratorPair, Attribute); 2] = [
            (&crate::pair::HTTP, parse_quote!(#[force_guards(X)])),
            (&crate::pair::GRAPHQL, parse_quote!(#[use_filters(X)])),
        ];
        for (pair, attr) in cases {
            let refused = reject_on_operations(pair, &[attr])
                .err()
                .map(|err| err.to_string())
                .unwrap_or_default();
            let block = format!("is not taken on a `{}` block", pair.operations());
            assert!(refused.contains(&block), "{refused}");
            assert!(!refused.contains(pair.host()), "{refused}");
        }
    }
}
