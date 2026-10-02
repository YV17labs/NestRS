//! The naming join: `architecture.md`'s layout law, against the tree.
//!
//! That file is the repo's one copy of the naming rules — `CLAUDE.md` calls it
//! "one copy, not a description of one" — and every sentence in it was prose
//! nothing ran. This module runs the ones that are a path or a symbol, and
//! **names the ones that are not** at the bottom, because *not mechanisable* and
//! *not yet done* are different sentences and only one of them is an excuse.
//!
//! Members are derived from the source in every case: the crate directories
//! themselves, the folders under each `src/`, and — for the reserved
//! vocabulary — the block inside `architecture.md`, parsed rather than
//! recopied. A second copy of that list here would be the defect the file
//! exists to prevent.
//!
//! **What it reads by its spelling** — and the `blinds` join refuses in any
//! other, under `cfg_attr`, or from a `macro_rules!`: `#[module]`, `impl Module`
//! and `impl DynamicModule`, which make a DI module wherever they sit;
//! `#[config]`, which makes a `*Config` one; `Error`, derived or implemented,
//! which makes an error type; and every framework decorator, since the role
//! files' types are the decorated items.

use std::collections::BTreeSet;
use std::path::Path;

use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    crate_dirs, flatten, is_cfg_test, parsed, relative, repo_root, rust_files, segments,
};
use proc_macro2::TokenTree;
use syn::Item;
use syn::visit::Visit;

use crate::Followed;

/// What this join reads by its spelling, for the `blinds` join to keep visible.
pub(crate) fn followed() -> Vec<Followed> {
    let mut out = vec![
        Followed::attribute("module"),
        Followed::implemented("Module"),
        Followed::implemented("DynamicModule"),
        Followed::attribute("config"),
        // `std::error::Error` / `core::error::Error`, and `thiserror`'s derive —
        // never `async_graphql::Error` and the other types that share the word.
        Followed::implemented("Error").through(&["error"]),
        Followed::attribute("Error").through(&["thiserror"]),
    ];
    for dir in crate_dirs() {
        if dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-macros"))
        {
            out.extend(
                nest_rs_conformance::sources::exported_decorators(&dir)
                    .into_iter()
                    .map(Followed::attribute),
            );
        }
    }
    out
}

/// One per gated join in this module. Named rather than written at the call
/// site for the reason every interpreted string here is: a mistyped filename
/// makes `compare` find no file, so `land` writes a fresh one and the failure
/// reads as a legitimate first landing rather than as a typo — while the real
/// baseline silently stops being read.
const MODULES_BASELINE: &str = "naming-baseline.txt";
const BINDINGS_BASELINE: &str = "bindings-baseline.txt";
const NAMESPACE_BASELINE: &str = "namespace-baseline.txt";
const ERRORS_BASELINE: &str = "errors-baseline.txt";
const ROOT_FILES_BASELINE: &str = "root-files-baseline.txt";
const CONFIG_NAMES_BASELINE: &str = "config-names-baseline.txt";

/// Below one of these the scan is reading the wrong tree and every agreement it
/// reports is an artefact.
///
/// A floor belongs to the population it guards, so this module carries one per
/// population rather than one number several joins share — the shape `canon`
/// and `docs` already use. Six of these were bare literals at their call site,
/// which is the one form that cannot be reviewed: a floor is all that stands
/// between a scan that silently reads nothing and a baseline diff that looks
/// green.
mod floors {
    pub(super) const MODULE_TYPES: usize = 18;
    pub(super) const EDGE_ADAPTERS: usize = 12;
    pub(super) const DISPATCH_MARKERS: usize = 10;
    pub(super) const DISPATCH_ENTRIES: usize = 4;
    pub(super) const EDGE_FOLDER_FILES: usize = 60;
    pub(super) const BINDING_FOLDER_FILES: usize = 8;
    pub(super) const CONFIGS: usize = 10;
    pub(super) const TREE_NAMESPACES: usize = 15;
    pub(super) const VOCABULARY_FILES: usize = 150;
    pub(super) const ERROR_TYPES: usize = 30;
    pub(super) const ADAPTER_ROOT_FILES: usize = 6;
    pub(super) const PRODUCT_SOURCE_ROOTS: usize = 6;
}

/// Fold a name to what it *says*, discarding how it was cased or separated:
/// `oauth-client`, `OAuthClient` and `Oauth_Client` all fold to `oauthclient`.
///
/// Folding rather than comparing literally is what lets the repo keep writing
/// one word two ways where English does — `openapi`/`OpenApi`,
/// `opentelemetry`/`OpenTelemetry`, `server-timing`/`ServerTiming` all agree
/// here and none would survive `==`.
fn folded(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Every type a parsed `module.rs` declares, whatever its suffix or visibility.
///
/// `architecture.md`: "**Every type in a `module.rs` shares the stem**, not just
/// the module". Membership used to be the suffix the rule imposes — `*Module`,
/// `*Setup`, `*Host` — so the rule could only ever be checked on types that
/// already half-obeyed it: `#[module] pub struct Wiring;` was not a member, and
/// neither was the private `AudienceBinding` the rule's own worked example sat
/// beside. So every struct, enum, union and alias is read, at any depth
/// outside `#[cfg(test)]`, and so is one a macro declares in the file — a
/// `struct Name` in any macro's tokens. A type a macro names through a
/// metavariable (`struct $name`) cannot be read, and is reported as such.
///
/// A trait is not a member: it is vocabulary that lives with its concern, and
/// the one `module.rs` holding traits is the kernel's, where `Module` and
/// `DynamicModule` *are* the concept the file is named for — the shape
/// `container.rs` holding `Container` has.
fn declared_module_types(file: &syn::File) -> Vec<String> {
    let mut types = ModuleTypes::default();
    types.visit_file(file);
    types.0
}

/// What a macro's tokens spell as a type declaration.
const METAVARIABLE: &str = "(a type a macro names through a metavariable)";

#[derive(Default)]
struct ModuleTypes(Vec<String>);

impl ModuleTypes {
    fn declared_in(&mut self, tokens: proc_macro2::TokenStream) {
        let mut flat = Vec::new();
        flatten(tokens, &mut flat);
        for pair in flat.windows(2) {
            let TokenTree::Ident(keyword) = &pair[0] else {
                continue;
            };
            if !["struct", "enum", "union"].contains(&keyword.to_string().as_str()) {
                continue;
            }
            match &pair[1] {
                TokenTree::Ident(name) => self.0.push(name.to_string()),
                TokenTree::Punct(dollar) if dollar.as_char() == '$' => {
                    self.0.push(METAVARIABLE.to_owned());
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for ModuleTypes {
    fn visit_item(&mut self, node: &'ast Item) {
        let attrs = match node {
            Item::Struct(i) => &i.attrs,
            Item::Enum(i) => &i.attrs,
            Item::Union(i) => &i.attrs,
            Item::Type(i) => &i.attrs,
            Item::Trait(i) => &i.attrs,
            Item::Mod(i) => &i.attrs,
            Item::Fn(i) => &i.attrs,
            Item::Impl(i) => &i.attrs,
            Item::Macro(i) => &i.attrs,
            _ => return syn::visit::visit_item(self, node),
        };
        if is_cfg_test(attrs) {
            return;
        }
        match node {
            Item::Struct(i) => self.0.push(i.ident.to_string()),
            Item::Enum(i) => self.0.push(i.ident.to_string()),
            Item::Union(i) => self.0.push(i.ident.to_string()),
            Item::Type(i) => self.0.push(i.ident.to_string()),
            Item::Macro(i) => self.declared_in(i.mac.tokens.clone()),
            _ => {}
        }
        syn::visit::visit_item(self, node);
    }

    fn visit_stmt_macro(&mut self, node: &'ast syn::StmtMacro) {
        self.declared_in(node.mac.tokens.clone());
    }
}

/// A crate's own subject — what follows the workspace prefix, which names the
/// workspace and nothing about this crate.
fn subject_of(krate: &str) -> &str {
    krate.strip_prefix("nest-rs-").unwrap_or(krate)
}

/// Words whose PascalCase is not a mechanical uppercase of the first letter.
///
/// Consulted **per `-`-separated segment**, so a family declares its spelling
/// once and every member inherits it: `oauth` here is what makes
/// `oauth-client`, `oauth-server` and `oauth-resource` derive `OAuthClient`,
/// `OAuthServer` and `OAuthResource` without a line each. A role added to the
/// family tomorrow needs no edit here.
const SPELLED: [(&str, &str); 6] = [
    ("oauth", "OAuth"),
    ("seaorm", "SeaOrm"),
    ("openapi", "OpenApi"),
    ("opentelemetry", "OpenTelemetry"),
    ("macro-hygiene", "MacroHygiene"),
    ("graphql", "Graphql"),
];

/// A folder or crate word as the type that names it spells it.
fn pascal(s: &str) -> String {
    s.split(['-', '_'])
        .map(|w| match SPELLED.iter().find(|(k, _)| *k == w) {
            Some((_, spelled)) => (*spelled).to_owned(),
            None => {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect()
}

/// The folder the product lives under, from the repository root — the `demo/`
/// workspace, whose `apps/` and `crates/` are its two fixed halves (`CLAUDE.md`,
/// *Two workspaces*).
const PRODUCT: &str = "demo";

/// Where the product's crates sit below [`PRODUCT`]: its binaries, and the
/// libraries they share.
const PRODUCT_WORKSPACES: [&str; 2] = ["apps", "crates"];

/// **The law: the stem is the crate's subject plus every folder below `src/`.**
///
/// `CLAUDE.md` calls naming the pillar, and this is the mechanical half of it:
/// from a path you know the type, from a type you know the path. `redis/queue/`
/// is `RedisQueue*`, `seaorm/health/` is `SeaOrmHealth*`, a crate whose
/// `module.rs` sits at its root is its own subject.
///
/// **A port keeps the bare name; a driver carries its own.** `ThrottlerStore`'s
/// implementations were already `InMemoryThrottler` and `RedisThrottler`, so the
/// module follows the implementation — `nest_rs::redis::RedisThrottlerModule`
/// sits beside `RedisThrottler` and says the same thing, while the bare
/// `ThrottlerModule` belongs to the crate defining the port. The path stutters
/// and a backend swap edits the type name; both are paid on purpose, because a
/// name that is unambiguous in a stack trace outranks a name that is short in an
/// import, and a module name lives in a composition root rather than in fifty
/// call sites.
///
/// **Every type in the file shares the stem**, not just the `*Module` — a rename
/// that leaves its `*Setup` or `*Host` behind is half a rename, and the half
/// left behind is the one a reader trips on. The stem is a **prefix**, not an
/// equality, so a file declaring two of one role may qualify them:
/// `ConfigRootSetup` and `ConfigFeatureSetup` name two seams and both carry
/// `Config`.
///
/// A product library is a container rather than a subject, so `features` never
/// prefixes: `audio/http/module.rs` is `AudioHttpModule`.
#[test]
fn every_module_type_is_named_for_its_path() {
    let root = repo_root();
    let (holes, scanned) = module_type_holes(&root, &crate_dirs());

    baseline::floor(scanned, floors::MODULE_TYPES, "module types");
    baseline::gate(
        MODULES_BASELINE,
        &holes,
        scanned,
        "module types",
        "module types whose name does not carry their path",
        "a type a reader cannot locate from its name, or name from its location. \
         The fix is the rename, or the folder the file should have been in — never \
         a line here",
    );
}

/// The module types below `dirs` whose name does not carry their path, and how
/// many were read.
fn module_type_holes(root: &Path, dirs: &[std::path::PathBuf]) -> (BTreeSet<String>, usize) {
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;

    for dir in dirs {
        let Some(krate) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // A framework crate is named for its subject, so it prefixes. A product
        // library is a container — its modules are domains, and
        // `FeaturesAudioHttpModule` would name the container nobody thinks in.
        let base = match krate.strip_prefix("nest-rs-") {
            Some(subject) => SPELLED
                .iter()
                .find(|(k, _)| *k == subject)
                .map(|(_, v)| (*v).to_owned())
                .unwrap_or_else(|| pascal(subject)),
            None => String::new(),
        };

        // A proc-macro crate's `module.rs` is the `#[module]` decorator's own
        // implementation, named for the decorator it expands — a DI module is
        // what it emits, never what it holds.
        if krate.ends_with("-macros") {
            continue;
        }
        let src = dir.join("src");
        for path in rust_files(&src) {
            if path.file_name().is_some_and(|n| n != "module.rs") {
                continue;
            }
            let Some(ast) = parsed(&path) else { continue };
            let parts = segments(&path, &src);
            let folders: String = parts[..parts.len() - 1]
                .iter()
                .map(|seg| &**seg)
                .map(|seg| {
                    SPELLED
                        .iter()
                        .find(|(k, _)| *k == seg)
                        .map(|(_, v)| (*v).to_owned())
                        .unwrap_or_else(|| pascal(seg))
                })
                .collect();
            // A framework crate with no folder is its own stem; a product
            // module with no folder would have none, and there are none.
            let stem = if base.is_empty() && folders.is_empty() {
                pascal(subject_of(krate))
            } else {
                format!("{base}{folders}")
            };

            for name in declared_module_types(&ast) {
                scanned += 1;
                if !name.starts_with(&stem) {
                    holes.insert(format!(
                        "{name}  ({})  — expected `{stem}…`",
                        relative(&path, root),
                    ));
                }
            }
        }
    }
    (holes, scanned)
}

/// The membership on a planted tree: a `#[module]` named for nothing, a private
/// provider, a type a macro declares and one it names through a metavariable
/// are all read, whatever their suffix — and a test's type is not.
#[test]
fn every_type_a_module_rs_declares_is_a_member() {
    const TREE: [(&str, &str); 2] = [
        (
            "crates/nest-rs-probe/Cargo.toml",
            "[package]\nname = \"nest-rs-probe\"\n",
        ),
        (
            "crates/nest-rs-probe/src/billing/module.rs",
            "#[module] pub struct ProbeBillingModule;\n\
             #[module] pub struct Wiring;\n\
             #[injectable] struct AudienceBinding;\n\
             macro_rules! declare { () => { pub struct BillingModule; }; }\n\
             declare!();\n\
             macro_rules! named { ($n:ident) => { pub struct $n; }; }\n\
             named!(Anything);\n\
             #[cfg(test)] mod tests { struct Fixture; }\n",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("naming-module-types-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let (holes, scanned) = module_type_holes(&root, &[root.join("crates/nest-rs-probe")]);
    let _ = std::fs::remove_dir_all(&root);

    let file = "crates/nest-rs-probe/src/billing/module.rs";
    assert_eq!(
        scanned, 5,
        "the module, the stray, the provider, and the two macros"
    );
    assert_eq!(
        holes.into_iter().collect::<Vec<_>>(),
        [
            format!("{METAVARIABLE}  ({file})  — expected `ProbeBilling…`"),
            format!("AudienceBinding  ({file})  — expected `ProbeBilling…`"),
            format!("BillingModule  ({file})  — expected `ProbeBilling…`"),
            format!("Wiring  ({file})  — expected `ProbeBilling…`"),
        ],
    );
}

/// **A DI module is declared in a `module.rs`, and only there.**
///
/// `architecture.md`: "`module.rs` is the DI module … One `#[module]` per file,
/// one `module.rs` per folder". The naming law is checked on `module.rs`, so a
/// `#[module]` — or a hand-written `impl Module` — anywhere else is a module
/// no naming rule reads, whatever it is called. No baseline: the tree holds
/// none.
///
/// **A `*Setup` is half of a module, and it is placed like one.** An `impl
/// DynamicModule` is what `for_root` returns — the seam every `*Setup` and the
/// hand-written `AuthnSetup` implement — so one outside a `module.rs` is a module
/// half no naming rule reads. It is not counted toward *one module per file*:
/// a module offering `for_root` and `for_feature` returns two setups from one
/// `module.rs`, which is `ConfigModule`'s shape. A `#[module]` a `macro_rules!`
/// writes is not read here: the `blinds` join refuses it.
#[test]
fn every_di_module_is_declared_in_a_module_rs() {
    let root = repo_root();
    let mut offenders = BTreeSet::new();
    for dir in crate_dirs() {
        if dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-macros"))
        {
            continue;
        }
        for path in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&path) else { continue };
            let found = di_modules(&ast);
            let here = path.file_name().is_some_and(|n| n == "module.rs");
            if !here && found.modules > 0 {
                offenders.insert(format!("{} declares a DI module", relative(&path, &root)));
            }
            if !here && found.setups > 0 {
                offenders.insert(format!(
                    "{} implements `DynamicModule` — a `*Setup` sits beside its module",
                    relative(&path, &root),
                ));
            }
            if here && found.modules > 1 {
                offenders.insert(format!(
                    "{} declares {} DI modules — one per file, two modules are two folders",
                    relative(&path, &root),
                    found.modules,
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a `#[module]` or an `impl Module` outside its own `module.rs`: {offenders:#?}",
    );
}

/// How many `#[module]` structs and `impl Module for` blocks a file holds —
/// its modules — and how many `impl DynamicModule for` — its setups — outside
/// `#[cfg(test)]`.
fn di_modules(file: &syn::File) -> DiModules {
    let mut found = DiModules::default();
    found.visit_file(file);
    found
}

#[derive(Default, Debug, PartialEq)]
struct DiModules {
    modules: usize,
    setups: usize,
}

impl<'ast> Visit<'ast> for DiModules {
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

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let decorated = node.attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "module")
        });
        if decorated && !is_cfg_test(&node.attrs) {
            self.modules += 1;
        }
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if is_cfg_test(&node.attrs) {
            return;
        }
        let implemented = node.trait_.as_ref().and_then(|(path, _)| {
            path.segments
                .last()
                .map(|segment| segment.ident.to_string())
        });
        match implemented.as_deref() {
            Some("Module") => self.modules += 1,
            Some("DynamicModule") => self.setups += 1,
            _ => {}
        }
        syn::visit::visit_item_impl(self, node);
    }
}

/// The DI-module gate on a planted file: a `*Setup` outside a `module.rs` is
/// read — the half macros2-5 found passing — and a `#[cfg(test)]` one is a
/// fixture.
#[test]
fn a_setup_is_read_as_a_module_half() {
    let file: syn::File = syn::parse_quote! {
        pub struct QueueZzprobeSetup;
        impl DynamicModule for QueueZzprobeSetup {}
        impl ::nest_rs_core::Module for Wiring {}
        #[module] pub struct Visible;
        #[cfg(test)]
        mod tests { impl DynamicModule for Fixture {} }
    };
    assert_eq!(
        di_modules(&file),
        DiModules {
            modules: 2,
            setups: 1
        }
    );
}

/// **The same law one level down: an edge adapter is named for its module.**
///
/// `architecture.md` spells the shape as `<Module><Edge>Module` for the module
/// and the role tables put the adapter's own types beside it, so
/// `posts/http/controller.rs` is `PostsController` and `users/ws/gateway.rs` is
/// `UsersGateway`. The **module** owns the name, not the edge folder: an edge is
/// an adapter *of* something, and `HttpController` would name the adapter twice
/// and the thing it adapts never.
///
/// **An edge folder directly under a framework crate's `src/` adapts the crate
/// itself**, so its adapter takes the crate's subject — see [`module_word`]. In
/// a product crate there is no such adapter to name: it is refused where it
/// sits, by [`an_edge_adapter_in_a_product_crate_sits_in_a_module_folder`].
#[test]
fn every_edge_adapter_is_named_for_the_module_it_adapts() {
    let (scanned, offenders) = misnamed_adapters(&repo_root());
    baseline::floor(scanned, floors::EDGE_ADAPTERS, "edge adapters");
    assert!(
        offenders.is_empty(),
        "an adapter is named for the module it adapts, so a reader finds it from \
         the feature they are working on rather than from the transport: \
         {offenders:#?}",
    );
}

/// The edge adapters under `root` whose type does not open with the module they
/// adapt, and how many adapter types were read.
fn misnamed_adapters(root: &Path) -> (usize, Vec<String>) {
    const ROLES: [(&str, &str); 5] = [
        ("controller.rs", "Controller"),
        ("resolver.rs", "Resolver"),
        ("gateway.rs", "Gateway"),
        ("processor.rs", "Processor"),
        ("listener.rs", "Listener"),
    ];
    let mut offenders = Vec::new();
    let mut scanned = 0usize;

    for area in ["crates", "demo"] {
        for path in rust_files(&root.join(area)) {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some((_, role)) = ROLES.iter().find(|(f, _)| *f == name) else {
                continue;
            };
            let parts = segments(&path, root);
            // …/<module>/<edge>/<role>.rs — the edge folder is what marks this
            // file as an adapter rather than a module-root role file.
            let Some(edge_at) = parts.len().checked_sub(2) else {
                continue;
            };
            if !crate::EDGES.contains(&&*parts[edge_at]) {
                continue;
            }
            let Some(module) = module_word(&parts[..edge_at]).map(folded) else {
                continue;
            };
            let Some(ast) = parsed(&path) else { continue };
            for declared in structs_at_any_depth(&ast.items) {
                let ident = declared.ident.to_string();
                if !ident.ends_with(role) {
                    continue;
                }
                scanned += 1;
                if !folded(&ident).starts_with(&module) {
                    offenders.push(format!("{ident} in {}", relative(&path, root)));
                }
            }
        }
    }
    // The walk follows the filesystem's order; a verdict does not.
    offenders.sort();
    (scanned, offenders)
}

/// Every struct an item list declares, inline modules included and
/// `#[cfg(test)]` ones left out — a type one `mod` down is the file's all the
/// same, and reading the top level alone let one pass unread.
fn structs_at_any_depth(items: &[Item]) -> Vec<&syn::ItemStruct> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Struct(s) if !is_cfg_test(&s.attrs) => out.push(s),
            Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                if let Some((_, inner)) = &m.content {
                    out.extend(structs_at_any_depth(inner));
                }
            }
            _ => {}
        }
    }
    out
}

/// A struct one `mod` down is read; a test's is not.
#[test]
fn a_struct_is_read_at_any_depth() {
    let file: syn::File = syn::parse_quote! {
        #[controller] pub struct PostsController;
        mod inner { #[controller] pub struct WrongController; }
        #[cfg(test)]
        mod tests { pub struct FixtureController; }
    };
    let read: Vec<String> = structs_at_any_depth(&file.items)
        .into_iter()
        .map(|s| s.ident.to_string())
        .collect();
    assert_eq!(read, ["PostsController", "WrongController"]);
}

/// The word the adapters in an edge folder are named for, read off the folders
/// above that edge folder (`above`, from the repository root down).
///
/// **The module folder above the edge — unless that folder is a source root**,
/// which names a layout level and no module. An edge folder directly under a
/// framework crate's `src/` adapts the crate itself, so it takes the crate's
/// subject: `nest-rs-x/src/http/controller.rs` is `XController`, the stem the
/// module join already gives `nest-rs-x/src/http/module.rs` (`XHttpModule`).
/// Read as a module, `src` expected a `SrcController` and refused the right name.
///
/// **A product crate's source root has no word to give**, because the crate is
/// not a subject there: the app name stops at `<App>Module`, and a product
/// library is a container of modules. So nothing is named for it — an edge
/// folder there is refused where it sits
/// ([`an_edge_adapter_in_a_product_crate_sits_in_a_module_folder`]), and one
/// refusal says so rather than two sentences disagreeing about one folder.
///
/// **A suite root is the same level**, because a suite's module tree mirrors
/// `src/` (`testing.md`): `tests/integration/http/` is the mirror of `src/http/`,
/// and the suite's name is no more a module than `src` is. The reserved
/// vocabulary keeps both words out of a module's name, so neither reading can
/// shadow a real module.
fn module_word<'a>(above: &'a [std::borrow::Cow<'a, str>]) -> Option<&'a str> {
    match above {
        [area, .., root] if root == "src" && area == PRODUCT => None,
        [.., krate, root] if root == "src" => Some(subject_of(krate)),
        [.., krate, tests, _suite] if tests == "tests" => Some(subject_of(krate)),
        [.., module] => Some(module),
        [] => None,
    }
}

/// [`module_word`], on a planted tree: every level an edge folder can sit under,
/// with a name that passes and a decoy that the old reading — the folder above
/// the edge, whatever it named — judged the other way.
#[test]
fn an_edge_folder_directly_under_src_adapts_the_crate() {
    const TREE: [(&str, &str); 9] = [
        (
            "crates/nest-rs-probe/src/http/controller.rs",
            "pub struct ProbeController;",
        ),
        // Named for the layout level, which the old reading asked for.
        (
            "crates/nest-rs-probe/src/ws/gateway.rs",
            "pub struct SrcGateway;",
        ),
        // A subject of several words folds as one, as the module join folds it.
        (
            "crates/nest-rs-oauth-probe/src/http/controller.rs",
            "pub struct OAuthProbeController;",
        ),
        // A product crate's root gives no word, so its adapter is not read here
        // — it is refused where it sits, by the placement rule.
        (
            "demo/apps/api/src/graphql/resolver.rs",
            "pub struct ApiResolver;",
        ),
        // The suite mirror of `src/http/`, and a decoy named for the suite.
        (
            "crates/nest-rs-probe/tests/integration/http/controller.rs",
            "pub struct ProbeController;",
        ),
        (
            "crates/nest-rs-probe/tests/integration/queue/processor.rs",
            "pub struct IntegrationProcessor;",
        ),
        // A module folder above the edge still owns the name, in source and in
        // the mirror — and the crate's subject does not stand in for it.
        (
            "crates/nest-rs-probe/src/users/http/controller.rs",
            "pub struct UsersController;",
        ),
        (
            "crates/nest-rs-probe/tests/integration/users/ws/gateway.rs",
            "pub struct UsersGateway;",
        ),
        (
            "crates/nest-rs-probe/src/posts/events/listener.rs",
            "pub struct ProbeListener;",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("naming-edge-under-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let verdict = misnamed_adapters(&root);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        verdict,
        (
            8,
            vec![
                "IntegrationProcessor in crates/nest-rs-probe/tests/integration/queue/processor.rs"
                    .to_owned(),
                "ProbeListener in crates/nest-rs-probe/src/posts/events/listener.rs".to_owned(),
                "SrcGateway in crates/nest-rs-probe/src/ws/gateway.rs".to_owned(),
            ],
        ),
    );
}

/// **In a product crate, an edge adapter sits in a module folder** — never
/// directly under an app's or a library's `src/`.
///
/// The framework reads that level as the crate adapting itself, because a
/// framework crate is named for its subject (`nest-rs-x/src/http/controller.rs`
/// is `XController`). A product crate is not a subject at that level, and
/// `architecture.md` says so twice: *"the app name stops at `<App>Module`"*, and
/// a product library is *"a container — its modules are domains"*. So an adapter
/// there adapts nothing a reader can name, and the edge it serves belongs to the
/// module that owns the domain — `src/<module>/<edge>/`, named for that module.
///
/// **A file is the same module as a folder**: `src/http.rs` declares `crate::http`
/// exactly as `src/http/mod.rs` does, so both spellings are refused, and a rule
/// that caught one would teach the other. Every product crate is read, the
/// binaries and every library beside `features` — the reserved-vocabulary join
/// reads `features` and the apps only, and answered an edge there with *"pick
/// the domain word"*, which is the wrong remedy for an adapter that is misplaced
/// rather than misnamed; it leaves the edge words to this one sentence.
///
/// Framework crates keep their reading. A scaffolded project is a product too,
/// and is outside this suite: `nestrs lint` runs only the pairing rule, and
/// whether it should run this one is the owner's.
#[test]
fn an_edge_adapter_in_a_product_crate_sits_in_a_module_folder() {
    let (scanned, offenders) = edges_at_a_product_root(&repo_root());
    baseline::floor(
        scanned,
        floors::PRODUCT_SOURCE_ROOTS,
        "product source roots",
    );
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// The product source roots below `root` that hold a module named for an edge,
/// each with the sentence refusing it, and how many roots were read.
fn edges_at_a_product_root(root: &Path) -> (usize, Vec<String>) {
    let mut scanned = 0usize;
    let mut offenders = Vec::new();
    for workspace in PRODUCT_WORKSPACES {
        let Ok(crates) = std::fs::read_dir(root.join(PRODUCT).join(workspace)) else {
            continue;
        };
        for krate in crates.flatten() {
            let Some(name) = krate.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let src = krate.path().join("src");
            let Ok(entries) = std::fs::read_dir(&src) else {
                continue;
            };
            scanned += 1;
            for entry in entries.flatten() {
                let path = entry.path();
                let module = if path.is_dir() {
                    path.file_name()
                } else if path.extension().is_some_and(|e| e == "rs") {
                    path.file_stem()
                } else {
                    continue;
                };
                let Some(edge) = module
                    .and_then(|m| m.to_str())
                    .filter(|m| crate::EDGES.contains(m))
                else {
                    continue;
                };
                let container = if workspace == "apps" {
                    format!(
                        "the app name stops at `{}Module`, so nothing below it adapts the app",
                        pascal(&name),
                    )
                } else {
                    "a product library is a container of modules, never a subject, so there is \
                     no module at its root to adapt"
                        .to_owned()
                };
                offenders.push(format!(
                    "{} — an edge adapter belongs to a module folder: move it to \
                     `src/<module>/{edge}/`, named for the module it adapts; {container}",
                    relative(&path, root),
                ));
            }
        }
    }
    offenders.sort();
    (scanned, offenders)
}

/// [`edges_at_a_product_root`] and [`misnamed_adapters`] on one planted tree:
/// both spellings of an edge module refused at an app's root and at a library's,
/// the same folder a module owns left alone, the framework's reading of its own
/// root kept — and no adapter answered twice.
#[test]
fn an_edge_at_a_product_root_is_refused_where_a_framework_root_is_read() {
    const TREE: [(&str, &str); 8] = [
        (
            "demo/apps/api/src/http/controller.rs",
            "pub struct ApiController;",
        ),
        ("demo/apps/api/src/graphql.rs", "pub struct ApiResolver;"),
        (
            "demo/crates/features/src/ws/gateway.rs",
            "pub struct FeaturesGateway;",
        ),
        // A module owns the edge, in an app and in a library.
        (
            "demo/apps/api/src/users/http/controller.rs",
            "pub struct UsersController;",
        ),
        (
            "demo/crates/features/src/posts/ws/gateway.rs",
            "pub struct PostsGateway;",
        ),
        // A word that only opens like an edge is not one, and a suite is not a
        // source root.
        ("demo/apps/api/src/httpd/mod.rs", "pub struct Daemon;"),
        ("demo/apps/api/tests/e2e/http.rs", "fn served() {}"),
        // The framework's own root keeps its reading.
        (
            "crates/nest-rs-probe/src/http/controller.rs",
            "pub struct ProbeController;",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("naming-product-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let placed = edges_at_a_product_root(&root);
    let named = misnamed_adapters(&root);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        placed,
        (
            2,
            vec![
                "demo/apps/api/src/graphql.rs — an edge adapter belongs to a module folder: \
                 move it to `src/<module>/graphql/`, named for the module it adapts; the app \
                 name stops at `ApiModule`, so nothing below it adapts the app"
                    .to_owned(),
                "demo/apps/api/src/http — an edge adapter belongs to a module folder: move it \
                 to `src/<module>/http/`, named for the module it adapts; the app name stops at \
                 `ApiModule`, so nothing below it adapts the app"
                    .to_owned(),
                "demo/crates/features/src/ws — an edge adapter belongs to a module folder: move \
                 it to `src/<module>/ws/`, named for the module it adapts; a product library is \
                 a container of modules, never a subject, so there is no module at its root to \
                 adapt"
                    .to_owned(),
            ],
        ),
    );
    // The two module-owned adapters and the framework's, and not the refused
    // ones: a misplaced adapter is answered once, by where it sits.
    assert_eq!(named, (3, Vec::new()));
}

/// `architecture.md`: "**No `*_module.rs`, ever.** One `#[module]` per file, one
/// `module.rs` per folder; two modules in a feature means two folders."
///
/// No baseline: the tree is clean today, and the whole value is that it stays
/// so. A first offender should fail the build, not land a line.
#[test]
fn no_file_is_named_for_a_module_instead_of_being_one() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for area in ["crates", "demo"] {
        for path in rust_files(&root.join(area)) {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_module.rs"))
            {
                offenders.push(relative(&path, &root));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`architecture.md` closes this one absolutely — a DI module is \
         `module.rs` in its own folder, never `<name>_module.rs`: {offenders:#?}",
    );
}

/// `architecture.md`: "A folder invented to group 'things that go together' —
/// `contract/`, `types/`, `core/`, `shared/`, `common/`, `interfaces/` — is a
/// defect: every file it would hold is already named by a table above […] A
/// folder that feels too full means the module is too big; split the module,
/// never the vocabulary."
///
/// The six names are the ones the rule itself enumerates, so this list is a
/// quotation rather than an invention.
#[test]
fn no_module_invents_a_folder_to_group_things_that_go_together() {
    let offenders = invented_folders(&repo_root());
    assert!(
        offenders.is_empty(),
        "each of these is a bag, and every file in it is already named by a role \
         table — split the module instead: {offenders:#?}",
    );
}

/// The folders under a `src/` tree below `root` that carry one of the names the
/// rule enumerates.
fn invented_folders(root: &Path) -> Vec<String> {
    const INVENTED: [&str; 6] = [
        "contract",
        "types",
        "core",
        "shared",
        "common",
        "interfaces",
    ];
    let mut offenders = Vec::new();

    for area in ["crates", "demo"] {
        walk_dirs(&root.join(area), &mut |dir| {
            // Only folders *inside* a `src/` tree are module vocabulary; a
            // crate may legitimately be called `nest-rs-core`, and a suite may
            // keep what its siblings share in a `common/`. Read below the root:
            // a checkout under `~/src/` put every folder of the tree inside one.
            let parts = segments(dir, root);
            if !parts.iter().any(|part| part == "src") {
                return;
            }
            if parts.last().is_some_and(|name| INVENTED.contains(&&**name)) {
                offenders.push(relative(dir, root));
            }
        });
    }
    offenders
}

/// `architecture.md`: "**A module may not take a name from the structural
/// vocabulary.** These words already mean something to the layout, and reusing
/// one makes every path ambiguous. Pick the domain word instead — a module about
/// desktop applications is `programs`, not `apps`."
///
/// **Level-aware, and it has to be.** The same word is legal one level down: a
/// module's `http/` is the sanctioned edge folder and its `dtos/` the sanctioned
/// plural role folder. What the rule forbids is a *module* — a domain — taking
/// one. So the population is the folders directly under a product `src/`, which
/// is where a module lives. The edge words are the one row left out: at that
/// level they are an adapter in the wrong place, and the placement rule words
/// the remedy (`src/<module>/<edge>/`) that "pick the domain word" would not.
///
/// The reserved list is derived from `architecture.md` itself, by the CLI that
/// ships that file — recopying it, or re-parsing it, would be the second copy
/// `CLAUDE.md` says that file exists to prevent.
#[test]
fn no_module_takes_a_name_from_the_structural_vocabulary() {
    let root = repo_root();
    // Read through `nest_rs_cli`, which derives it from the same
    // `architecture.md` it embeds into every scaffolded project. A second
    // parser here would be the copy that file exists to prevent.
    let reserved = nest_rs_cli::reserved_words();
    assert!(
        reserved.len() > 20,
        "parsed {} reserved words out of architecture.md — the block moved and \
         this test is now reading nothing",
        reserved.len(),
    );

    let mut offenders = Vec::new();
    let mut module_roots = vec![root.join("demo/crates/features/src")];
    if let Ok(apps) = std::fs::read_dir(root.join("demo/apps")) {
        module_roots.extend(apps.flatten().map(|a| a.path().join("src")));
    }

    for src in module_roots {
        let Ok(entries) = std::fs::read_dir(&src) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // An edge word here is an adapter misplaced rather than a module
            // misnamed, so "pick the domain word" is the wrong remedy for it:
            // [`an_edge_adapter_in_a_product_crate_sits_in_a_module_folder`]
            // answers it, naming the move, over every product crate.
            if reserved.contains(name.as_str()) && !crate::EDGES.contains(&name.as_str()) {
                offenders.push(relative(&entry.path(), &root));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a module named from the layout's own vocabulary makes every path below \
         it ambiguous — pick the domain word: {offenders:#?}",
    );
}

/// The one rule in `architecture.md` a path cannot derive: a vocabulary file's
/// stem against the types it declares.
///
/// Run here through `nest_rs_cli::lint`, which is the same code `nestrs lint`
/// runs in a developer's project — not a second implementation of it. A rule
/// the framework ships and does not itself pass is the failure that matters,
/// and re-deriving it here is exactly how the two would come to disagree with
/// nobody positioned to notice.
///
/// No baseline: the tree is clean today, and the whole value is that it stays.
#[test]
fn every_file_is_named_for_what_it_declares() {
    let root = repo_root();
    let scan = nest_rs_cli::lint::scan(&root);
    baseline::floor(scan.checked, floors::VOCABULARY_FILES, "vocabulary files");

    let offenders: Vec<String> = scan.findings.iter().map(ToString::to_string).collect();
    assert!(offenders.is_empty(), "{}", offenders.join("\n\n"),);
}

/// Every directory under `dir`, recursively, skipping build output.
fn walk_dirs(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || path.file_name().is_some_and(|n| n == "target") {
            continue;
        }
        visit(&path);
        walk_dirs(&path, visit);
    }
}

// ── What this join deliberately does not check, and why ─────────────────────
//
// Naming them is the point: a reader who finds five tests here must not
// conclude the file's law is four sentences long.
//
// - **Which declared subject is the right one.** The law binds the vocabulary,
//   not the choice: a module may name itself for its crate's subject, a port it
//   depends on, or its folder, and this join cannot say which of those the
//   author should have picked. `CoreModule` inside `nest-rs-http` segments
//   cleanly and is still a bad name. What it does catch is the class that
//   actually shipped — twice — a subject **nothing declares**, which is what a
//   synonym split always is: `DiscoveryModule` under `nest-rs-oauth-resource`
//   fails here, and that is the defect this join was written for. Adjudicating
//   between two declared subjects stays `/architecture`'s, and it is judgement
//   rather than a scan.
// - **"One `#[module]` per file."** Counting the attribute is a false-positive
//   machine, and measuring it proved so: `src/module.rs` files hold 0, 2, 3, 4
//   and 7 of them today and every one is legal. Zero means a hand-written
//   `impl Module`, which `framework.md` says "is still a DI module"; the higher
//   counts are `#[cfg(test)]` fixtures and doctests inside the same file. A
//   sound check needs to parse, drop test-gated items and doctests, and then
//   still reconcile the hand-written form — which is a different test from this
//   one and owes its own argument.
// - **"A file exists only if it has real content."** *Real* is the judgement;
//   an empty `mod.rs` is legal and a one-line one may or may not be.
// - **The role tables** (`service.rs`, `guard.rs`, `controller.rs`, …). What a
//   file *is* comes from what it holds, so checking the name against the role
//   means classifying the contents — `/architecture`'s job, not a join's.
// - **A name collision between the product and the framework.** Inside either
//   workspace the compiler is the check and it is free: two `AuthnModule` in
//   one type namespace is `E0255`, so `cargo check` catches what a join would.
//   The one site it cannot see is `nest-rs-cli/src/templates/*.rs`, where the
//   generated code is a `&str` — and that is exactly where the collision
//   shipped when the `app_` prefix was removed. Its guard is not another join
//   here: it is `nest-rs-cli/tests/e2e/scaffold.rs`, which `cargo check`s a
//   generated workspace. **A template edit is not done until that suite has
//   run** — it is `binary(e2e)`-gated, so the default `not binary(e2e)` run
//   says nothing about it.
// - **"A product module never wears a framework concern's name."** There is no
//   such rule any more, and the join that enforced it is deleted rather than
//   relaxed: the `app_` prefix triggered on the namespace the umbrella
//   re-exports, not on the ident, so it marked fourteen types that collided
//   with nothing. A product name is told from a framework one by the path a
//   caller already types — `io::Result` beside `Result` — and a path is not
//   something a name-shaped join can weigh.
// - **"The project name stops at the workspace."** Checkable in principle,
//   unwritable without the project's name, which no file in this repo declares
//   as data.

/// **The same law read on a file's *content*: a file under `<edge>/` serves
/// that edge and nothing else.**
///
/// `architecture.md` gives an edge folder one job — it is one adapter of one
/// module for one transport — so what the folder says and what the file does are
/// the same statement, exactly as a type's name and its path are. A file that
/// answers two edges from inside one of them makes that statement false, and the
/// cost is never only the path: through 5.1 `nest-rs-authz/src/http/guard.rs`
/// held the `AbilityGuard` that implements `check_http`, `check_graphql`,
/// `check_ws_message` **and** `check_mcp`, so a GraphQL-only app enabled the
/// HTTP feature to reach its own guard, the WS entry compiled under `http`, and
/// three of the demo's four `Authz<Edge>Module`s imported an HTTP adapter they
/// never served. The name read fine; only its location was wrong, which is the
/// class `.claude/skills/architecture/SKILL.md` says an LLM most reliably
/// under-weights — hence a scan rather than a sentence.
///
/// **The vocabulary is derived, never listed.** An edge's dispatch surface is
/// what the framework calls on your type *because of the edge it is bound at*,
/// and it declares itself by name: a `pub trait` whose ident opens with an edge
/// word (`WsGuard`, `McpToolContext`), and a method declared in a `pub trait`
/// carrying one as a `_`-delimited segment (`check_graphql`,
/// `transform_ws_data`). Adding an edge, or a marker for one, therefore extends
/// this test the day it is written.
///
/// **What it cannot see, stated rather than implied:** a trait that is
/// edge-bound but does not *say so* in its name — `SocketContext`,
/// `RouteResponseShaper` — is outside a name-derived law. Deriving the edge from
/// the declaring crate instead would catch them and sweep in every generic
/// utility those crates also export (`Reflector`, `HandlerMetadata`), which is
/// the false-positive trade this shape declines. Nor does it read an **alias**:
/// `pub type AuthzGuard = AbilityGuard<AuthzAbility>;` under `authz/http/` was
/// the product's copy of this same defect, and seeing it needs the aliased
/// type's own file resolved across a crate boundary. Both gaps are owner
/// questions, not holes this test pretends to cover — what closes the alias in
/// practice is that the framework type it points at is now at a root, and a CLI
/// template writes the alias beside it.
///
/// Tests are out of population on purpose: a bridge suite under `graphql/`
/// legitimately declares an HTTP guard fixture, because re-running the HTTP
/// chain in band is the thing under test.
#[test]
fn no_file_under_an_edge_folder_answers_another_edge() {
    let scan = edge_folders(&repo_root());
    baseline::floor(
        scan.markers,
        floors::DISPATCH_MARKERS,
        "edge dispatch markers",
    );
    baseline::floor(
        scan.entries,
        floors::DISPATCH_ENTRIES,
        "edge dispatch entries",
    );
    baseline::floor(
        scan.scanned,
        floors::EDGE_FOLDER_FILES,
        "files under an edge folder",
    );
    let offenders = scan.offenders;
    assert!(
        offenders.is_empty(),
        "an edge folder says the file serves that edge; answering another from \
         inside it makes the path a false statement, and the fix is the move — \
         a type answering every edge belongs where every edge can reach it, not \
         in the folder of whichever one asked first: {offenders:#?}",
    );
}

/// What [`edge_folders`] read, and what it found.
#[derive(Debug, PartialEq)]
struct EdgeFolders {
    /// Edge-marker traits in the dispatch vocabulary.
    markers: usize,
    /// Edge-entry methods in the dispatch vocabulary.
    entries: usize,
    /// Source files sitting under an edge folder.
    scanned: usize,
    /// Each file answering an edge other than its folder's, with what it answers.
    offenders: Vec<String>,
}

/// The source files under `root` that sit in one edge's folder and answer
/// another's dispatch surface.
///
/// **The edge folder is looked for between the file and its `src/`, and nowhere
/// else.** Above `src/` a folder is a crate or a workspace, and a crate or an app
/// named like an edge is not an adapter folder; above the root it is not the
/// tree at all — read off the absolute path, a checkout under
/// `…/schedule/nestrs` made every file with no edge folder of its own a
/// `schedule` adapter, and `nest-rs-authz`'s one guard for all four transports
/// was reported as a scheduler answering them.
fn edge_folders(root: &Path) -> EdgeFolders {
    let (markers, entries) = edge_dispatch_vocabulary(root);
    let mut offenders = Vec::new();
    let mut scanned = 0usize;

    for area in ["crates", "demo"] {
        for path in rust_files(&root.join(area)) {
            let parts = segments(&path, root);
            let folders = &parts[..parts.len() - 1];
            // Source only: a suite's files are out of population, for the reason
            // the test above gives.
            let Some(src_at) = folders.iter().position(|folder| folder == "src") else {
                continue;
            };
            // The deepest edge folder above the file — `audio/mcp/tool.rs` is
            // mcp's, and a nested one would be read the same way.
            let Some(owner) = folders[src_at + 1..]
                .iter()
                .rev()
                .map(|folder| &**folder)
                .find(|folder| crate::EDGES.contains(folder))
            else {
                continue;
            };
            let Some(ast) = parsed(&path) else { continue };
            scanned += 1;

            let mut foreign = BTreeSet::new();
            for item in &ast.items {
                let Item::Impl(block) = item else { continue };
                if let Some((trait_path, _)) = &block.trait_
                    && let Some(last) = trait_path.segments.last()
                    && let Some(edge) = markers.get(&last.ident.to_string())
                    && edge != owner
                {
                    foreign.insert(format!("impl {} ({edge})", last.ident));
                }
                for sub in &block.items {
                    let syn::ImplItem::Fn(f) = sub else { continue };
                    if let Some(edge) = entries.get(&f.sig.ident.to_string())
                        && edge != owner
                    {
                        foreign.insert(format!("fn {}() ({edge})", f.sig.ident));
                    }
                }
            }
            if !foreign.is_empty() {
                offenders.push(format!(
                    "{} sits under {owner}/ and answers {}",
                    relative(&path, root),
                    foreign.into_iter().collect::<Vec<_>>().join(", "),
                ));
            }
        }
    }

    EdgeFolders {
        markers: markers.len(),
        entries: entries.len(),
        scanned,
        offenders,
    }
}

/// The edge dispatch surface, read off the framework's own trait declarations.
///
/// Returns `(markers, entries)` — trait idents that open with an edge word, and
/// method idents declared inside a `pub trait` carrying one as a `_`-delimited
/// segment. `__`-prefixed idents are macro seams rather than a surface a
/// developer implements, so they are skipped. A trait a suite declares is a
/// fixture rather than a surface, so only a `src/` tree below `root` is read —
/// below it, because under `~/src/` every suite's traits read as source.
fn edge_dispatch_vocabulary(
    root: &Path,
) -> (
    std::collections::BTreeMap<String, String>,
    std::collections::BTreeMap<String, String>,
) {
    let mut markers = std::collections::BTreeMap::new();
    let mut entries = std::collections::BTreeMap::new();

    for path in rust_files(&root.join("crates")) {
        let parts = segments(&path, root);
        if !parts[..parts.len() - 1]
            .iter()
            .any(|folder| folder == "src")
        {
            continue;
        }
        let Some(ast) = parsed(&path) else { continue };
        for item in &ast.items {
            let Item::Trait(t) = item else { continue };
            if !matches!(t.vis, syn::Visibility::Public(_)) {
                continue;
            }
            let name = t.ident.to_string();
            if let Some(edge) = edge_opening(&name) {
                markers.insert(name, edge);
            }
            for sub in &t.items {
                let syn::TraitItem::Fn(f) = sub else { continue };
                let method = f.sig.ident.to_string();
                if method.starts_with("__") {
                    continue;
                }
                if let Some(edge) = edge_segment(&method) {
                    entries.insert(method, edge);
                }
            }
        }
    }
    (markers, entries)
}

/// The edge a `CamelCase` ident opens with — `WsGuard` is ws's, `Websocket`
/// nobody's, because the word has to end where the next one starts.
fn edge_opening(ident: &str) -> Option<String> {
    crate::EDGES
        .iter()
        .find(|edge| {
            let lowered = ident.to_ascii_lowercase();
            lowered.starts_with(*edge)
                && ident[edge.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
        })
        .map(|edge| (*edge).to_string())
}

/// The edge a `snake_case` ident carries as a whole segment — `check_ws_message`
/// is ws's, `http_status` is not a dispatch entry and never reaches here.
fn edge_segment(ident: &str) -> Option<String> {
    crate::EDGES
        .iter()
        .find(|edge| ident.split('_').any(|part| part == **edge))
        .map(|edge| (*edge).to_string())
}

/// **The same law, one folder-set wider: a binding folder's files name only its
/// own types.**
///
/// [`no_file_under_an_edge_folder_answers_another_edge`] reads the law over the
/// closed edge vocabulary. This one reads it over the set `architecture.md`
/// describes next: *"A driver gives each port it binds a folder."* A folder under
/// a framework crate's `src/` that declares a `module.rs` **is** such a binding,
/// so the folder is a statement about every file inside it, and a file naming a
/// sibling binding's type makes that statement false.
///
/// **`module.rs` is exempt, and that exemption is the whole precision of the
/// test.** A module composes — naming what it registers is its one job — so
/// `database/module.rs` reaching `http/interceptor.rs` is the layout working.
/// Every *other* file states what it serves. `nest-rs-redis`'s
/// `throttler/store.rs` holding a `RedisQueueConnection` field is the defect
/// this test exists for: three bindings share one Redis handle, and it is named,
/// filed and module-gated for whichever asked first — so enabling the throttler
/// obliges an app with no queue to import `RedisQueueModule` and set
/// `NESTRS_QUEUE__URL`.
///
/// **Framework crates only, and the boundary is not arbitrary.** In
/// `demo/crates/features` the folders under `src/` are *domains*, not bindings —
/// `posts/` depending on `authn/`'s `Claims` is a feature using another feature,
/// which is what a product does. Run there, this scan reports 44 legitimate
/// references and no defect. The product's own sub-folders are already covered
/// one level down, by the edge law above.
///
/// What it cannot see, stated rather than implied: a shared thing whose
/// consumers are in **other crates** — `nest_rs_authn::JwtService`, reached by
/// three OAuth-family crates through a module named for authentication — is the
/// same class across a boundary this scan does not cross. That half is the
/// reviewer's.
#[test]
fn no_binding_folder_names_a_sibling_bindings_type() {
    let root = repo_root();
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;

    // Framework crates only — the boundary the doc above argues. `crate_dirs`
    // walks all three workspaces, and the product's top-level folders are
    // domains rather than bindings.
    let framework = root.join("crates");
    for krate in crate_dirs() {
        if !krate.starts_with(&framework) {
            continue;
        }
        let src = krate.join("src");
        let bindings: BTreeSet<String> = rust_files(&src)
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "module.rs"))
            .filter_map(|p| p.parent())
            .filter(|d| *d != src)
            .filter_map(|d| d.file_name().and_then(|n| n.to_str()))
            .map(str::to_owned)
            .collect();
        if bindings.len() < 2 {
            continue;
        }

        // Which binding declares each public type. Parsed rather than matched:
        // a `*-macros` crate and a CLI template carry these very idents inside
        // string literals, and a token walk never sees a literal.
        let mut declared: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let mut files = Vec::new();
        for path in rust_files(&src) {
            let Some(folder) = binding_of(&path, &src, &bindings) else {
                continue;
            };
            let Some(ast) = parsed(&path) else { continue };
            for item in &ast.items {
                let (ident, vis) = match item {
                    Item::Struct(i) => (i.ident.to_string(), &i.vis),
                    Item::Enum(i) => (i.ident.to_string(), &i.vis),
                    Item::Type(i) => (i.ident.to_string(), &i.vis),
                    Item::Trait(i) => (i.ident.to_string(), &i.vis),
                    _ => continue,
                };
                if matches!(vis, syn::Visibility::Public(_)) {
                    declared.insert(ident, folder.clone());
                }
            }
            files.push((path, folder, ast));
        }

        for (path, folder, ast) in &files {
            if path.file_name().is_some_and(|n| n == "module.rs") {
                continue;
            }
            scanned += 1;
            let named = idents_in(ast);
            for (ident, home) in &declared {
                if home != folder && named.contains(ident) {
                    holes.insert(format!(
                        "{ident} is {home}/'s, named by {}",
                        relative(path, &root),
                    ));
                }
            }
        }
    }

    baseline::floor(
        scanned,
        floors::BINDING_FOLDER_FILES,
        "files under a binding folder",
    );
    baseline::gate(
        BINDINGS_BASELINE,
        &holes,
        scanned,
        "files under a binding folder",
        "types reached across a binding boundary",
        "a thing several bindings share, named and filed for whichever asked \
         first — it belongs at the level all of them reach, which is the crate \
         root",
    );
}

/// **Every error type lives in `error.rs`** — `CLAUDE.md`, *Naming — strict*:
/// public or crate-private, domain or driver defect.
///
/// An error type is a struct or enum deriving `Error` (`thiserror`'s or any
/// other), or one a file implements `std::error::Error` for. A `#[cfg(test)]`
/// module is a fixture and is skipped. Both workspaces: the rule is the layout's,
/// and a product's `service.rs` holding its errors is the case it was written for.
#[test]
fn every_error_type_lives_in_error_rs() {
    fn derives_error(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|attr| {
            attr.path().is_ident("derive")
                && attr
                    .parse_args_with(
                        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
                    )
                    .is_ok_and(|paths| {
                        paths
                            .iter()
                            .any(|path| path.segments.last().is_some_and(|s| s.ident == "Error"))
                    })
        })
    }
    fn walk(items: &[Item], out: &mut Vec<String>) {
        for item in items {
            match item {
                Item::Struct(i) if derives_error(&i.attrs) => out.push(i.ident.to_string()),
                Item::Enum(i) if derives_error(&i.attrs) => out.push(i.ident.to_string()),
                Item::Impl(i) => {
                    let implements_error = i.trait_.as_ref().is_some_and(|(path, _)| {
                        path.segments.last().is_some_and(|s| s.ident == "Error")
                    });
                    if implements_error
                        && let syn::Type::Path(ty) = &*i.self_ty
                        && let Some(last) = ty.path.segments.last()
                    {
                        out.push(last.ident.to_string());
                    }
                }
                Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                    if let Some((_, inner)) = &m.content {
                        walk(inner, out);
                    }
                }
                _ => {}
            }
        }
    }

    let root = repo_root();
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;
    for krate in crate_dirs() {
        for path in rust_files(&krate.join("src")) {
            let Some(ast) = parsed(&path) else { continue };
            let mut found = Vec::new();
            walk(&ast.items, &mut found);
            scanned += found.len();
            if path.file_name().is_some_and(|n| n == "error.rs") {
                continue;
            }
            for ident in found {
                holes.insert(format!("{ident} is declared by {}", relative(&path, &root),));
            }
        }
    }
    baseline::floor(scanned, floors::ERROR_TYPES, "error types");
    baseline::gate(
        ERRORS_BASELINE,
        &holes,
        scanned,
        "error types",
        "error types declared outside `error.rs`",
        "an error type filed beside the code that raises it — move it to the \
         module's `error.rs`",
    );
}

/// **A root file of an adapter crate serves several bindings, or the root
/// itself** — `architecture.md`: *what several bindings share sits at the root —
/// and only that.* A root file one binding folder alone reaches reads as shared
/// and is not; it belongs in that binding's folder.
///
/// "Reaches" is read off identifiers: a binding's files naming the root file's
/// module (`crate::key::…`) or an item it declares. A root file another root file
/// also uses — `module.rs` opening the connection — is the root's own, whatever
/// the bindings do. Framework crates with two binding folders or more, since a
/// crate with one has nothing to share.
#[test]
fn a_root_file_of_an_adapter_crate_serves_more_than_one_binding() {
    const ROOT_OWN: [&str; 3] = ["lib.rs", "error.rs", "testing.rs"];
    let root = repo_root();
    let framework = root.join("crates");
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;
    for krate in crate_dirs() {
        if !krate.starts_with(&framework) {
            continue;
        }
        let src = krate.join("src");
        let files = rust_files(&src);
        let bindings: BTreeSet<String> = files
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "module.rs"))
            .filter_map(|p| p.parent())
            .filter(|d| *d != src)
            .filter_map(|d| d.file_name().and_then(|n| n.to_str()))
            .map(str::to_owned)
            .collect();
        if bindings.len() < 2 {
            continue;
        }
        let parsed_files: Vec<(std::path::PathBuf, BTreeSet<String>, syn::File)> = files
            .iter()
            .filter_map(|p| parsed(p).map(|ast| (p.clone(), idents_in(&ast), ast)))
            .collect();
        for (path, _, ast) in &parsed_files {
            if path.parent() != Some(src.as_path()) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if ROOT_OWN.contains(&name) {
                continue;
            }
            let stem = name.trim_end_matches(".rs").to_owned();
            let mut declared: BTreeSet<String> = ast
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Struct(i) => Some(i.ident.to_string()),
                    Item::Enum(i) => Some(i.ident.to_string()),
                    Item::Trait(i) => Some(i.ident.to_string()),
                    Item::Fn(i) => Some(i.sig.ident.to_string()),
                    Item::Const(i) => Some(i.ident.to_string()),
                    Item::Static(i) => Some(i.ident.to_string()),
                    Item::Type(i) => Some(i.ident.to_string()),
                    _ => None,
                })
                .filter(|ident| ident != "TARGET")
                .collect();
            declared.insert(stem.clone());
            scanned += 1;
            let mut reached_by = BTreeSet::new();
            let mut reached_at_root = false;
            for (other, idents, _) in &parsed_files {
                if other == path || other.file_name().is_some_and(|n| n == "lib.rs") {
                    continue;
                }
                if !declared.iter().any(|d| idents.contains(d)) {
                    continue;
                }
                match binding_of(other, &src, &bindings) {
                    Some(binding) => {
                        reached_by.insert(binding);
                    }
                    None => reached_at_root = true,
                }
            }
            if reached_by.len() == 1 && !reached_at_root {
                holes.insert(format!(
                    "{} is reached by {}/ alone",
                    relative(path, &root),
                    reached_by
                        .iter()
                        .next()
                        .map(String::as_str)
                        .unwrap_or_default(),
                ));
            }
        }
    }
    baseline::floor(scanned, floors::ADAPTER_ROOT_FILES, "adapter root files");
    baseline::gate(
        ROOT_FILES_BASELINE,
        &holes,
        scanned,
        "adapter root files",
        "root files one binding alone reaches",
        "a file that reads as shared and serves one binding — move it into that \
         binding's folder",
    );
}

/// **`Config` names a `#[config]`, and nothing else** — `architecture.md`,
/// *Configuration*. A public struct ending in `Config` is a `#[config]`, or sits
/// in a `config.rs` (a config assembled by hand, such as one resolved before the
/// container exists); anything else is a settings struct a config nests, and takes
/// the crate's subject and the file's kind instead (`HttpTls`, `RedisTls`).
/// Framework crates only: a product names its own vocabulary.
#[test]
fn config_names_a_config() {
    let root = repo_root();
    let framework = root.join("crates");
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;
    for krate in crate_dirs() {
        if !krate.starts_with(&framework) {
            continue;
        }
        for path in rust_files(&krate.join("src")) {
            let Some(ast) = parsed(&path) else { continue };
            let in_config_rs = path.file_name().is_some_and(|n| n == "config.rs");
            for i in structs_at_any_depth(&ast.items) {
                if !matches!(i.vis, syn::Visibility::Public(_)) {
                    continue;
                }
                let ident = i.ident.to_string();
                if !ident.ends_with("Config") {
                    continue;
                }
                scanned += 1;
                let declared = nest_rs_conformance::sources::config_namespace(&i.attrs).is_some();
                if !declared && !in_config_rs {
                    holes.insert(format!(
                        "{ident} is declared by {} and is no `#[config]`",
                        relative(&path, &root),
                    ));
                }
            }
        }
    }
    baseline::floor(scanned, floors::CONFIGS, "public `*Config` structs");
    baseline::gate(
        CONFIG_NAMES_BASELINE,
        &holes,
        scanned,
        "public `*Config` structs",
        "`*Config` structs that are no `#[config]`",
        "a settings struct wearing a config's suffix — name it for the crate's \
         subject and its file's kind",
    );
}

/// The binding folder a path sits under, or `None` when it sits at the crate
/// root or under something that declares no module.
fn binding_of(path: &Path, src: &Path, bindings: &BTreeSet<String>) -> Option<String> {
    let first = segments(path, src).into_iter().next()?;
    bindings.contains(&*first).then(|| first.into_owned())
}

/// Every identifier the file's tokens carry. A token walk rather than a text
/// scan, so a name inside a string literal — which is all a CLI template holds —
/// is not a reference.
fn idents_in(file: &syn::File) -> BTreeSet<String> {
    fn walk(stream: proc_macro2::TokenStream, out: &mut BTreeSet<String>) {
        for tree in stream {
            match tree {
                proc_macro2::TokenTree::Ident(i) => {
                    out.insert(i.to_string());
                }
                proc_macro2::TokenTree::Group(g) => walk(g.stream(), out),
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(quote::ToTokens::to_token_stream(file), &mut out);
    out
}

/// **A `#[config]`'s namespace is its stem, exactly as its type name is — read,
/// never chosen.**
///
/// The law in `architecture.md`: the segments are the crate's subject, then
/// every binding folder below `src/` on the way to the file, joined by `__` —
/// the same derivation that names the type. `http/src/config.rs` → `http`;
/// `seaorm/src/config.rs` → `seaorm`; `redis/src/worker/config.rs` →
/// `redis__worker`; `social/src/providers/github/config.rs` → `social__github`,
/// because a pluralised role folder is not a segment exactly as it is not a
/// word of the type. From a variable a reader knows the type, the file and the
/// module that reads it; from a module they know the variable.
///
/// Two defects this test exists for: `NESTRS_QUEUE__URL`, the one Redis
/// connection three bindings share, filed and configured under whichever asked
/// first; and `NESTRS_DATABASE__URL`, the universal convention, which named
/// neither the crate nor the type that parsed it — from the variable, a reader
/// could not find the code.
///
/// Framework crates only, for the reason the bindings gate gives: a product's
/// top-level folders are domains, and a product's namespace is the product's
/// own decision. Shipped declarations only — a `#[config]` inside a `#[cfg(test)]`
/// module is a fixture, and this walks top-level items.
/// The pluralised role folders of `architecture.md` (*Several of the same role*)
/// plus `providers/`, the folder a selection-by-configuration family keeps its
/// members in. A role folder groups files of one role; it is not a level of the
/// name, so it is not a segment of the namespace either. `events` is
/// deliberately absent although the role table lists it: the word is also an
/// edge folder, and an edge's config owes its segment — dropping it would give
/// an `events/` adapter's namespace one word fewer than its type.
const PLURAL_ROLE_FOLDERS: [&str; 7] = [
    "services",
    "entities",
    "dtos",
    "commands",
    "strategies",
    "pipes",
    "providers",
];

/// The leading segments of a crate's namespace: its subject, folded — or, for a
/// member of a family, the family and the member as two levels, read off the one
/// `TARGET` the crate declares (`nest_rs::oauth::client` → `oauth`, `client`), so
/// the variable, the span target and the path a caller types say one string.
fn subject_segments(krate: &str, subject: &str) -> Vec<String> {
    let own = nest_rs_conformance::sources::declared_targets()
        .iter()
        .filter(|(_, owner, konst)| *owner == krate && *konst == "TARGET")
        .filter_map(|(target, _, _)| target.strip_prefix("nest_rs::"))
        .find(|tail| tail.contains("::"));
    match own {
        Some(tail) => tail.split("::").map(folded).collect(),
        None => vec![folded(subject)],
    }
}

#[test]
fn namespace_is_the_stem() {
    let root = repo_root();
    let framework = root.join("crates");
    let mut holes = BTreeSet::new();
    let mut scanned = 0usize;

    for krate in crate_dirs() {
        if !krate.starts_with(&framework) {
            continue;
        }
        let Some(name) = krate.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let subject = subject_of(name);
        let src = krate.join("src");
        for path in rust_files(&src) {
            let Some(ast) = parsed(&path) else { continue };
            let parts = segments(&path, &src);
            let folders = &parts[..parts.len() - 1];
            for item in &ast.items {
                let Item::Struct(s) = item else { continue };
                let Some(namespace) = nest_rs_conformance::sources::config_namespace(&s.attrs)
                else {
                    continue;
                };
                scanned += 1;
                let expected: Vec<String> = subject_segments(name, subject)
                    .into_iter()
                    .chain(
                        folders
                            .iter()
                            .map(|f| &**f)
                            .filter(|f| !PLURAL_ROLE_FOLDERS.contains(f))
                            .map(folded),
                    )
                    .collect();
                let declared: Vec<String> = namespace.split("__").map(folded).collect();
                if declared != expected {
                    holes.insert(format!(
                        "`{namespace}` is declared by {} — its stem says `{}`",
                        relative(&path, &root),
                        expected.join("__"),
                    ));
                }
            }
        }
    }

    baseline::floor(scanned, floors::CONFIGS, "shipped `#[config]` structs");
    baseline::gate(
        NAMESPACE_BASELINE,
        &holes,
        scanned,
        "shipped `#[config]` structs",
        "namespaces that disagree with their stem",
        "a namespace chosen rather than read off the path — move the config to \
         where its word belongs, or take the word of where it sits",
    );
}

/// **No namespace of the tree is a near miss of another**, by the rule the
/// loader's report applies.
///
/// A binary reports a variable whose namespace is a near miss of one it links
/// (`nest_rs_config::unclaimed::is_near_miss`), and stays silent on any other,
/// since one `.env` serves several binaries. That silence is only safe while no
/// two namespaces a deployment may set side by side are near misses of each
/// other: the demo's `api` would otherwise report its `worker`'s variables as
/// typos. Derived over every `#[config]` in both workspaces and judged by the
/// report's own function, so the rule shipped and the rule met are one symbol.
#[test]
fn no_namespace_is_a_near_miss_of_another() {
    let root = repo_root();
    let mut declared: Vec<(String, String)> = Vec::new();
    for krate in crate_dirs() {
        for path in rust_files(&krate.join("src")) {
            let Some(ast) = parsed(&path) else { continue };
            for item in &ast.items {
                let Item::Struct(s) = item else { continue };
                if let Some(namespace) = nest_rs_conformance::sources::config_namespace(&s.attrs) {
                    declared.push((namespace, relative(&path, &root)));
                }
            }
        }
    }
    baseline::floor(
        declared.len(),
        floors::TREE_NAMESPACES,
        "`#[config]` namespaces in both workspaces",
    );
    let mut near = BTreeSet::new();
    for (i, (a, at)) in declared.iter().enumerate() {
        for (b, bt) in &declared[i + 1..] {
            if a != b && nest_rs_config::unclaimed::is_near_miss(a, b) {
                near.insert(format!("`{a}` ({at}) and `{b}` ({bt})"));
            }
        }
    }
    assert!(
        near.is_empty(),
        "namespaces the unclaimed-variable report would read as one misspelled for the other — \
         a binary linking one would report the other's variables; rename one:\n{}",
        near.into_iter().collect::<Vec<_>>().join("\n"),
    );
}

/// **No verdict here depends on where the checkout sits.**
///
/// A join judges the tree, and the directories above the repository root are not
/// part of it. Three of the joins above, and the dispatch vocabulary one of them
/// reads, once took `path.components()` of the absolute path, so a clone under
/// `~/src/` put every folder inside a `src/` tree, and a clone under a folder
/// named like an edge put every file with no edge folder of its own in that
/// edge's folder. Each now reads `sources::segments`, below the root.
///
/// Proved on the joins' own code rather than on the helper alone: one small tree
/// is planted twice — under a plain root and under one spelling both words,
/// `…/src/schedule/nestrs` — and every path-reading verdict of this module is
/// taken over each, `nestrs lint`'s scan included since
/// [`every_file_is_named_for_what_it_declares`] runs it over the same tree. Both
/// must equal the verdict written below, so the test is not satisfied by two
/// readings that are wrong the same way. Each decoy is a file the absolute
/// reading judged differently; the comment beside it says how.
#[test]
fn no_verdict_depends_on_where_the_checkout_sits() {
    const TREE: [(&str, &str); 13] = [
        // The dispatch vocabulary: an HTTP marker and a WS one, both source.
        (
            "crates/nest-rs-probe/src/lib.rs",
            "pub trait HttpProbe { fn check_http(&self); }\n\
             pub trait WsProbe { fn check_ws_message(&self); }",
        ),
        // A guard answering both edges from where both can reach it. Under
        // `…/schedule/…` it read as sitting in `schedule/`.
        (
            "crates/nest-rs-probe/src/guard.rs",
            "pub struct ProbeGuard;\n\
             impl HttpProbe for ProbeGuard { fn check_http(&self) {} }\n\
             impl WsProbe for ProbeGuard { fn check_ws_message(&self) {} }",
        ),
        // The one real edge-folder offender: under `http/`, answering WS.
        (
            "crates/nest-rs-probe/src/http/guard.rs",
            "pub struct HttpOnlyGuard;\n\
             impl WsProbe for HttpOnlyGuard { fn check_ws_message(&self) {} }",
        ),
        // Answers a method only a suite declares. Under `~/src/` the suite's
        // trait read as source, and this file as answering GraphQL.
        (
            "crates/nest-rs-probe/src/http/bridge.rs",
            "pub struct Bridge;\nimpl Bridge { pub fn check_graphql(&self) {} }",
        ),
        (
            "crates/nest-rs-probe/tests/integration/vocabulary.rs",
            "pub trait GraphqlProbe { fn check_graphql(&self); }",
        ),
        // A suite's fixture under an edge folder, out of population — until
        // `~/src/` put it in a `src/` tree.
        (
            "crates/nest-rs-probe/tests/integration/http/fixture.rs",
            "pub struct FixtureGuard;\n\
             impl WsProbe for FixtureGuard { fn check_ws_message(&self) {} }",
        ),
        // A suite's `common/`, which the invented-folder rule does not reach —
        // until `~/src/` — and a module's `shared/`, which it does.
        (
            "crates/nest-rs-probe/tests/integration/common/mod.rs",
            "pub fn fixture() {}",
        ),
        (
            "crates/nest-rs-probe/src/shared/mod.rs",
            "pub fn helper() {}",
        ),
        // An adapter named for its module, and one named for another.
        (
            "crates/nest-rs-probe/src/users/http/controller.rs",
            "pub struct UsersController;",
        ),
        (
            "crates/nest-rs-probe/src/posts/http/controller.rs",
            "pub struct UsersController;",
        ),
        // A stem reaching nothing it declares, in source and in a suite:
        // `nestrs lint` judges source only.
        (
            "crates/nest-rs-probe/src/principal.rs",
            "pub struct DeskOperator;",
        ),
        (
            "crates/nest-rs-probe/tests/integration/principal.rs",
            "pub struct DeskOperator;",
        ),
        // An app's root holding an edge: refused where it sits, and read off
        // `demo/` below the root — under `…/src/schedule/…` a reading of the
        // absolute path would have found a source root above every file.
        (
            "demo/apps/probe/src/http/controller.rs",
            "pub struct ProbeController;",
        ),
    ];

    #[derive(Debug, PartialEq)]
    struct Verdicts {
        adapters: (usize, Vec<String>),
        placed: (usize, Vec<String>),
        invented: Vec<String>,
        edges: EdgeFolders,
        lint_checked: usize,
        lint_findings: Vec<std::path::PathBuf>,
    }
    fn verdicts(root: &Path) -> Verdicts {
        let lint = nest_rs_cli::lint::scan(root);
        Verdicts {
            adapters: misnamed_adapters(root),
            placed: edges_at_a_product_root(root),
            invented: invented_folders(root),
            edges: edge_folders(root),
            lint_checked: lint.checked,
            lint_findings: lint.findings.into_iter().map(|f| f.path).collect(),
        }
    }

    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("naming-checkout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let plain = scratch.join("plain/nestrs");
    let hostile = scratch.join("src/schedule/nestrs");
    for root in [&plain, &hostile] {
        crate::plant(root, &TREE);
    }
    let (from_plain, from_hostile) = (verdicts(&plain), verdicts(&hostile));
    let _ = std::fs::remove_dir_all(&scratch);

    let expected = Verdicts {
        adapters: (
            2,
            vec!["UsersController in crates/nest-rs-probe/src/posts/http/controller.rs".to_owned()],
        ),
        placed: (
            1,
            vec![
                "demo/apps/probe/src/http — an edge adapter belongs to a module folder: move it \
                 to `src/<module>/http/`, named for the module it adapts; the app name stops at \
                 `ProbeModule`, so nothing below it adapts the app"
                    .to_owned(),
            ],
        ),
        invented: vec!["crates/nest-rs-probe/src/shared".to_owned()],
        edges: EdgeFolders {
            markers: 2,
            entries: 2,
            scanned: 5,
            offenders: vec![
                "crates/nest-rs-probe/src/http/guard.rs sits under http/ and answers \
                 fn check_ws_message() (ws), impl WsProbe (ws)"
                    .to_owned(),
            ],
        },
        lint_checked: 1,
        lint_findings: vec!["crates/nest-rs-probe/src/principal.rs".into()],
    };
    assert_eq!(
        from_plain, expected,
        "the plain root read the planted tree wrong"
    );
    assert_eq!(
        from_hostile, expected,
        "a root spelling `src` and an edge changed a verdict — a join is reading \
         above the repository root",
    );
}
