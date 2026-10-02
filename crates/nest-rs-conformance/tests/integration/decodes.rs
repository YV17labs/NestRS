//! The decodes join — two families the rule *A decode failure is said without its
//! value* (`CLAUDE.md`) has, each derived from the source rather than listed.
//!
//! **Every `#[config]`'s reader**, against the rule that a structured value is
//! decoded by `ConfigService::json` and never by the config itself. A
//! `#[config]` value is a payload too, and a structured one is where a deployment
//! writes client secrets. serde's own sentence quotes the value it refused, so a
//! `from_env` that calls `serde_json::from_str` and hands the error to
//! `Setting::refuse` — the demo's OAuth config did, for its client list —
//! prints that list's secrets into the boot error. `ConfigService::json` decodes
//! once, in `nest-rs-config`, and refuses through `nest_rs_core::DecodeError`.
//! Members: every reader under `crates/` and `demo/`, and what it reaches.
//!
//! **What it reads**: every function a config *reader* reaches — a reader being
//! any function whose signature names `ConfigService`, every `from_env`
//! included — through what the source spells: a path in call position, read
//! through the crate's imports (`nest_rs_conformance::imports`) and matched by
//! its owner when it names a type of the crate (`Self::f`, `HttpTls::from_env`),
//! by its name alone when it roots at a name the imports do not explain (a
//! generic, a type of another crate); a method name, which any method of the
//! crate may answer; `.parse()`, which reaches every `from_str` of the crate by
//! inference; and a macro, whose transcriber is read as a body. Matching by name
//! can only add a function, never hide one. A reader is handed values — the
//! environment and the base it overlays — and never behaviour: one taking a
//! function, a trait object or a generic runs a body written at its caller,
//! which reaching the reader does not reach, so it is refused.
//! Every function so reached, and every macro, must name `serde_json` nowhere —
//! its own name, an alias, an item imported from it — and call into no other
//! crate of the workspace than the decoder's home, the one crate declaring
//! `ConfigService`, where `Setting::json` decodes once for every reader. A
//! helper beside the reader, one in another file of the crate, an extension
//! method, a `FromStr` and a macro are all read; a helper in another crate is
//! refused rather than followed, so the join needs no call graph across crates.
//! The decoder's home itself is the one crate not held to it.
//!
//! **What it reads by its spelling** is `Config`, the trait an `impl` is counted
//! by, and `ConfigService`, the type a reader is recognised by — and the `blinds`
//! join refuses either renamed, aliased or written by a `macro_rules!`.
//! `serde_json` is read through the resolver, so the one import that hides it —
//! a glob of its items — is refused there too, with the two constructions the
//! resolver cannot read: an `extern crate … as` and a `#[path]` module.
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
//! renaming `Into`, `Box` or `boxed_error` would hide a member, and a
//! `macro_rules!` writing an `Into<…>` bound would hide one from the walk: the
//! `blinds` join refuses all four, from this join's declaration. The known
//! looseness is the name an alias is resolved by — its last segment, across
//! every crate — which can only add a member, never hide one.

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
        Followed::type_("ConfigService"),
        Followed::krate("serde_json"),
        Followed::implemented("Into"),
        Followed::aliased("Box").through(&["boxed"]),
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
    let workspace: BTreeSet<String> = crate_dirs()
        .iter()
        .filter_map(|dir| dir.file_name())
        .map(|name| name.to_string_lossy().replace('-', "_"))
        .collect();
    let crates: Vec<Crate> = crate_dirs()
        .iter()
        .flat_map(|dir| Crate::read(dir, &root))
        .collect();
    let home: BTreeSet<String> = crates
        .iter()
        .filter(|krate| krate.declares_the_reader)
        .map(|krate| krate.name.clone())
        .collect();
    assert_eq!(
        home.len(),
        1,
        "exactly one crate declares `ConfigService`, the decoder's home: {home:?}"
    );
    let impls: usize = crates.iter().map(|krate| krate.config_impls).sum();
    let wrong: BTreeSet<String> = crates
        .iter()
        .filter(|krate| !krate.declares_the_reader)
        .flat_map(|krate| krate.decoding(&workspace, &home))
        .collect();

    baseline::floor(impls, FLOOR, "`impl Config for` block(s)");
    assert!(
        wrong.is_empty(),
        "{} function(s) a config reader reaches decode a value themselves — they name \
         `serde_json` (its own name, an alias, or an import of one of its items), so a value \
         that does not decode is refused in serde's words, which quote it — or call into \
         another crate of the workspace, which this join does not follow. Read a structured \
         value with `env.json::<T>(\"KEY\")?`, and keep what a reader calls in its own crate:\n  \
         {}",
        wrong.len(),
        wrong.into_iter().collect::<Vec<_>>().join("\n  "),
    );
}

/// One crate, read for the reader closure.
struct Crate {
    /// The crate's name as a path root.
    name: String,
    imports: CrateImports,
    functions: Vec<Function>,
    /// Every `macro_rules!` of the crate: its module, its name, its tokens.
    macros: Vec<(Vec<String>, String, TokenStream)>,
    /// `impl Config for` blocks outside `#[cfg(test)]`.
    config_impls: usize,
    /// Whether the crate declares `ConfigService` — the decoder's home, where
    /// `Setting::json` decodes once, for every reader.
    declares_the_reader: bool,
}

/// One function of a crate, outside `#[cfg(test)]`.
struct Function {
    module: Vec<String>,
    label: String,
    /// The type of the `impl` it sits in, by its last path segment.
    owner: Option<String>,
    name: String,
    /// Whether its signature names `ConfigService` — a reader.
    reader: bool,
    /// Whether it is handed behaviour — a function, a trait object, a generic —
    /// whose body the join cannot see from here.
    handed_behaviour: bool,
    body: syn::Block,
}

impl Crate {
    /// The crate at `dir`, once per root (`lib.rs`, `main.rs`).
    fn read(dir: &std::path::Path, root: &std::path::Path) -> Vec<Self> {
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().replace('-', "_"))
            .unwrap_or_default();
        let mut out = Vec::new();
        for entry in ["src/lib.rs", "src/main.rs"] {
            let entry = dir.join(entry);
            if !entry.is_file() {
                continue;
            }
            let imports = CrateImports::read(&entry, root);
            let src = dir.join("src");
            let mut walk = Walk::default();
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
                walk.file = relative(&file, root);
                walk.module = module;
                walk.visit_file(&ast);
            }
            out.push(Self {
                name: name.clone(),
                imports,
                functions: walk.functions,
                macros: walk.macros,
                config_impls: walk.config_impls,
                declares_the_reader: walk.declares_the_reader,
            });
        }
        out
    }

    /// Every function a reader of this crate reaches that decodes, or calls into
    /// another crate of `workspace` than the decoder's `home`.
    fn decoding(&self, workspace: &BTreeSet<String>, home: &BTreeSet<String>) -> Vec<String> {
        let mut reached: BTreeSet<usize> = (0..self.functions.len())
            .filter(|&at| self.functions[at].reader)
            .collect();
        let mut macros: BTreeSet<usize> = BTreeSet::new();
        let mut queue: Vec<usize> = reached.iter().copied().collect();
        while let Some(at) = queue.pop() {
            let function = &self.functions[at];
            let mut calls = Calls::default();
            calls.visit_block(&function.body);
            for next in self.callees(function, &calls) {
                if reached.insert(next) {
                    queue.push(next);
                }
            }
            for (index, (_, name, tokens)) in self.macros.iter().enumerate() {
                if calls.macros.contains(name) && macros.insert(index) {
                    // A transcriber's calls are read by name: every identifier
                    // it writes that names a function of the crate is one.
                    let written = nest_rs_conformance::sources::idents(tokens.clone());
                    for (next, other) in self.functions.iter().enumerate() {
                        if written.contains(&other.name) && reached.insert(next) {
                            queue.push(next);
                        }
                    }
                }
            }
        }
        let mut wrong = Vec::new();
        for &at in &reached {
            let function = &self.functions[at];
            if function.reader && function.handed_behaviour {
                wrong.push(format!(
                    "{} is a reader handed behaviour — a function, a trait object or a \
                     generic — whose body is written where this join does not read",
                    function.label
                ));
            }
            let tokens = function.body.to_token_stream();
            if self
                .resolved(&function.module, tokens)
                .iter()
                .any(|path| path.first().is_some_and(|root| root == "serde_json"))
            {
                wrong.push(format!("{} names `serde_json`", function.label));
            }
            let mut calls = Calls::default();
            calls.visit_block(&function.body);
            for path in &calls.paths {
                let path = match path.split_first() {
                    Some((first, rest)) if first.is_empty() => rest.to_vec(),
                    _ => self.imports.resolve(&function.module, path),
                };
                if let Some(root) = path.first()
                    && *root != self.name
                    && (workspace.contains(root) || root == "nest_rs")
                    && !reaches_home(&path, home)
                {
                    wrong.push(format!(
                        "{} calls `{}`, in another crate of the workspace",
                        function.label,
                        path.join("::")
                    ));
                }
            }
        }
        for &index in &macros {
            let (module, name, tokens) = &self.macros[index];
            if self
                .resolved(module, tokens.clone())
                .iter()
                .any(|path| path.first().is_some_and(|root| root == "serde_json"))
            {
                wrong.push(format!(
                    "`{name}!`, a macro a reader invokes, names `serde_json`"
                ));
            }
        }
        wrong.sort();
        wrong.dedup();
        wrong
    }

    /// The functions `function` may call, by what its body spells: a path —
    /// read through the crate's imports, its owner matched when it names a
    /// type — a method name, which any method of the crate may answer, and
    /// `.parse()`, which reaches every `from_str` of the crate by inference. By
    /// name, so a collision adds a function and never hides one.
    fn callees(&self, function: &Function, calls: &Calls) -> Vec<usize> {
        let mut out = Vec::new();
        for path in &calls.paths {
            let resolved = match path.split_first() {
                Some((first, rest)) if first.is_empty() => rest.to_vec(),
                _ => self.imports.resolve(&function.module, path),
            };
            let Some(name) = resolved.last() else {
                continue;
            };
            // Whose function it is: the crate's, by the owner the path names
            // (`Self::f`, `HttpTls::from_env`); anyone's, when the path roots at
            // a name the imports do not explain — a generic (`C::from_env`) or
            // a type of another crate, so any method of the crate of that name
            // may answer; a free function's, when it has one segment.
            let owner = match resolved.first().map(String::as_str) {
                _ if path.first().is_some_and(|first| first == "Self") => {
                    Owner::Named(function.owner.clone())
                }
                Some("crate") => Owner::Named(
                    resolved
                        .len()
                        .checked_sub(2)
                        .map(|at| resolved[at].clone())
                        .filter(|segment| segment.starts_with(char::is_uppercase)),
                ),
                _ if resolved.len() == 1 => Owner::Named(None),
                _ if resolved.len() == 2 => Owner::Any,
                _ => continue,
            };
            out.extend(
                self.functions
                    .iter()
                    .enumerate()
                    .filter(|(_, other)| {
                        &other.name == name
                            && match &owner {
                                Owner::Named(owner) => &other.owner == owner,
                                Owner::Any => other.owner.is_some(),
                            }
                    })
                    .map(|(at, _)| at),
            );
        }
        for method in &calls.methods {
            let wanted = if method == "parse" {
                "from_str"
            } else {
                method
            };
            out.extend(
                self.functions
                    .iter()
                    .enumerate()
                    .filter(|(_, other)| other.owner.is_some() && other.name == wanted)
                    .map(|(at, _)| at),
            );
        }
        out
    }

    /// Every path `tokens` spell, read through the crate's imports from
    /// `module` — a leading `::` dropped, so `::serde_json` is `serde_json`.
    fn resolved(&self, module: &[String], tokens: TokenStream) -> Vec<Vec<String>> {
        spelled_paths(tokens)
            .iter()
            .map(|path| match path.split_first() {
                Some((first, rest)) if first.is_empty() => rest.to_vec(),
                _ => self.imports.resolve(module, path),
            })
            .collect()
    }
}

/// Whether `sig` takes behaviour rather than values: a type parameter, an
/// `impl Trait` or a `dyn Trait`, a function pointer or an `Fn*` bound. A reader
/// takes the environment and the base it overlays; behaviour handed to it runs a
/// body written at the caller, which reaching the reader does not reach.
fn hands_behaviour(sig: &syn::Signature) -> bool {
    let types = sig.generics.type_params().next().is_some();
    let inputs = sig.inputs.to_token_stream();
    let written = nest_rs_conformance::sources::idents(inputs.clone());
    let mut flat = Vec::new();
    nest_rs_conformance::sources::flatten(inputs, &mut flat);
    let bare_fn = flat
        .iter()
        .any(|tree| matches!(tree, TokenTree::Ident(ident) if ident == "fn"));
    types
        || bare_fn
        || ["impl", "dyn", "Fn", "FnMut", "FnOnce"]
            .iter()
            .any(|word| written.contains(*word))
}

/// Whose functions a path in call position may name.
enum Owner {
    /// A free function's (`None`), or a type's by its name.
    Named(Option<String>),
    /// Any type's — a path rooted at a name the imports do not explain.
    Any,
}

/// Whether `path`, rooted at a crate of the workspace, is the decoder's home —
/// by its own name, or through the umbrella's `config` re-export.
fn reaches_home(path: &[String], home: &BTreeSet<String>) -> bool {
    match path {
        [root, ..] if home.contains(root) => true,
        [umbrella, concern, ..] => {
            umbrella == "nest_rs"
                && home
                    .iter()
                    .any(|home| home.strip_prefix("nest_rs_") == Some(concern.as_str()))
        }
        _ => false,
    }
}

/// The functions, macros and `impl Config` blocks of a crate's files.
#[derive(Default)]
struct Walk {
    file: String,
    module: Vec<String>,
    owner: Option<String>,
    functions: Vec<Function>,
    macros: Vec<(Vec<String>, String, TokenStream)>,
    config_impls: usize,
    declares_the_reader: bool,
}

impl Walk {
    fn record(&mut self, sig: &syn::Signature, body: &syn::Block) {
        let name = sig.ident.to_string();
        let owner = self.owner.clone();
        self.functions.push(Function {
            module: self.module.clone(),
            label: format!(
                "{}: fn {}{name}",
                self.file,
                owner
                    .as_deref()
                    .map(|owner| format!("{owner}::"))
                    .unwrap_or_default()
            ),
            owner,
            name,
            reader: nest_rs_conformance::sources::idents(sig.inputs.to_token_stream())
                .contains("ConfigService"),
            handed_behaviour: hands_behaviour(sig),
            body: body.clone(),
        });
    }
}

impl<'ast> Visit<'ast> for Walk {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_cfg_test(&item.attrs) {
            let enclosing = self.owner.take();
            self.record(&item.sig, &item.block);
            syn::visit::visit_item_fn(self, item);
            self.owner = enclosing;
        }
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if is_cfg_test(&item.attrs) {
            return;
        }
        let is_config = item
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|last| last.ident == "Config");
        self.config_impls += usize::from(is_config);
        let owner = match &*item.self_ty {
            syn::Type::Path(path) => path.path.segments.last().map(|last| last.ident.to_string()),
            _ => None,
        };
        let enclosing = std::mem::replace(&mut self.owner, owner);
        syn::visit::visit_item_impl(self, item);
        self.owner = enclosing;
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !is_cfg_test(&item.attrs) {
            self.record(&item.sig, &item.block);
            syn::visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        self.declares_the_reader |= item.ident == "ConfigService" && !is_cfg_test(&item.attrs);
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if let Some(name) = &item.ident
            && item.mac.path.is_ident("macro_rules")
            && !is_cfg_test(&item.attrs)
        {
            self.macros.push((
                self.module.clone(),
                name.to_string(),
                item.mac.tokens.clone(),
            ));
        }
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !is_cfg_test(&node.attrs) {
            syn::visit::visit_item_mod(self, node);
        }
    }
}

/// What a body calls, as it spells it: each path in call position, each
/// method name, each macro invoked.
#[derive(Default)]
struct Calls {
    paths: Vec<Vec<String>>,
    methods: BTreeSet<String>,
    macros: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Calls {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*call.func {
            let mut segments: Vec<String> = path
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect();
            if path.path.leading_colon.is_some() {
                segments.insert(0, String::new());
            }
            self.paths.push(segments);
        }
        syn::visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        self.methods.insert(call.method.to_string());
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Some(last) = mac.path.segments.last() {
            self.macros.insert(last.ident.to_string());
        }
        syn::visit::visit_macro(self, mac);
    }
}

/// The crate of `tree`, planted at a scratch root and read as the join reads
/// one — every file a `mod` of `src/lib.rs` declares.
fn planted(tag: &str, tree: &[(&str, &str)]) -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("decodes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, tree);
    let crates = Crate::read(&root.join("crates/probe"), &root);
    let workspace = BTreeSet::from(["probe".to_owned(), "nest_rs_core".to_owned()]);
    let home = BTreeSet::from(["nest_rs_config".to_owned()]);
    let wrong = crates
        .iter()
        .flat_map(|krate| krate.decoding(&workspace, &home))
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    wrong
}

/// The join is proved on the shapes it exists to catch: the demo's OAuth config
/// as it stood (then named `IssuerConfig`), the same body through an alias and through an
/// imported function — and the shapes a reader in one file reaches a decoder in
/// another: a helper, a method of a crate-local extension, a `FromStr` that
/// `.parse()` reaches by inference, a macro, and a helper in another crate. The
/// body reading through `env.json` passes, as do the decoder's own crate and a
/// fixture behind `#[cfg(test)]`.
#[test]
fn the_join_sees_a_config_decoding_its_own_value_wherever_the_decoder_sits() {
    const READER: &str = "pub struct IssuerConfig { clients: Vec<String> }\n";
    for (shape, config, other) in [
        (
            "serde_json in the body",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: serde_json::from_str(&raw.value).map_err(|e| raw.refuse(e))? }) } }",
            "",
        ),
        (
            "an alias of serde_json",
            "use serde_json as json;\n\
             impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: json::from_str(&raw.value).map_err(|e| raw.refuse(e))? }) } }",
            "",
        ),
        (
            "a helper in another file",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: crate::wire::decode(&raw.value).map_err(|e| raw.refuse(e))? }) } }",
            "pub fn decode(raw: &str) -> Result<Vec<String>, ::serde_json::Error> { ::serde_json::from_str(raw) }",
        ),
        (
            "a method of a crate-local extension",
            "use crate::wire::Decode;\n\
             impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: raw.value.decoded().map_err(|e| raw.refuse(e))? }) } }",
            "pub trait Decode { fn decoded(&self) -> Result<Vec<String>, serde_json::Error>; }\n\
             impl Decode for String { fn decoded(&self) -> Result<Vec<String>, serde_json::Error> { serde_json::from_str(self) } }",
        ),
        (
            "a FromStr .parse() reaches",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let clients: crate::wire::Clients = env.parse(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: clients.0 }) } }",
            "pub struct Clients(pub Vec<String>);\n\
             impl std::str::FromStr for Clients { type Err = serde_json::Error; \
               fn from_str(raw: &str) -> Result<Self, Self::Err> { serde_json::from_str(raw).map(Clients) } }",
        ),
        (
            "a macro",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: decode!(&raw.value).map_err(|e| raw.refuse(e))? }) } }",
            "#[macro_export] macro_rules! decode { ($raw:expr) => { serde_json::from_str($raw) }; }",
        ),
        (
            "a generic the reader is handed",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               Ok(Self { clients: decoded::<crate::wire::Json>(env)? }) } }\n\
             fn decoded<D: crate::wire::Decoder>(env: &ConfigService) -> Result<Vec<String>> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); D::decode(&raw.value).map_err(|e| raw.refuse(e)) }",
            "pub trait Decoder { fn decode(raw: &str) -> Result<Vec<String>, String>; }\n\
             pub struct Json;\n\
             impl Decoder for Json { fn decode(raw: &str) -> Result<Vec<String>, String> { \
               serde_json::from_str(raw).map_err(|e| e.to_string()) } }",
        ),
        (
            "a function the reader is handed",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               Ok(base) } }\n\
             pub fn overlay(env: &ConfigService, decode: impl Fn(&str) -> Vec<String>) -> Vec<String> { \
               decode(&env.get(\"CLIENTS\").unwrap().unwrap()) }",
            "",
        ),
        (
            "a helper in another crate",
            "impl Config for IssuerConfig { fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
               let raw = env.setting(\"CLIENTS\")?.unwrap(); \
               Ok(Self { clients: nest_rs_core::decode(&raw.value) }) } }",
            "",
        ),
    ] {
        let wrong = planted(
            "caught",
            &[
                ("crates/probe/src/lib.rs", "mod config;\nmod wire;\n"),
                ("crates/probe/src/config.rs", &format!("{READER}{config}\n")),
                ("crates/probe/src/wire.rs", other),
            ],
        );
        assert!(!wrong.is_empty(), "{shape}: the join must see it");
    }

    let passed = planted(
        "passed",
        &[
            ("crates/probe/src/lib.rs", "mod config;\nmod wire;\n"),
            (
                "crates/probe/src/config.rs",
                "pub struct IssuerConfig { clients: Vec<String> }\n\
                 impl nest_rs::config::Config for IssuerConfig { \
                   fn from_env(env: &ConfigService, base: Self) -> Result<Self> { \
                     Ok(Self { clients: env.json(\"CLIENTS\")?.unwrap_or(base.clients) }) } }\n\
                 #[cfg(test)]\nmod tests { fn fixture() { let _ = serde_json::json!({}); } }\n",
            ),
            (
                "crates/probe/src/wire.rs",
                "pub fn body(value: &str) -> serde_json::Value { serde_json::from_str(value).unwrap_or_default() }",
            ),
        ],
    );
    assert!(passed.is_empty(), "{passed:?}");
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

    let (members, wrong) = found.judged();
    baseline::floor(members, INTAKE_FLOOR, "item(s) boxing a developer's error");
    assert!(
        wrong.is_empty(),
        "{} item(s) box a developer's error by its own conversion (`.into()`, \
         `From::from`, `Box::from`), which for an `anyhow::Error` is anyhow's own box — \
         the error inside it is no link of the chain, so a decode failure there is \
         rendered in serde's words. Box it with `nest_rs_core::boxed_error(error)` \
         instead:\n  {}",
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

/// Every item bounded by `Into<…>` under `crates/*/src/`, and the `Box<dyn …>`
/// aliases those bounds may name.
#[derive(Default)]
struct Intakes {
    file: String,
    owner: Option<String>,
    bounded: Vec<Bounded>,
    /// `type X = Box<dyn …>`: `X`, and whether its object is `Send + Sync`.
    aliases: BTreeMap<String, bool>,
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
fn judged(file: &syn::File) -> (usize, Vec<String>) {
    let mut found = Intakes {
        file: "fixture.rs".to_owned(),
        ..Intakes::default()
    };
    found.visit_file(file);
    found.judged()
}

/// The join is proved on the shapes it exists to catch — `Opaque` and
/// `JobError::retry` as they stood, a box named through an alias, a box without
/// `Send + Sync` standing alone — and on the shapes it must pass: the same
/// bodies through `boxed_error`, a member that only forwards its value, and the
/// lower tier of a specialisation. A renamed `Into`, `Box` or `boxed_error`
/// would hide a member, and is refused by the `blinds` join, which the last
/// assertion runs over the same fixture with this join's own declaration.
#[test]
fn the_join_sees_an_item_boxing_a_developer_s_error_by_its_own_conversion() {
    let caught: syn::File = syn::parse_quote! {
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
    let (members, wrong) = judged(&caught);
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
    let (members, wrong) = judged(&passed);
    assert_eq!(members, 4);
    assert!(wrong.is_empty(), "{wrong:?}");

    let renames: syn::File = syn::parse_quote! {
        use std::convert::Into as Convert;
        use std::boxed::Box as Boxed;
        use nest_rs_core::boxed_error as boxed;
        type BoxError = Box<dyn std::error::Error + Send + Sync>;
    };
    let refused = crate::blinds::hidden("fixture.rs", &renames, &followed(), "decodes");
    assert_eq!(
        refused,
        [
            "fixture.rs :: `use nest_rs_core::boxed_error as boxed` renames it — blinds decodes",
            "fixture.rs :: `use std::boxed::Box as Boxed` renames it — blinds decodes",
            "fixture.rs :: `use std::convert::Into as Convert` renames it — blinds decodes",
        ],
        "a rename of what this join reads is refused, and an alias it resolves is not",
    );
}
