//! The durations join — every duration a deployment sets is read through
//! `DurationBounds`, and every `DurationBounds` names its unit in its key.
//!
//! `framework.md`: *"A duration a deployment sets has a floor and a ceiling …
//! One reader, one sentence: every bounded duration is read through
//! `nest_rs_config::DurationBounds`."* The ceiling is a required field there, so
//! the compiler holds every declaration to both ends; what the compiler cannot
//! see is a duration read **around** the type. Nine were: the OpenTelemetry
//! metric interval went through `env.parse::<u64>("METRIC_INTERVAL_SECS")`, with
//! no ceiling and a `0` read as "keep the default", HTTP's TLS reload through
//! `env.setting`, and seven — three connection ceilings, two keep-alives, MCP's
//! `retry:` and HTTP's request timeout — through a second reader,
//! `ConfigService::seconds`, whose `0` meant off and whose values had no ceiling
//! at all.
//!
//! **What it reads**: every call to one of `ConfigService`'s readers — the
//! methods of its `impl` taking a `key`, derived from `nest-rs-config`'s source
//! rather than listed — and to the free `env_var`, in the `src/` of every crate
//! in both workspaces outside `#[cfg(test)]`, a `macro_rules!` transcriber
//! included. A string literal among such a call's arguments, at any depth, that
//! ends in `_SECS` or `_MS` — the suffixes `DurationUnit` writes a duration's key
//! with — is a duration read around the type, and is refused. A literal naming
//! the variable in a sentence (`var_name(ns, "CONNECT_TIMEOUT_SECS")` in an
//! error) is no read, and passes. And every `DurationBounds { key, unit, .. }`
//! literal's key ends in its unit's suffix.
//!
//! **What it reads by its spelling** is `ConfigService`, whose `impl` names the
//! readers, `env_var`, the free one, and `DurationBounds`, whose literals carry
//! the keys — and the `blinds` join refuses each renamed, aliased or written by
//! a `macro_rules!` where this join does not read it. A key held in a `const`
//! and passed by name is a literal this join does not see at the call, so a
//! `const` string ending in a duration suffix is refused wherever it is
//! declared, unless it is a `DurationBounds`' own `key:`.

use std::collections::BTreeSet;

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, flatten, is_cfg_test, parsed, relative, repo_root, rust_files,
};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![
        Followed::type_("ConfigService"),
        Followed::type_("DurationBounds"),
        Followed::call("env_var").read_in_transcribers(),
    ]
}

/// `DurationBounds` declarations the walk must find — every bounded duration of
/// the framework. Below this the scan is reading the wrong tree.
const FLOOR: usize = 20;

/// The suffixes a duration's key carries, one per `DurationUnit`.
const SUFFIXES: [&str; 2] = ["_SECS", "_MS"];

#[test]
fn every_duration_is_read_through_its_bounds() {
    let root = repo_root();
    let readers = config_service_readers(&root);
    assert!(
        readers.contains("parse") && readers.contains("setting"),
        "the readers are derived from `ConfigService`'s own impl: {readers:?}"
    );
    let mut found = Durations::new(&readers, "");
    for dir in crate_dirs() {
        for path in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&path) else { continue };
            found.file = relative(&path, &root);
            found.visit_file(&ast);
        }
    }
    baseline::floor(found.bounds, FLOOR, "`DurationBounds` declaration(s)");
    assert!(
        found.wrong.is_empty(),
        "{} duration(s) read around `DurationBounds` — with no ceiling, or a `0` read in a way \
         of its own. Declare a `DurationBounds` beside the config and read it with \
         `.read(env, base)` / `.read_optional(env, base)`; a ceiling a deployment may lift is \
         `Floor::UnitsOrOff`:\n  {}",
        found.wrong.len(),
        found.wrong.into_iter().collect::<Vec<_>>().join("\n  "),
    );
}

/// The methods of `ConfigService`'s `impl` taking a `key` — the readers a
/// duration could be read through — from `nest-rs-config`'s source.
fn config_service_readers(root: &std::path::Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for path in rust_files(&root.join("crates/nest-rs-config/src")) {
        let Some(ast) = parsed(&path) else { continue };
        for item in &ast.items {
            let syn::Item::Impl(block) = item else {
                continue;
            };
            if block.trait_.is_some()
                || block.self_ty.to_token_stream().to_string() != "ConfigService"
            {
                continue;
            }
            for member in &block.items {
                let syn::ImplItem::Fn(method) = member else {
                    continue;
                };
                let takes_a_key = method.sig.inputs.iter().any(|input| {
                    matches!(input, syn::FnArg::Typed(arg)
                        if matches!(&*arg.pat, syn::Pat::Ident(name) if name.ident == "key"))
                });
                if takes_a_key && matches!(method.vis, syn::Visibility::Public(_)) {
                    out.insert(method.sig.ident.to_string());
                }
            }
        }
    }
    out
}

/// Every duration read around its bounds, and every declaration counted.
struct Durations<'r> {
    file: String,
    readers: &'r BTreeSet<String>,
    bounds: usize,
    wrong: BTreeSet<String>,
}

impl<'r> Durations<'r> {
    /// A walk refusing a read through any of `readers`.
    fn new(readers: &'r BTreeSet<String>, file: &str) -> Self {
        Self {
            file: file.to_owned(),
            readers,
            bounds: 0,
            wrong: BTreeSet::new(),
        }
    }

    /// The duration keys `tokens` spell as string literals, at any depth.
    fn keys(tokens: TokenStream) -> Vec<String> {
        let mut flat = Vec::new();
        flatten(tokens, &mut flat);
        flat.iter()
            .filter_map(|tree| match tree {
                TokenTree::Literal(lit) => syn::parse_str::<syn::LitStr>(&lit.to_string()).ok(),
                _ => None,
            })
            .map(|lit| lit.value())
            .filter(|value| SUFFIXES.iter().any(|suffix| value.ends_with(suffix)))
            .collect()
    }

    fn refuse(&mut self, how: &str, key: &str) {
        self.wrong
            .insert(format!("{}: `{key}` read through {how}", self.file));
    }

    /// The reads a `macro_rules!` transcriber writes: a reader's name followed by
    /// its argument group, read by tokens since nothing parses a transcriber.
    fn transcriber(&mut self, tokens: TokenStream) {
        let mut flat = Vec::new();
        flatten(tokens, &mut flat);
        for (at, tree) in flat.iter().enumerate() {
            let TokenTree::Ident(ident) = tree else {
                continue;
            };
            let name = ident.to_string();
            if !(self.readers.contains(&name) || name == "env_var") {
                continue;
            }
            if let Some(TokenTree::Group(group)) = flat.get(at + 1) {
                for key in Self::keys(group.stream()) {
                    self.refuse(&format!("`{name}` in a `macro_rules!`"), &key);
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Durations<'_> {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let method = call.method.to_string();
        if self.readers.contains(&method) {
            for key in Self::keys(call.args.to_token_stream()) {
                self.refuse(&format!("`.{method}(..)`"), &key);
            }
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*call.func
            && path
                .path
                .segments
                .last()
                .is_some_and(|last| last.ident == "env_var")
        {
            for key in Self::keys(call.args.to_token_stream()) {
                self.refuse("`env_var`", &key);
            }
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_struct(&mut self, literal: &'ast syn::ExprStruct) {
        let declares = literal
            .path
            .segments
            .last()
            .is_some_and(|last| last.ident == "DurationBounds");
        if declares {
            self.bounds += 1;
            let field = |name: &str| {
                literal.fields.iter().find_map(|field| match &field.member {
                    syn::Member::Named(ident) if ident == name => Some(field.expr.clone()),
                    _ => None,
                })
            };
            let key = field("key").and_then(|expr| match expr {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(key),
                    ..
                }) => Some(key.value()),
                _ => None,
            });
            let unit = field("unit").map(|expr| expr.to_token_stream().to_string());
            if let (Some(key), Some(unit)) = (key, unit) {
                let suffix = if unit.ends_with("Millis") {
                    "_MS"
                } else {
                    "_SECS"
                };
                if !key.ends_with(suffix) {
                    self.wrong.insert(format!(
                        "{}: `{key}` is read in {unit}, so its key ends in `{suffix}`",
                        self.file
                    ));
                }
            }
        }
        syn::visit::visit_expr_struct(self, literal);
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if is_cfg_test(&item.attrs) {
            return;
        }
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = &*item.expr
            && SUFFIXES
                .iter()
                .any(|suffix| value.value().ends_with(suffix))
        {
            self.refuse(
                &format!("a `const {}` a reader may be handed", item.ident),
                &value.value(),
            );
        }
        syn::visit::visit_item_const(self, item);
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if item.mac.path.is_ident("macro_rules") && !is_cfg_test(&item.attrs) {
            self.transcriber(item.mac.tokens.clone());
        }
        syn::visit::visit_item_macro(self, item);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !is_cfg_test(&node.attrs) {
            syn::visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if !is_cfg_test(&node.attrs) {
            syn::visit::visit_item_fn(self, node);
        }
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if !is_cfg_test(&node.attrs) {
            syn::visit::visit_impl_item_fn(self, node);
        }
    }
}

/// The join is proved on the shapes it exists to catch — the metric interval's
/// `env.parse::<u64>(…)` as it stood, a read through `env.get`, a key handed
/// through a `const`, a read a `macro_rules!` writes and one through `env_var`,
/// and a key whose suffix contradicts its unit — and on what it passes: a
/// `DurationBounds` read, and a variable named in a sentence.
#[test]
fn the_join_sees_a_duration_read_around_its_bounds() {
    let readers = BTreeSet::from(["parse".to_owned(), "get".to_owned(), "setting".to_owned()]);
    let caught: syn::File = syn::parse_quote! {
        fn from_env(env: &ConfigService) {
            let interval = env.parse::<u64>("METRIC_INTERVAL_SECS");
            let timeout = env.get("REQUEST_TIMEOUT_SECS");
            let raw = nest_rs_config::env_var(&var_name("x", "POLL_MS"));
        }
        const KEY: &str = "WINDOW_SECS";
        macro_rules! read { ($env:expr) => { $env.setting("LEASE_SECS") }; }
        const WRONG: DurationBounds = DurationBounds {
            key: "DEADLINE_SECS",
            field: "X::deadline",
            unit: DurationUnit::Millis,
            least: Floor::Units(Bound { count: 1, why: "w" }),
            most: Bound { count: 2, why: "w" },
        };
    };
    let passed: syn::File = syn::parse_quote! {
        const RIGHT: DurationBounds = DurationBounds {
            key: "DEADLINE_MS",
            field: "X::deadline",
            unit: DurationUnit::Millis,
            least: Floor::Units(Bound { count: 1, why: "w" }),
            most: Bound { count: 2, why: "w" },
        };
        fn from_env(env: &ConfigService) {
            let deadline = RIGHT.read(env, Duration::from_millis(5));
            let message = format!("set {}", var_name("x", "DEADLINE_MS"));
        }
        #[cfg(test)]
        mod tests { fn t(env: &ConfigService) { env.parse::<u64>("FIXTURE_SECS"); } }
    };
    let judge = |file: &syn::File| {
        let mut found = Durations::new(&readers, "fixture.rs");
        found.visit_file(file);
        (found.bounds, found.wrong.into_iter().collect::<Vec<_>>())
    };
    let (bounds, wrong) = judge(&caught);
    assert_eq!(bounds, 1);
    assert_eq!(
        wrong,
        [
            "fixture.rs: `DEADLINE_SECS` is read in DurationUnit :: Millis, so its key ends in `_MS`",
            "fixture.rs: `LEASE_SECS` read through `setting` in a `macro_rules!`",
            "fixture.rs: `METRIC_INTERVAL_SECS` read through `.parse(..)`",
            "fixture.rs: `POLL_MS` read through `env_var`",
            "fixture.rs: `REQUEST_TIMEOUT_SECS` read through `.get(..)`",
            "fixture.rs: `WINDOW_SECS` read through a `const KEY` a reader may be handed",
        ],
    );
    let (bounds, wrong) = judge(&passed);
    assert_eq!(bounds, 1);
    assert!(wrong.is_empty(), "{wrong:?}");
}
