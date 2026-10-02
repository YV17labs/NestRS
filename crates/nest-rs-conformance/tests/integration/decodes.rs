//! The decodes join — two families the rule *A decode failure is said without its
//! value* (`CLAUDE.md`) has, each derived from the source rather than listed.
//!
//! **Every `#[config]`'s `from_env`**, against the rule that a structured value
//! is decoded by `ConfigService::json` and never by the config itself. A
//! `#[config]` value is a payload too, and a structured one is where a deployment
//! writes client secrets. serde's own sentence quotes the value it refused, so a
//! `from_env` that calls `serde_json::from_str` and hands the error to
//! `Setting::refuse` — the demo's `IssuerConfig` did, for its OAuth client list —
//! prints that list's secrets into the boot error. `ConfigService::json` decodes
//! once, in `nest-rs-config`, and refuses through `nest_rs_core::DecodeError`.
//! Members: every `impl Config for …` under `crates/` and `demo/`.
//!
//! **What it reads**: the whole file an `impl Config` sits in, outside
//! `#[cfg(test)]` — a `from_env` decodes through a helper beside it as readily
//! as in its own body — and every path in it read through the crate's imports
//! (`nest_rs_conformance::imports`), so `use serde_json as json`, `use
//! serde_json::from_str` and a re-export the crate root makes are all
//! `serde_json`. A file declaring a `#[config]` names `serde_json` nowhere: none
//! does today, and the rule costs nothing to keep.
//!
//! **What it reads by its spelling** is `Config`, the trait an `impl` makes a
//! member with, and the `blinds` join refuses it renamed, aliased or written by a
//! `macro_rules!`. `serde_json` is read through the resolver, so the one import
//! that hides it — a glob of its items — is refused there too, with the two
//! constructions the resolver cannot read: an `extern crate … as` and a
//! `#[path]` module.
//!
//! **What it cannot read**: a helper in *another* file of the crate that a
//! `from_env` calls. Following it needs the crate's call graph, which a
//! syntactic read does not have; it is the one shape left, named here rather
//! than implied.
//!
//! **Every framework item taking a developer's error into a box** — a bound
//! `Into<Box<dyn …>>`, on the item or on the `impl` it sits in — against the rule
//! that it boxes through `nest_rs_core::boxed_error`. anyhow's own conversion into
//! a box hides the error it holds from every `downcast` and from `error_message`,
//! so a decode failure inside an `anyhow::Error` — the documented handler shape —
//! reached the queue's dead-letter line, HTTP's `.opaque()` line and the WS frame
//! in serde's words. A member passes when it names `boxed_error`, or when it
//! converts nothing itself (`.into()`, `Into::into`, `From::from`, `Box::from`)
//! and only forwards the value to a member that does. One shape converts and
//! passes, on an argument rather than a list: the lower tier of an autoref
//! specialisation, a box without `Send + Sync` on a type that also carries a
//! `Send + Sync` member — anyhow converts into both, but an `anyhow::Error` meets
//! the upper tier's bound and never reaches the lower one (WS's `ErrorReport`).
//!
//! Members are read under `crates/*/src/`, and the bound by its shape — the last
//! path segment `Into`, a `Box` of a trait object — so an alias of the error
//! trait hides nothing, and neither does a `type X = Box<dyn …>` named in the
//! bound: the walk resolves those by name once every file is read. An import
//! renaming `Into`, `Box` or `boxed_error` would hide a member, so the join
//! refuses the import itself. The known looseness is the name an alias is
//! resolved by — its last segment, across every crate — which can only add a
//! member, never hide one.

use std::collections::{BTreeMap, BTreeSet};

use crate::Followed;
use nest_rs_conformance::baseline;
use nest_rs_conformance::imports::{CrateImports, module_of, spelled_paths};
use nest_rs_conformance::sources::{
    crate_dirs, each_source, is_cfg_test, parsed, relative, repo_root, rust_files,
};
use proc_macro2::{TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    vec![
        Followed::implemented("Config"),
        Followed::krate("serde_json"),
        Followed::implemented("Into"),
        Followed::call("boxed_error"),
    ]
}

/// `impl Config for` blocks the walk must find. The framework and the demo hold
/// some two dozen; below this the scan is reading the wrong tree.
const FLOOR: usize = 15;

/// Items boxing a developer's error the walk must find. The framework holds
/// eleven: the four `Opaque` impls, the queue's two `JobError` constructors, the
/// schedule's `OccurrenceLockError::new`, WS's `from_handler_error` and the two
/// boxing tiers of `ErrorReport`, and `boxed_error` itself. Below this the scan
/// is reading the wrong tree.
const INTAKE_FLOOR: usize = 9;

#[test]
fn no_config_decodes_a_structured_value_itself() {
    let root = repo_root();
    let mut impls = 0usize;
    let mut wrong: BTreeSet<String> = BTreeSet::new();
    for dir in crate_dirs() {
        for entry in ["src/lib.rs", "src/main.rs"] {
            let entry = dir.join(entry);
            if !entry.is_file() {
                continue;
            }
            let imports = CrateImports::read(&entry, &root);
            let src = dir.join("src");
            for file in rust_files(&src) {
                let Some(module) = module_of(&file, &src) else {
                    continue;
                };
                // The other crate root reads its own files, and a file no
                // `mod` declares is no part of this crate.
                if (module.is_empty() && file != entry) || !imports.has_module(&module) {
                    continue;
                }
                let Some(ast) = parsed(&file) else { continue };
                let (found, decoding) = decodes_in(&ast, &imports, &module);
                impls += found;
                if decoding {
                    wrong.insert(relative(&file, &root));
                }
            }
        }
    }

    baseline::floor(impls, FLOOR, "`impl Config for` block(s)");
    assert!(
        wrong.is_empty(),
        "{} file(s) declare a `#[config]` and name `serde_json` — its own name, an alias, \
         or an import of one of its items — so a value that does not decode is refused in \
         serde's words, which quote it. Read it with `env.json::<T>(\"KEY\")?` instead:\n  {}",
        wrong.len(),
        wrong.into_iter().collect::<Vec<_>>().join("\n  "),
    );
}

/// How many `impl Config for` blocks `ast` holds, and whether — when it holds
/// any — a path in it outside `#[cfg(test)]` resolves to `serde_json`.
fn decodes_in(ast: &syn::File, imports: &CrateImports, module: &[String]) -> (usize, bool) {
    let mut found = ConfigImpls::default();
    found.visit_file(ast);
    if found.impls == 0 {
        return (0, false);
    }
    let tokens: TokenStream = ast
        .items
        .iter()
        .filter(|item| !item_is_cfg_test(item))
        .map(ToTokens::to_token_stream)
        .collect();
    let decoding = spelled_paths(tokens).iter().any(|path| {
        let resolved = match path.split_first() {
            Some((first, rest)) if first.is_empty() => rest.to_vec(),
            _ => imports.resolve(module, path),
        };
        resolved.first().is_some_and(|root| root == "serde_json")
    });
    (found.impls, decoding)
}

/// The `impl Config for` blocks of one file.
#[derive(Default)]
struct ConfigImpls {
    impls: usize,
}

impl<'ast> Visit<'ast> for ConfigImpls {
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let is_config = item
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|last| last.ident == "Config");
        if is_config && !is_cfg_test(&item.attrs) {
            self.impls += 1;
        }
        syn::visit::visit_item_impl(self, item);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !is_cfg_test(&node.attrs) {
            syn::visit::visit_item_mod(self, node);
        }
    }
}

/// A top-level item behind `#[cfg(test)]`.
fn item_is_cfg_test(item: &syn::Item) -> bool {
    match item {
        syn::Item::Mod(i) => is_cfg_test(&i.attrs),
        syn::Item::Fn(i) => is_cfg_test(&i.attrs),
        syn::Item::Impl(i) => is_cfg_test(&i.attrs),
        syn::Item::Use(i) => is_cfg_test(&i.attrs),
        syn::Item::Const(i) => is_cfg_test(&i.attrs),
        syn::Item::Static(i) => is_cfg_test(&i.attrs),
        syn::Item::Struct(i) => is_cfg_test(&i.attrs),
        _ => false,
    }
}

/// The join is proved on the shapes it exists to catch: the demo's
/// `IssuerConfig` as it stood, the same body through an alias, through an
/// imported function and through a helper beside it — and the body reading
/// through `env.json`, which passes.
#[test]
fn the_join_sees_a_config_decoding_its_own_value_however_it_names_serde_json() {
    let caught: [syn::File; 4] = [
        syn::parse_quote! {
            impl Config for IssuerConfig {
                fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                    let clients = match env.setting("CLIENTS")? {
                        Some(raw) => serde_json::from_str(&raw.value).map_err(|e| raw.refuse(e))?,
                        None => base.clients,
                    };
                    Ok(Self { clients })
                }
            }
        },
        syn::parse_quote! {
            use serde_json as json;
            impl Config for IssuerConfig {
                fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                    let raw = env.setting("CLIENTS")?.unwrap();
                    Ok(Self { clients: json::from_str(&raw.value).map_err(|e| raw.refuse(e))? })
                }
            }
        },
        syn::parse_quote! {
            use serde_json::from_str;
            impl Config for IssuerConfig {
                fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                    let raw = env.setting("CLIENTS")?.unwrap();
                    Ok(Self { clients: from_str(&raw.value).map_err(|e| raw.refuse(e))? })
                }
            }
        },
        syn::parse_quote! {
            fn decode(raw: &str) -> Result<Vec<Client>, ::serde_json::Error> {
                ::serde_json::from_str(raw)
            }
            impl Config for IssuerConfig {
                fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                    let raw = env.setting("CLIENTS")?.unwrap();
                    Ok(Self { clients: decode(&raw.value).map_err(|e| raw.refuse(e))? })
                }
            }
        },
    ];
    for file in &caught {
        let imports = CrateImports::of_items(&file.items);
        assert_eq!(
            decodes_in(file, &imports, &[]),
            (1, true),
            "{}",
            file.to_token_stream()
        );
    }
    // A re-export at the crate root, imported plainly into the file: the
    // crate's imports read through it.
    let lib: syn::File = syn::parse_quote! {
        pub(crate) use serde_json as wire;
        mod config;
    };
    let config: syn::File = syn::parse_quote! {
        use crate::wire::from_str;
        impl Config for IssuerConfig {
            fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                Ok(Self { clients: from_str("[]").unwrap_or(base.clients) })
            }
        }
    };
    let mut crate_items = lib.items.clone();
    crate_items.push(syn::parse_quote! { mod config { use crate::wire::from_str; } });
    let imports = CrateImports::of_items(&crate_items);
    assert_eq!(
        decodes_in(&config, &imports, &["config".to_owned()]),
        (1, true)
    );

    let passed: syn::File = syn::parse_quote! {
        impl nest_rs::config::Config for IssuerConfig {
            fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                Ok(Self { clients: env.json("CLIENTS")?.unwrap_or(base.clients) })
            }
        }
        #[cfg(test)]
        mod tests { fn fixture() { let _ = serde_json::json!({}); } }
    };
    let imports = CrateImports::of_items(&passed.items);
    assert_eq!(decodes_in(&passed, &imports, &[]), (1, false));
}

#[test]
fn every_framework_item_boxing_a_developer_s_error_goes_through_boxed_error() {
    let root = repo_root();
    let mut found = Intakes::default();
    each_source(&root, |rel, ast| {
        if rel.starts_with("crates/") && rel.contains("/src/") {
            found.file = rel.to_owned();
            found.visit_file(ast);
        }
    });

    let (members, mut wrong) = found.judged();
    wrong.extend(found.renamed);
    baseline::floor(members, INTAKE_FLOOR, "item(s) boxing a developer's error");
    assert!(
        wrong.is_empty(),
        "{} item(s) box a developer's error by its own conversion (`.into()`, \
         `From::from`, `Box::from`), which for an `anyhow::Error` is anyhow's own box — \
         the error inside it is no link of the chain, so a decode failure there is \
         rendered in serde's words. Box it with `nest_rs_core::boxed_error(error)` \
         instead, and import `Into`, `Box` and `boxed_error` under their own names:\n  {}",
        wrong.len(),
        wrong.join("\n  "),
    );
}

/// What one bound converts a value into: a boxed trait object, read by shape, or
/// a type named by path, which is a member when it names a `Box<dyn …>` alias.
enum Target {
    /// `Box<dyn …>`, and whether its object carries `Send` and `Sync`.
    Boxed(bool),
    /// The last segment of a path type — resolved once every file is read.
    Named(String),
}

/// One item carrying an `Into<…>` bound, before its targets are resolved.
struct Bounded {
    label: String,
    /// The type the enclosing `impl` is for, qualified by its file.
    owner: Option<String>,
    targets: Vec<Target>,
    /// Converts a value itself and never names `boxed_error`.
    converts: bool,
}

/// Every item bounded by `Into<…>` under `crates/*/src/`, the `Box<dyn …>`
/// aliases those bounds may name, and every import renaming what the join reads.
#[derive(Default)]
struct Intakes {
    file: String,
    owner: Option<String>,
    bounded: Vec<Bounded>,
    /// `type X = Box<dyn …>`: `X`, and whether its object is `Send + Sync`.
    aliases: BTreeMap<String, bool>,
    renamed: Vec<String>,
}

impl Intakes {
    /// How many members the walk found, and those converting by their own
    /// conversion. A member's box is `Send + Sync` when any of its bounds is.
    ///
    /// One shape converts by `.into()` and passes: a bound whose box is **not**
    /// `Send + Sync`, on a type that also carries a `Send + Sync` member — the
    /// tiers of an autoref specialisation (WS's `ErrorReport`). anyhow converts
    /// into both boxes, but an `anyhow::Error` is `Send + Sync + 'static`, so it
    /// always meets the upper tier's bound and never reaches the lower one.
    fn judged(&self) -> (usize, Vec<String>) {
        let members: Vec<(&Bounded, bool)> = self
            .bounded
            .iter()
            .filter_map(|item| {
                item.targets
                    .iter()
                    .filter_map(|target| match target {
                        Target::Boxed(send_sync) => Some(*send_sync),
                        Target::Named(name) => self.aliases.get(name).copied(),
                    })
                    .reduce(|left, right| left || right)
                    .map(|send_sync| (item, send_sync))
            })
            .collect();
        let upper_tier = |owner: &Option<String>| {
            owner.is_some()
                && members
                    .iter()
                    .any(|(other, send_sync)| *send_sync && other.owner == *owner)
        };
        let wrong = members
            .iter()
            .filter(|(item, send_sync)| item.converts && (*send_sync || !upper_tier(&item.owner)))
            .map(|(item, _)| item.label.clone())
            .collect();
        (members.len(), wrong)
    }

    fn record(&mut self, label: String, targets: Vec<Target>, tokens: TokenStream, converts: bool) {
        if targets.is_empty() {
            return;
        }
        self.bounded.push(Bounded {
            label: format!("{}: {label}", self.file),
            owner: self.owner.clone(),
            targets,
            converts: converts && !names_ident(tokens, "boxed_error"),
        });
    }
}

impl<'ast> Visit<'ast> for Intakes {
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let self_ty = match &*item.self_ty {
            syn::Type::Path(path) => path.path.segments.last().map(|last| last.ident.to_string()),
            _ => None,
        };
        let enclosing = std::mem::replace(
            &mut self.owner,
            self_ty.map(|ty| format!("{}::{ty}", self.file)),
        );
        let mut bounds = IntoBounds::default();
        bounds.visit_generics(&item.generics);
        if !bounds.0.is_empty() {
            let label = match &item.trait_ {
                Some((path, _)) => format!(
                    "impl {} for {}",
                    path.to_token_stream(),
                    item.self_ty.to_token_stream()
                ),
                None => format!("impl {}", item.self_ty.to_token_stream()),
            };
            let mut converts = Conversions::default();
            converts.visit_item_impl(item);
            self.record(label, bounds.0, item.to_token_stream(), converts.0);
        }
        syn::visit::visit_item_impl(self, item);
        self.owner = enclosing;
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let mut bounds = IntoBounds::default();
        bounds.visit_signature(&item.sig);
        let mut converts = Conversions::default();
        converts.visit_block(&item.block);
        self.record(
            format!("fn {}", item.sig.ident),
            bounds.0,
            item.to_token_stream(),
            converts.0,
        );
        syn::visit::visit_impl_item_fn(self, item);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let enclosing = self.owner.take();
        let mut bounds = IntoBounds::default();
        bounds.visit_signature(&item.sig);
        let mut converts = Conversions::default();
        converts.visit_block(&item.block);
        self.record(
            format!("fn {}", item.sig.ident),
            bounds.0,
            item.to_token_stream(),
            converts.0,
        );
        syn::visit::visit_item_fn(self, item);
        self.owner = enclosing;
    }

    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        // Two aliases sharing a name resolve as `Send + Sync` if either is: the
        // stricter reading, under which a collision can add a member's failure
        // and never excuse one.
        if let Some(send_sync) = boxed_object(&item.ty) {
            *self.aliases.entry(item.ident.to_string()).or_default() |= send_sync;
        }
        syn::visit::visit_item_type(self, item);
    }

    fn visit_use_rename(&mut self, rename: &'ast syn::UseRename) {
        if ["Into", "Box", "boxed_error"]
            .iter()
            .any(|name| rename.ident == name)
        {
            self.renamed.push(format!(
                "{}: use … {} as {}",
                self.file, rename.ident, rename.rename
            ));
        }
    }
}

/// The targets of every `Into<…>` bound on the visited node — in a `where`
/// clause, on a generic, or as an `impl Into<…>` argument.
#[derive(Default)]
struct IntoBounds(Vec<Target>);

impl<'ast> Visit<'ast> for IntoBounds {
    fn visit_trait_bound(&mut self, bound: &'ast syn::TraitBound) {
        if let Some(into) = bound
            .path
            .segments
            .last()
            .filter(|last| last.ident == "Into")
        {
            self.0
                .extend(generic_types(&into.arguments).filter_map(|ty| {
                    boxed_object(ty).map(Target::Boxed).or_else(|| match ty {
                        syn::Type::Path(path) => path
                            .path
                            .segments
                            .last()
                            .map(|last| Target::Named(last.ident.to_string())),
                        _ => None,
                    })
                }));
        }
        syn::visit::visit_trait_bound(self, bound);
    }
}

/// Whether the visited node converts a value by a conversion of its own:
/// `.into()`, `Into::into`, `From::from` or `Box::from`.
#[derive(Default)]
struct Conversions(bool);

impl<'ast> Visit<'ast> for Conversions {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.0 |= call.method == "into" && call.args.is_empty();
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        let mut segments = path.path.segments.iter().rev();
        self.0 |= matches!(
            (segments.next(), segments.next()),
            (Some(last), Some(owner))
                if (last.ident == "into" && owner.ident == "Into")
                    || (last.ident == "from" && (owner.ident == "From" || owner.ident == "Box"))
        );
        syn::visit::visit_expr_path(self, path);
    }
}

/// `Box<dyn …>`, read by shape: `Some(true)` when the object carries `Send` and
/// `Sync`, `Some(false)` when it does not, `None` for any other type.
fn boxed_object(ty: &syn::Type) -> Option<bool> {
    let syn::Type::Path(boxed) = ty else {
        return None;
    };
    let boxed = boxed.path.segments.last()?;
    if boxed.ident != "Box" {
        return None;
    }
    generic_types(&boxed.arguments).find_map(|object| {
        let syn::Type::TraitObject(object) = object else {
            return None;
        };
        let marker = |name: &str| {
            object.bounds.iter().any(|bound| match bound {
                syn::TypeParamBound::Trait(bound) => bound
                    .path
                    .segments
                    .last()
                    .is_some_and(|last| last.ident == name),
                _ => false,
            })
        };
        Some(marker("Send") && marker("Sync"))
    })
}

fn generic_types(arguments: &syn::PathArguments) -> impl Iterator<Item = &syn::Type> {
    let args = match arguments {
        syn::PathArguments::AngleBracketed(args) => Some(&args.args),
        _ => None,
    };
    args.into_iter().flatten().filter_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

fn names_ident(tokens: TokenStream, name: &str) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => ident == name,
        TokenTree::Group(group) => names_ident(group.stream(), name),
        _ => false,
    })
}

/// The walk of one fixture file, judged.
fn judged(file: &syn::File) -> (usize, Vec<String>, Vec<String>) {
    let mut found = Intakes {
        file: "fixture.rs".to_owned(),
        ..Intakes::default()
    };
    found.visit_file(file);
    let (members, wrong) = found.judged();
    (members, wrong, found.renamed)
}

/// The join is proved on the shapes it exists to catch — `Opaque` and
/// `JobError::retry` as they stood, a box named through an alias, a box without
/// `Send + Sync` standing alone, and a renamed import — and on the shapes it
/// must pass: the same bodies through `boxed_error`, a member that only
/// forwards its value, and the lower tier of a specialisation.
#[test]
fn the_join_sees_an_item_boxing_a_developer_s_error_by_its_own_conversion() {
    let caught: syn::File = syn::parse_quote! {
        use std::convert::Into as Convert;
        type BoxError = Box<dyn std::error::Error + Send + Sync>;
        impl<T, E> Opaque<T> for Result<T, E>
        where
            E: Into<Box<dyn std::error::Error + Send + Sync>>,
        {
            fn opaque(self) -> Result<T, Error> {
                self.map_err(|err| {
                    let err: Box<dyn std::error::Error + Send + Sync> = err.into();
                    Error::from(err)
                })
            }
        }
        impl JobError {
            pub fn retry(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
                Self { source: source.into() }
            }
            pub fn abort(source: impl Into<BoxError>) -> Self {
                Self { source: Into::into(source) }
            }
        }
        impl Local {
            pub fn new<E: Into<Box<dyn Error>>>(source: E) -> Self {
                Self(Box::from(source))
            }
        }
    };
    let passed: syn::File = syn::parse_quote! {
        impl<T, E> Opaque<T> for Result<T, E>
        where
            E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
        {
            fn opaque(self) -> Result<T, Error> {
                self.map_err(|err| Error::from(nest_rs_core::boxed_error(err)))
            }
        }
        impl JobError {
            pub fn retry(source: impl Into<Box<dyn Error + Send + Sync>> + 'static) -> Self {
                Self { source: nest_rs_core::boxed_error(source) }
            }
        }
        impl<E> ErrorReport<E>
        where
            E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
        {
            pub fn into_frame(self, event: &str) -> WsReply {
                WsReply::from_handler_error(event, self.0)
            }
        }
        impl<E: Into<Box<dyn std::error::Error>>> ErrorReportChain for ErrorReport<E> {
            fn into_frame(self, event: &str) -> WsReply {
                let error: Box<dyn std::error::Error> = self.0.into();
                WsReply::error(error.to_string())
            }
        }
        // Not a member: the bound converts into no box.
        fn label(name: impl Into<String>) -> String {
            name.into()
        }
    };
    let (members, wrong, renamed) = judged(&caught);
    assert_eq!(members, 4);
    assert_eq!(
        wrong,
        [
            "fixture.rs: impl Opaque < T > for Result < T , E >",
            "fixture.rs: fn retry",
            "fixture.rs: fn abort",
            "fixture.rs: fn new",
        ]
    );
    assert_eq!(renamed, ["fixture.rs: use … Into as Convert"]);
    let (members, wrong, renamed) = judged(&passed);
    assert_eq!(members, 4);
    assert!(wrong.is_empty(), "{wrong:?}");
    assert!(renamed.is_empty(), "{renamed:?}");
}
