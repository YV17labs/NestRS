//! Where a check reads its population from: the two workspaces' crate
//! directories, their files, and the constants they declare — walked from the
//! tree, never listed.

use std::path::{Path, PathBuf};
use std::{fs, io};

use proc_macro2::{Spacing, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;
use syn::{Attribute, Expr, Item, ItemFn, ItemMod, Lit, Meta};

/// The repository root, from this crate's own manifest — so a join is run from
/// anywhere and reads the same tree.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in crates/")
        .parent()
        .expect("crates/ sits at the repo root")
        .to_path_buf()
}

/// Every `.rs` under `dir`, `target/` excluded. A directory that does not exist
/// yields nothing: a join names its own floor rather than relying on this to
/// fail.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    files_with_extension(dir, "rs")
}

/// The same walk for any extension — a join reads `.stderr` snapshots and `.mdx`
/// pages the way it reads Rust, and the `target/` skip is the part a per-join
/// copy leaves out.
pub fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(
        dir,
        &|path| path.extension().is_some_and(|e| e == extension),
        &mut out,
    );
    out
}

/// The same walk by whole file name, for the files an operator reads that carry
/// no extension — a `Justfile`, a `Dockerfile`.
pub fn files_with_name(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(
        dir,
        &|path| path.file_name().is_some_and(|n| n == name),
        &mut out,
    );
    out
}

fn collect(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, keep, out);
        } else if keep(&path) {
            out.push(path);
        }
    }
}

/// The part of `path` below `base` — the only part of a path a verdict may read.
///
/// **Never the absolute path.** A join that reads `path.components()` reads
/// where the clone happens to sit as well as where the file sits in the repo,
/// and the two are indistinguishable once they are one list: cloned under
/// `~/src/`, every folder in the tree reads as being inside a `src/` tree, and
/// cloned under a folder named like an edge (`…/schedule/nestrs`), every file
/// with no edge folder of its own reads as that edge's adapter. Both shipped:
/// the edge-folder join gave another verdict on a machine whose checkout path
/// held `src` or one of the seven edge words. `base` is the repository root for a
/// repo-relative reading, or a crate's `src/` for a module-relative one; either
/// way the answer depends on the tree and nothing above it.
///
/// **Panics when `path` is not below `base`**, and that is the point rather than
/// a shortcut: every population a join reads is walked from the root, so a path
/// outside it is a join reading the wrong tree. The fallback `relative` used to
/// take — the absolute path, whole — is exactly the reading this function exists
/// to refuse, so it is not offered as a quiet alternative.
fn below<'p>(path: &'p Path, base: &Path) -> &'p Path {
    path.strip_prefix(base).unwrap_or_else(|_| {
        panic!(
            "{} is not below {} — a join read a path from outside the tree it walks",
            path.display(),
            base.display(),
        )
    })
}

/// The folders and file name of `path` below `base`, in order — what a join
/// matches a layout word against (`src`, an edge, `diagnostics`).
///
/// The one door to a path's components in this crate: `below` first, always.
/// A component that is not UTF-8 is kept in its lossy form rather than dropped,
/// so every index still names the level it did — and a lossy name can never
/// equal a layout word, which are all ASCII, so the verdict is the one the real
/// name would get.
pub fn segments<'p>(path: &'p Path, base: &Path) -> Vec<std::borrow::Cow<'p, str>> {
    below(path, base)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect()
}

/// The path as the repo spells it, for a message a reader can paste into `rg` —
/// and for a join that classifies on the spelling. Read through `below`, so a
/// message and a verdict can never be taken from two different readings.
pub fn relative(path: &Path, root: &Path) -> String {
    below(path, root).display().to_string()
}

/// A file parsed as Rust, or `None` when it is not (a fixture that must not
/// compile, a generated stub). A join reads Rust with Rust's parser: a
/// `\`-continued literal and a `#[cfg(test)]` block are the two things a text
/// scan reads wrong, and both cost a false reading here before this crate.
pub fn parsed(path: &Path) -> Option<syn::File> {
    let text = fs::read_to_string(path).ok()?;
    syn::parse_file(&text).ok()
}

/// Read a file, propagating the failure — used where absence is a bug rather
/// than a case.
pub fn read(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// The value of a `pub const <name>: &str = "…";` in one file — free, or
/// associated on an `impl`.
///
/// **Parsed, not grepped.** The declarations this crate reads are string
/// constants, and a text scan reads two of them wrong: a `\`-continued literal
/// and a `#[cfg(test)]` fixture spelling the same name. [`parsed`] exists for
/// exactly that, and the alternative — linking the crate that declares it — is
/// a relink of most of the workspace to read one string.
pub fn declared_str(path: &Path, name: &str) -> Option<String> {
    let ast = parsed(path)?;
    // Top level first, so a free constant always wins a name an `impl` also
    // uses. The associated arm reads a value that lives where the type owning
    // it does — `EnvPrefix::DEFAULT` — which is the placement the naming rules
    // ask for.
    ast.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Const(konst) if konst.ident == name => as_str_lit(&konst.expr),
            _ => None,
        })
        .or_else(|| {
            ast.items.iter().find_map(|item| {
                let syn::Item::Impl(block) = item else {
                    return None;
                };
                block.items.iter().find_map(|item| match item {
                    syn::ImplItem::Const(konst) if konst.ident == name => as_str_lit(&konst.expr),
                    _ => None,
                })
            })
        })
}

/// Every crate directory the repo's two workspaces hold.
///
/// Read from the tree rather than from `cargo metadata`, and the three roots are
/// the ones `CLAUDE.md` pins by name — `crates/` for the framework, `demo/apps/`
/// and `demo/crates/` for the product, all three under *No collapsing the two
/// workspaces*. A member is a child directory carrying a `Cargo.toml`, which is
/// what the `members = ["crates/*"]` globs mean.
pub fn crate_dirs() -> Vec<PathBuf> {
    let root = repo_root();
    let mut out = Vec::new();
    for workspace in ["crates", "demo/apps", "demo/crates"] {
        let Ok(entries) = fs::read_dir(root.join(workspace)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("Cargo.toml").is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The umbrella's `[features]` table, as declared.
///
/// **One parse, two named views, because "capability" means two things and the
/// repo had never said which.** A *feature* is what a developer types after
/// `--features`; a *crate* is what one activates. They numbered the same for as
/// long as every capability feature activated exactly one crate, so nothing
/// forced the distinction — then `seaorm` grew a second `dep:` (`#[expose]`
/// expands to one crate and `#[crud]` to the other, and two features would have
/// to imply each other, which Cargo rejects as a cycle) and `redis-throttler`
/// arrived activating none at all. Two readers, two answers, one word.
///
/// So neither reading is left to a call site to re-derive:
/// [`UmbrellaMatrix::features`] is the developer's set — what the landing counts
/// and the docs' packages table maps — and [`UmbrellaMatrix::crates`] is the set
/// of crates whose README owes an install line, which the `canon` binary
/// publishes for the docs linter.
///
/// Parsed with a TOML parser rather than scanned: a feature list wraps across
/// lines as freely as a Rust string does, and the wrapping is what a
/// line-oriented read gets wrong.
pub struct UmbrellaMatrix {
    /// Every feature the table declares, in declaration order, mapped to the
    /// entries it activates.
    entries: Vec<(String, Vec<String>)>,
}

/// Features that activate nothing of their own and are documented as
/// aggregates, so neither view counts them.
pub const UMBRELLA_AGGREGATES: [&str; 2] = ["default", "full"];

impl UmbrellaMatrix {
    /// Every capability a developer can name in `--features`.
    ///
    /// The aggregates aside, that is the whole table: a feature exists to be
    /// typed, and one activating no `dep:` of its own is still a capability when
    /// it forwards a surface that exists in no build without it —
    /// `redis-throttler` is exactly that, and the manifest's own comment calls it
    /// one. Counting `dep:`-bearing features instead is the derivation the docs
    /// linter used to run, and it disagreed with this file by one.
    pub fn features(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| !UMBRELLA_AGGREGATES.contains(&name.as_str()))
            .collect()
    }

    /// Every `(feature, crate)` the table activates outright.
    ///
    /// `nest-rs-x/y` and `nest-rs-x?/y` are excluded: both only forward a
    /// feature to a crate some *other* feature activates, so neither makes this
    /// feature the owner of a crate.
    pub fn crates(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .flat_map(|(feature, entries)| {
                entries.iter().filter_map(move |entry| {
                    entry
                        .strip_prefix("dep:")
                        .map(|krate| (feature.clone(), krate.to_owned()))
                })
            })
            .collect()
    }

    /// The entries one feature activates, for a join asking about a single row.
    pub fn entries_of(&self, feature: &str) -> &[String] {
        self.entries
            .iter()
            .find(|(name, _)| name == feature)
            .map_or(&[], |(_, entries)| entries.as_slice())
    }
}

/// Read the umbrella's feature matrix, or an empty one when the manifest has no
/// `[features]` table — a floor at the call site names that, rather than this
/// returning a shape nobody can tell from a real one.
pub fn umbrella_matrix(root: &Path) -> UmbrellaMatrix {
    let Ok(manifest) = read(&root.join("crates/nest-rs/Cargo.toml")) else {
        return UmbrellaMatrix {
            entries: Vec::new(),
        };
    };
    let Ok(doc) = manifest.parse::<toml_edit::DocumentMut>() else {
        return UmbrellaMatrix {
            entries: Vec::new(),
        };
    };
    let Some(features) = doc.get("features").and_then(|f| f.as_table()) else {
        return UmbrellaMatrix {
            entries: Vec::new(),
        };
    };
    let mut entries = Vec::new();
    for (feature, value) in features {
        let Some(list) = value.as_array() else {
            continue;
        };
        entries.push((
            feature.to_owned(),
            list.iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect(),
        ));
    }
    UmbrellaMatrix { entries }
}

/// The decorators a crate exports, or empty when it exports none.
///
/// Rust forces `#[proc_macro_attribute]` items to the crate root, so `lib.rs` is
/// the whole surface — the one place this join has to read, and the reason a
/// proc-macro crate is recognised by what it contains rather than by its name.
/// Attribute macros only, and deliberately: the framework exports 27 of them and
/// zero derives, so a `proc_macro_derive` arm here would be a member list for a
/// family that does not exist — and an *unjoinable* one, since the umbrella
/// join records an applied attribute's last path segment and a derive is applied
/// as `#[derive(X)]`, whose path is `derive`. Any derive it returned would be a
/// cell nothing could ever fill.
///
/// **`#[proc_macro]` is refused on that same argument**, which the arm used to
/// contradict while the paragraph above made the case against it. A bang macro
/// is invoked `name!(…)` and the umbrella join records an *applied attribute's*
/// path, so a bang macro returned here opens a cell nothing can fill — and
/// `baseline.rs` guarantees the only way back out is a permanent line in a file
/// documented as one that only shrinks. Zero are exported today, so this closed
/// a latent hole rather than a live one.
pub fn exported_decorators(dir: &Path) -> Vec<String> {
    let Some(ast) = parsed(&dir.join("src/lib.rs")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in &ast.items {
        let Item::Fn(f) = item else {
            continue;
        };
        for attr in &f.attrs {
            let path = attr.path();
            if path.is_ident("proc_macro_attribute") {
                out.push(f.sig.ident.to_string());
            }
        }
    }
    out
}

fn as_str_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

/// Every span target the framework declares, read out of the declaration.
///
/// **Derived, never listed**, which is this crate's whole posture and was worth
/// insisting on here: the alternative was a `match` naming every crate plus a
/// path dev-dependency on each, and a crate added later joins the check only
/// when someone remembers both. It also cost the test binary 400 crates and a
/// 100 MB relink to read twenty `&'static str`s.
///
/// The convention the framework now follows is what makes this mechanical: a
/// crate owning one concern writes `pub const TARGET` at its root, a crate
/// owning several writes a `pub mod target` of them. Both are `Item::Const`
/// with a string literal, so both are read the same way.
///
/// Returns `(target, declaring crate directory, constant name)`. The name is
/// what disambiguates: `nest_rs_core` declares seven, so the crate alone cannot
/// say which of them `…::operation_log::TARGET` is.
///
/// **Walked once per test process**, because several checks read the same
/// table. Leaked, deliberately: the table is the process's, and every caller
/// compares against a borrowed `&'static str`.
pub fn declared_targets() -> &'static [(&'static str, &'static str, &'static str)] {
    static TABLE: std::sync::OnceLock<Vec<(&'static str, &'static str, &'static str)>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut out = Vec::new();
        for dir in crate_dirs() {
            let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            for file in rust_files(&dir.join("src")) {
                let Some(ast) = parsed(&file) else {
                    continue;
                };
                collect_target_consts(&ast.items, name, &mut out);
            }
        }
        out.into_iter()
            .map(|(target, krate, konst)| {
                let leak = |s: String| &*Box::leak(s.into_boxed_str());
                (leak(target), leak(krate), leak(konst))
            })
            .collect()
    })
}

/// The namespace a `#[config(namespace = "…")]` declares, or `None` for any
/// other struct.
///
/// One reader, because there were two and they had already drifted: the copy
/// that did not consume a *valued* sibling key (`validate = "manual"`) left the
/// nested-meta walk in an error state, so a `namespace` written after one was
/// silently reported absent.
///
/// The attribute is known by its path's **last segment**: `#[nest_rs::config]`
/// is the same decorator as `#[config]`, and reading the bare ident alone took a
/// qualified one for no config at all.
pub fn config_namespace(attrs: &[syn::Attribute]) -> Option<String> {
    let attr = attrs.iter().find(|a| {
        a.path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "config")
    })?;
    let mut namespace = None;
    #[expect(
        clippy::disallowed_methods,
        reason = "a scanner reading `#[config]` as text, not a decorator's grammar"
    )]
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("namespace") {
            let lit: syn::LitStr = meta.value()?.parse()?;
            namespace = Some(lit.value());
        } else if meta.input.peek(syn::Token![=]) {
            let _: syn::Expr = meta.value()?.parse()?;
        }
        Ok(())
    });
    namespace
}

/// Every canonical unit-of-work name the framework declares.
///
/// The same shape [`declared_targets`] reads, for the other per-edge vocabulary:
/// a unit name is the edge's, not the kernel's, so it is declared by the crate
/// that owns the edge as `pub const X: &str` inside that crate's `unit` module —
/// a `src/unit.rs` file, or an inline `mod unit`. Reading the declarations is
/// what lets the shape and namespace checks be *derived*; the same rule was a
/// hand-written array in `nest-rs-core` for as long as the kernel held the
/// names, and such a list can only ever police what whoever typed it remembered.
///
/// Returns `(unit name, declaring crate directory, constant name)`.
pub fn declared_units() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for dir in crate_dirs() {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for file in rust_files(&dir.join("src")) {
            let Some(ast) = parsed(&file) else {
                continue;
            };
            let in_unit_file = file.file_name().and_then(|n| n.to_str()) == Some("unit.rs");
            collect_unit_consts(&ast.items, name, in_unit_file, &mut out);
        }
    }
    out
}

/// A `pub const X: Unit = unit!("…", …);` inside a `unit` module — which is either the
/// whole of a `src/unit.rs` file or an inline `mod unit`. Nowhere else, so that
/// a name declared off the convention is reported rather than quietly accepted.
fn collect_unit_consts(
    items: &[Item],
    krate: &str,
    inside: bool,
    out: &mut Vec<(String, String, String)>,
) {
    for item in items {
        match item {
            Item::Const(konst) if inside => {
                if let Some(name) = unit_name(&konst.expr) {
                    out.push((name, krate.to_owned(), konst.ident.to_string()));
                }
            }
            Item::Mod(module) if module.ident == "unit" => {
                if let Some((_, inner)) = &module.content {
                    collect_unit_consts(inner, krate, true, out);
                }
            }
            _ => {}
        }
    }
}

/// The name a unit constant declares: the first string literal of its
/// `nest_rs_core::unit!("<edge>.<unit>", …)` declaration.
fn unit_name(expr: &Expr) -> Option<String> {
    let Expr::Macro(call) = expr else {
        return None;
    };
    if call.mac.path.segments.last()?.ident != "unit" {
        return None;
    }
    call.mac
        .tokens
        .clone()
        .into_iter()
        .find_map(|tree| match tree {
            TokenTree::Literal(lit) => match syn::parse_str::<Lit>(&lit.to_string()) {
                Ok(Lit::Str(text)) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
}

/// A `pub const X: &str = "nest_rs::…";`, at an item list's top level or one
/// `mod target` down. Only those two depths, because those are the two shapes
/// the convention permits — a target declared anywhere else is meant to be
/// invisible here, so that it is reported rather than quietly accepted.
fn collect_target_consts(items: &[Item], krate: &str, out: &mut Vec<(String, String, String)>) {
    for item in items {
        match item {
            Item::Const(konst) => {
                if let Expr::Lit(lit) = &*konst.expr
                    && let Lit::Str(text) = &lit.lit
                    && text.value().starts_with("nest_rs::")
                {
                    out.push((text.value(), krate.to_owned(), konst.ident.to_string()));
                }
            }
            Item::Mod(module) if module.ident == "target" => {
                if let Some((_, inner)) = &module.content {
                    collect_target_consts(inner, krate, out);
                }
            }
            _ => {}
        }
    }
}

/// Whether an item is compiled for tests only — a `#[cfg(…)]` whose predicate
/// implies `test`.
///
/// Shared rather than per join: it separates the two things a `src/` file holds
/// — the framework's own emissions, and the assertions about them — and every
/// join needs one side or the other.
///
/// **Implies, not mentions.** It answered "the predicate names `test`
/// somewhere", so `#[cfg(not(test))]` — shipped code, *excluded* from tests —
/// and `#[cfg(any(test, feature = "testing"))]` — shipped under a feature — read
/// as fixtures, and every join skipped what they hold. `test` implies itself,
/// `all(…)` implies it when one of its terms does, `any(…)` when every term
/// does, and `not(…)` never does.
pub fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| match &attr.meta {
        Meta::List(list) if list.path.is_ident("cfg") => list
            .parse_args::<Meta>()
            .is_ok_and(|predicate| implies_test(&predicate)),
        _ => false,
    })
}

/// Whether a `cfg` predicate holds only when `test` does.
fn implies_test(predicate: &Meta) -> bool {
    match predicate {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) => {
            let Ok(terms) = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return false;
            };
            if list.path.is_ident("all") {
                terms.iter().any(implies_test)
            } else if list.path.is_ident("any") {
                !terms.is_empty() && terms.iter().all(implies_test)
            } else {
                false
            }
        }
        Meta::NameValue(_) => false,
    }
}

/// Whether `tokens` spell the path `<first>::<second>` anywhere, at any depth.
///
/// Tokens rather than `syn::visit` over expressions, because the wiring a boot
/// test declares lives inside `#[module(imports = [HttpModule::for_root(cfg)])]`
/// — an attribute's contents are tokens, and an expression visitor never sees
/// them. Tokens see both spellings.
///
/// A mention inside a doc comment or a string literal never matches: by the time
/// the lexer is done both are a single `Literal`, which is why
/// `crates/nest-rs-ws/src/module.rs`'s doc example does not read as a boot.
pub fn spells_path(tokens: &TokenStream, first: &str, second: &str) -> bool {
    let mut flat = Vec::new();
    flatten(tokens.clone(), &mut flat);
    flat.windows(4).any(|w| match (&w[0], &w[1], &w[2], &w[3]) {
        (TokenTree::Ident(a), TokenTree::Punct(p), TokenTree::Punct(q), TokenTree::Ident(b)) => {
            a == first && p.as_char() == ':' && q.as_char() == ':' && b == second
        }
        _ => false,
    })
}

/// Every identifier `tokens` uses as a path root — `<ident>::…`.
///
/// A bare ident is not enough to say a test reaches into a crate: `migrations`
/// is a local binding as readily as it is a crate. The `::` is what makes the
/// spelling unambiguous, and it is how a test names a crate it did not itself
/// declare a `mod` for.
pub fn path_roots(tokens: &TokenStream) -> Vec<String> {
    let mut flat = Vec::new();
    flatten(tokens.clone(), &mut flat);
    flat.windows(3)
        .filter_map(|w| match (&w[0], &w[1], &w[2]) {
            // Both colons, and the first `Joint` — that is what `::` is as
            // tokens, and what a single `:` is not. Matching one colon accepted
            // `name:` from a struct literal, a field init, a type ascription or
            // a `tracing` field, so every crate whose directory shares a name
            // with a common binding (`api`, `auth`, `worker`, `config`) read as
            // reached with no test behind it.
            (TokenTree::Ident(ident), TokenTree::Punct(first), TokenTree::Punct(second))
                if first.as_char() == ':'
                    && first.spacing() == Spacing::Joint
                    && second.as_char() == ':' =>
            {
                Some(ident.to_string())
            }
            _ => None,
        })
        .collect()
}

/// Every token at every depth, groups kept *and* descended into.
///
/// Both halves matter: a join keying on `Group` needs the group itself, and one
/// keying on a path needs the tokens inside it. Shared rather than per join for
/// the reason `path_roots` records — the `Spacing::Joint` correction changed
/// which crates read as reached, and a second copy would have kept the old
/// answer.
fn flatten(tokens: TokenStream, out: &mut Vec<TokenTree>) {
    for tree in tokens {
        match tree {
            TokenTree::Group(group) => {
                let inner = group.stream();
                out.push(TokenTree::Group(group));
                flatten(inner, out);
            }
            other => out.push(other),
        }
    }
}

/// An item's attributes, for the `#[cfg(test)]` question. `syn` gives no
/// uniform accessor, so every shape that carries attributes is listed — one
/// left out would read as shipped whatever gates it.
pub fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// What a `cargo nextest run` actually executes in this file, as token streams.
///
/// A file under `tests/` is a test target whole. A file under `src/` is executed
/// only inside its `#[cfg(test)]` items — which is the distinction between
/// *asserting* a thing and merely *mentioning* it, and the one a text scan
/// cannot make.
///
/// Trybuild fixtures are excluded: `tests/**/diagnostics/` is input the suite
/// hands to rustc, never code the suite runs, so a seam named there is compiled
/// at best and usually not even that.
pub fn executed_tokens(path: &Path, root: &Path) -> Vec<TokenStream> {
    let rel = relative(path, root);
    if rel.contains("/diagnostics/") {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    if rel.contains("/tests/") {
        return text.parse::<TokenStream>().ok().into_iter().collect();
    }
    let Ok(file) = syn::parse_file(&text) else {
        return Vec::new();
    };
    let mut scan = UnderCfgTest::default();
    scan.visit_file(&file);
    scan.out
}

#[derive(Default)]
struct UnderCfgTest {
    out: Vec<TokenStream>,
}

impl<'ast> Visit<'ast> for UnderCfgTest {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if is_cfg_test(&node.attrs) {
            self.out.push(Item::Mod(node.clone()).into_token_stream());
            return;
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if is_cfg_test(&node.attrs) {
            self.out.push(Item::Fn(node.clone()).into_token_stream());
            return;
        }
        syn::visit::visit_item_fn(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A predicate that names `test` without implying it is shipped code — the
    /// two spellings every join used to skip as fixtures.
    #[test]
    fn a_cfg_is_a_fixture_only_when_it_implies_test() {
        let fixture = |cfg: &str| {
            let item: syn::ItemMod = syn::parse_str(&format!("#[cfg({cfg})] mod m {{}}"))
                .expect("a module with a cfg parses");
            is_cfg_test(&item.attrs)
        };
        for implies in [
            "test",
            "all(test, unix)",
            "all(unix, any(test, all(test, x)))",
            "any(test)",
        ] {
            assert!(fixture(implies), "{implies}");
        }
        for ships in [
            "not(test)",
            "any(test, feature = \"testing\")",
            "unix",
            "feature = \"test\"",
            "any()",
        ] {
            assert!(!fixture(ships), "{ships}");
        }
    }

    /// A root spelling every word a join classifies a path on — `src`, an edge,
    /// a suite, a fixture folder, a template folder.
    const HOSTILE_ROOT: &str = "/home/dev/src/tests/diagnostics/templates/schedule/nestrs";

    #[test]
    fn a_path_reads_the_same_below_any_root() {
        let rel = "crates/nest-rs-probe/src/http/guard.rs";
        for root in ["/repo", HOSTILE_ROOT] {
            let root = Path::new(root);
            let path = root.join(rel);
            assert_eq!(
                segments(&path, root),
                ["crates", "nest-rs-probe", "src", "http", "guard.rs"],
            );
            assert_eq!(relative(&path, root), rel);
        }
    }

    #[test]
    fn a_path_reads_below_a_crates_src_as_well() {
        let src = Path::new(HOSTILE_ROOT).join("crates/nest-rs-redis/src");
        assert_eq!(
            segments(&src.join("queue/module.rs"), &src),
            ["queue", "module.rs"],
        );
    }

    #[test]
    #[should_panic(expected = "is not below")]
    fn a_path_outside_the_tree_is_refused_rather_than_read_whole() {
        let _ = segments(
            Path::new("/elsewhere/src/http/guard.rs"),
            Path::new("/repo"),
        );
    }
}
