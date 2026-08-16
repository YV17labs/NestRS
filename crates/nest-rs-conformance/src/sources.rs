//! Where a join reads its population from.
//!
//! Every member list here is walked from the tree, never listed: a family that
//! grows a member the day someone writes it is the whole point, and a literal
//! list is the defect the rule names.

use std::collections::BTreeSet;
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
    collect(dir, extension, &mut out);
    out
}

fn collect(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, extension, out);
        } else if path.extension().is_some_and(|e| e == extension) {
            out.push(path);
        }
    }
}

/// The path as the repo spells it, for a message a reader can paste into `rg`.
pub fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Compared by value, so the wrapping a `\` continuation introduces never
/// decides whether two spellings are the same string.
pub fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// The decorators a crate exports, or empty when it exports none.
///
/// Rust forces `#[proc_macro_attribute]` items to the crate root, so `lib.rs` is
/// the whole surface — the one place this join has to read, and the reason a
/// proc-macro crate is recognised by what it contains rather than by its name.
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
            if path.is_ident("proc_macro_attribute") || path.is_ident("proc_macro") {
                out.push(f.sig.ident.to_string());
            } else if path.is_ident("proc_macro_derive")
                && let Meta::List(list) = &attr.meta
                && let Some(TokenTree::Ident(name)) = list.tokens.clone().into_iter().next()
            {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// A pair as its own declaration spells it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pair {
    /// The `*-macros` crate that declares it, e.g. `nest-rs-http-macros`.
    pub krate: String,
    /// The struct half — `#[injectable]` for a provider-hosted pair.
    pub host: String,
    /// The impl half.
    pub operations: String,
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

/// Both spellings of a declaration, and only these two: a third would be a
/// second way to declare a pair, which the rule forbids before this join has
/// to care.
fn pair_from(expr: &Expr) -> Option<(String, String)> {
    match expr {
        // `DecoratorPair { host: "#[controller]", operations: "#[routes]", .. }`
        Expr::Struct(lit) if lit.path.segments.last()?.ident == "DecoratorPair" => {
            let field = |name: &str| {
                lit.fields
                    .iter()
                    .find(|f| matches!(&f.member, syn::Member::Named(i) if i == name))
                    .and_then(|f| as_str_lit(&f.expr))
            };
            Some((field("host")?, field("operations")?))
        }
        // `DecoratorPair::on_provider("#[processor]", "#[process]")` — the host
        // is the generic `#[injectable]`, which is why five pairs share one
        // host cell rather than owing five.
        Expr::Call(call) => {
            let Expr::Path(path) = &*call.func else {
                return None;
            };
            if path.path.segments.last()?.ident != "on_provider" {
                return None;
            }
            let operations = as_str_lit(call.args.first()?)?;
            Some(("#[injectable]".to_owned(), operations))
        }
        _ => None,
    }
}

pub fn declared_pairs() -> Vec<Pair> {
    let root = repo_root();
    let mut out = Vec::new();
    for dir in std::fs::read_dir(root.join("crates"))
        .expect("crates/ is readable")
        .flatten()
    {
        let path = dir.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with("-macros") {
            continue;
        }
        for file in rust_files(&path.join("src")) {
            let Some(ast) = parsed(&file) else {
                continue;
            };
            for item in &ast.items {
                let Item::Const(konst) = item else {
                    continue;
                };
                if let Some((host, operations)) = pair_from(&konst.expr) {
                    out.push(Pair {
                        krate: name.to_owned(),
                        host,
                        operations,
                    });
                }
            }
        }
    }
    out
}

/// Whether an item is behind `#[cfg(test)]`.
///
/// Shared rather than per join: it separates the two things a `src/` file holds
/// — the framework's own emissions, and the assertions about them — and every
/// join needs one side or the other.
pub fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| match &attr.meta {
        Meta::List(list) if list.path.is_ident("cfg") => list
            .tokens
            .clone()
            .into_iter()
            .any(|t| matches!(&t, TokenTree::Ident(i) if i == "test")),
        _ => false,
    })
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
pub fn flatten(tokens: TokenStream, out: &mut Vec<TokenTree>) {
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

/// Every identifier `tokens` spell, at any depth.
///
/// The loosest of the three token reads here, and the right one where the
/// question is whether a name is *written* at all — a fixture naming a
/// decorator, a hygiene use site naming a macro.
pub fn idents(tokens: TokenStream) -> BTreeSet<String> {
    let mut flat = Vec::new();
    flatten(tokens, &mut flat);
    flat.iter()
        .filter_map(|tree| match tree {
            TokenTree::Ident(ident) => Some(ident.to_string()),
            _ => None,
        })
        .collect()
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
