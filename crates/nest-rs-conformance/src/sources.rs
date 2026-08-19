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
/// with a string literal, so both are read the same way — the same shape
/// [`declared_pairs`] reads for decorator pairs.
///
/// Returns `(target, declaring crate directory, constant name)`. The name is
/// what disambiguates: `nest_rs_core` declares seven, so the crate alone cannot
/// say which of them `…::operation_log::TARGET` is.
pub fn declared_targets() -> Vec<(String, String, String)> {
    let root = repo_root();
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
    let _ = root;
    out
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

/// How a site spelled a value the framework interprets — a `target:`, a unit
/// name, a span kind.
///
/// One enum rather than one per join: the three that read these token streams
/// asked the same question and answered it in three shapes, and the shapes had
/// already drifted (one held the identifier, another the resolved value).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Named {
    /// A path. The payload is the last segment — the constant's name.
    Path(String),
    /// A string literal.
    Literal(String),
}

/// A macro invocation's tokens, flattened at the **top level only**.
///
/// Deliberately not [`flatten`], which descends into groups: a `target:` inside
/// a nested group is not the macro's target, and reading one as if it were is
/// how a join comes to believe a fixture is an emission.
pub fn top_level(tokens: &TokenStream) -> Vec<TokenTree> {
    tokens.clone().into_iter().collect()
}

/// The index just past `key <punct>` at the top level, if present.
pub fn value_after(tokens: &[TokenTree], key: &str, punct: char) -> Option<usize> {
    tokens
        .windows(2)
        .position(|w| match (&w[0], &w[1]) {
            (TokenTree::Ident(i), TokenTree::Punct(p)) => i == key && p.as_char() == punct,
            _ => false,
        })
        .map(|at| at + 2)
}

/// The index just past the value at `at`, and the comma after it.
///
/// A value is one literal or a `::`-joined path, so this **walks** it rather
/// than assuming a width — assuming one is how a fixed offset came to read the
/// middle of `operation_log::kind::SERVER`.
pub fn past_value(tokens: &[TokenTree], at: usize) -> usize {
    let mut i = at;
    while let Some(t) = tokens.get(i) {
        match t {
            TokenTree::Ident(_) => i += 1,
            TokenTree::Punct(p) if p.as_char() == ':' => i += 1,
            TokenTree::Literal(_) if i == at => i += 1,
            _ => break,
        }
    }
    match tokens.get(i) {
        Some(TokenTree::Punct(p)) if p.as_char() == ',' => i + 1,
        _ => i,
    }
}

/// Read the token at `at` as a literal or a path, with the path's segments.
pub fn named_at(tokens: &[TokenTree], at: usize) -> Option<(Named, Vec<String>)> {
    match tokens.get(at)? {
        TokenTree::Literal(lit) => syn::parse_str::<syn::LitStr>(&lit.to_string())
            .ok()
            .map(|s| (Named::Literal(s.value()), Vec::new())),
        TokenTree::Ident(_) => {
            let segments: Vec<String> = tokens[at..]
                .iter()
                .take_while(|t| {
                    matches!(t, TokenTree::Ident(_))
                        || matches!(t, TokenTree::Punct(p) if p.as_char() == ':')
                })
                .filter_map(|t| match t {
                    TokenTree::Ident(i) => Some(i.to_string()),
                    _ => None,
                })
                .collect();
            let last = segments.last()?.clone();
            Some((Named::Path(last), segments))
        }
        _ => None,
    }
}

/// The `target:` a macro call declares, whether spelled as a literal or — as
/// every framework site now does — as a constant.
///
/// Worded once because it was worded three times: `filters`, `units` and
/// `events` each walked this grammar, differing only in what they returned, and
/// a correction to one left the others answering the old way in silence.
pub fn declared_target(tokens: &TokenStream, file: &str) -> Option<Named> {
    let flat = top_level(tokens);
    let at = value_after(&flat, "target", ':')?;
    match named_at(&flat, at)? {
        (Named::Literal(text), _) => Some(Named::Literal(text)),
        (Named::Path(_), segments) => Some(match resolve_target(&segments, file) {
            Some(value) => Named::Literal(value.to_owned()),
            None => Named::Path(segments.join("::")),
        }),
    }
}

/// Resolve a constant path to the target string it names.
///
/// The key is **(declaring crate, constant name)**, which is unique:
/// `nest-rs-core` declares seven targets, so the crate alone cannot say which
/// of them `…::operation_log::TARGET` is, and `TARGET` alone cannot say which
/// crate's it is now that every crate has one.
pub fn resolve_target(segments: &[String], file: &str) -> Option<&'static str> {
    let name = segments.last()?;
    let owner = declaring_crate(segments, file);
    target_table()
        .iter()
        .find(|(_, krate, konst)| *krate == owner && konst == name)
        .map(|(target, _, _)| *target)
}

/// Which crate a constant belongs to: the one its path names, or — for
/// `crate::…` and a bare name — the one the file lives in.
fn declaring_crate(segments: &[String], file: &str) -> String {
    for segment in segments.iter().rev().skip(1) {
        if let Some(krate) = segment.strip_prefix("nest_rs_") {
            return format!("nest-rs-{}", krate.replace('_', "-"));
        }
    }
    file.strip_prefix("crates/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .to_owned()
}

/// The declared table, read once per test process.
fn target_table() -> &'static [(&'static str, &'static str, &'static str)] {
    static TABLE: std::sync::OnceLock<Vec<(&'static str, &'static str, &'static str)>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        declared_targets()
            .into_iter()
            // Leaked once, deliberately: the table is the process's, and every
            // caller compares against a borrowed `&'static str`.
            .map(|(target, krate, konst)| {
                let leak = |s: String| &*Box::leak(s.into_boxed_str());
                (leak(target), leak(krate), leak(konst))
            })
            .collect()
    })
}

/// The operation log's own target, read from its declaration.
///
/// Read rather than linked: this crate proves things *about* the framework and
/// depending on it to learn one string would put the whole tree behind the test
/// binary — 400 crates and a 100 MB relink, measured, to compare a `&str`.
pub fn operation_log_target() -> Option<&'static str> {
    resolve_target(
        &["operation_log".to_owned(), "TARGET".to_owned()],
        "crates/nest-rs-core/src/operation_log.rs",
    )
}

/// Every `.rs` file both workspaces own, parsed, with the CLI's templates left
/// out — their targets carry handlebars placeholders and become real ones under
/// a scaffolded tree's own root.
///
/// The skip is the load-bearing half and it was written three times in three
/// wordings; a fourth join would have copied whichever it sat next to.
pub fn each_source(root: &Path, mut visit: impl FnMut(&str, &syn::File)) {
    let mut sources = rust_files(&root.join("crates"));
    sources.extend(rust_files(&root.join("demo")));
    for path in sources {
        let rel = relative(&path, root);
        if rel.contains("cli/src/templates/") {
            continue;
        }
        if let Some(ast) = parsed(&path) {
            visit(&rel, &ast);
        }
    }
}

/// Every fenced code block in this file's doc comments, parsed as Rust.
///
/// **A join that reads only items is blind exactly where a developer copies
/// from.** `syn` lowers `///` to `#[doc = "…"]`, so a macro invocation inside a
/// doctest is one string literal and `visit_macro` never descends into it: the
/// canonical `operation_span!` example taught all three of the literal forms the
/// `units` join forbids, for as long as that was true, and nothing failed. An
/// example is code the compiler runs and the reader imitates, so it is scanned
/// as code.
///
/// Rustdoc's hidden lines (`# …`) are code and are kept. A block that does not
/// parse is **dropped, never reported**: the same fences hold `text`, JSON and
/// shell, and this is a reader for what an example *does*, not a linter for what
/// it says. The converse is the known looseness — prose that happens to parse as
/// an expression is walked as if it were code, which costs nothing to a join
/// keyed on a macro name and would matter to one keyed on an identifier.
pub fn doctests(ast: &syn::File) -> Vec<syn::File> {
    let mut text = DocText::default();
    text.visit_file(ast);

    let mut out = Vec::new();
    let mut open: Option<String> = None;
    for line in text.0.lines() {
        // `syn` hands back the comment's text with the space after `///` intact.
        let body = line.strip_prefix(' ').unwrap_or(line);
        if body.trim_start().starts_with("```") {
            match open.take() {
                Some(block) => out.extend(parse_doctest(&block)),
                None => open = Some(String::new()),
            }
            continue;
        }
        if let Some(block) = open.as_mut() {
            let code = match body {
                "#" => "",
                _ if body.starts_with("##") => &body[1..],
                _ => body.strip_prefix("# ").unwrap_or(body),
            };
            block.push_str(code);
            block.push('\n');
        }
    }
    out
}

/// A doctest is items or statements, and which one is the author's business.
fn parse_doctest(code: &str) -> Option<syn::File> {
    syn::parse_file(code)
        .ok()
        .or_else(|| syn::parse_file(&format!("fn __doctest() {{\n{code}\n}}")).ok())
}

/// Every `#[doc]` string in a file, in source order.
#[derive(Default)]
struct DocText(String);

impl<'ast> Visit<'ast> for DocText {
    fn visit_attribute(&mut self, node: &'ast Attribute) {
        if let Meta::NameValue(pair) = &node.meta
            && pair.path.is_ident("doc")
            && let Expr::Lit(literal) = &pair.value
            && let Lit::Str(text) = &literal.lit
        {
            self.0.push_str(&text.value());
            self.0.push('\n');
        }
    }
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
