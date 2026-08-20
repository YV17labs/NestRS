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

/// The value of a `pub const <name>: &str = "…";` in one file — free, or
/// associated on an `impl`.
///
/// **Parsed, not grepped.** The declarations this crate reads are string
/// constants, and a text scan reads two of them wrong: a `\`-continued literal
/// and a `#[cfg(test)]` fixture spelling the same name. [`parsed`] exists for
/// exactly that, and the alternative — linking the crate that declares it — is
/// the 400-crate relink [`operation_log_target`] records measuring.
pub fn declared_str(path: &Path, name: &str) -> Option<String> {
    let ast = parsed(path)?;
    // Top level first, so a free constant always wins a name an `impl` also
    // uses. The associated arm came second, for `EnvPrefix::DEFAULT` — a value
    // the `docs` join needs and which lives where the type that owns it does,
    // which is the placement the naming rules ask for. Reading only free
    // constants would have meant either a second reader beside this one or a
    // literal beside the rule forbidding it.
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
/// that owes witnesses, which is the umbrella join's own subject and is keyed on
/// the crate for the reason that join argues at `Capability::cell`.
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

/// Top-level items only, which is a stated limit rather than an oversight: a
/// pair declared inside an inline `mod` is not joined. Its sibling
/// [`collect_target_consts`] descends one level for the same `Item::Const`
/// shape, and the asymmetry is the population's — a `DecoratorPair` const is
/// written at a macro crate's top level by every one of the nine, and a tenth
/// hidden in a `mod` would also be invisible to the `rg 'DecoratorPair'` the
/// rule names as the human half of the check.
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
///
/// **Walked once per test process**, because three callers want the same table:
/// [`resolve_target`] resolves every emission site against it, `filters` merges
/// the declarations into its population, and `edges` asks it one question per
/// edge. Answering each by re-walking every `crates/*/src` file was four passes
/// over the same tree inside one join — the cost the same rule is stated
/// against in `edges`'s `framework_idents`. Leaked, deliberately: the table is
/// the process's, and every caller compares against a borrowed `&'static str`.
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

/// A `pub const X: &str = "…";` inside a `unit` module — which is either the
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
                if let Expr::Lit(lit) = &*konst.expr
                    && let Lit::Str(text) = &lit.lit
                {
                    out.push((text.value(), krate.to_owned(), konst.ident.to_string()));
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
    declared_targets()
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

/// The operation log's own target, read from its declaration.
///
/// Read rather than linked: this crate proves things *about* the framework and
/// depending on it to learn one string would put the whole tree behind the test
/// binary — 400 crates and a 100 MB relink, measured, to compare a `&str`.
///
/// Resolved once: it is one `&'static str` for the whole process, and the
/// `units` join asks for it once per `info!` in both workspaces.
pub fn operation_log_target() -> Option<&'static str> {
    static TARGET: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *TARGET.get_or_init(|| {
        resolve_target(
            &["operation_log".to_owned(), "TARGET".to_owned()],
            "crates/nest-rs-core/src/operation_log.rs",
        )
    })
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

/// Whether a test target rooted at `main_rs` **runs anything**, following its
/// `mod` tree the way Cargo compiles it.
///
/// The tree, not the directory, and that distinction is the finding: a suite's
/// sibling files are compiled only because `main.rs` declares them, so
/// truncating `main.rs` to zero bytes leaves a directory full of `#[test]`
/// functions that Cargo never sees. Asking the directory said "yes" there;
/// asking the root says "no", which is the true answer.
///
/// `testing.md` is what makes the walk cheap and total: "`main.rs` is the suite
/// *root*, never a test module: `//!` + the `mod` list + the fixtures the
/// siblings share — **no `#[test]` function lives there**". So a root with no
/// `mod` is a suite with nothing in it, whatever the folder holds.
pub fn suite_runs_tests(main_rs: &Path) -> bool {
    let Some(dir) = main_rs.parent() else {
        return false;
    };
    let mut pending = vec![main_rs.to_path_buf()];
    let mut seen = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(ast) = parsed(&path) else {
            continue;
        };
        let mut scan = TestFns::default();
        scan.visit_file(&ast);
        if scan.found {
            return true;
        }
        // `mod x;` resolves to `x.rs` or `x/mod.rs` beside the declaring file,
        // which for a suite root is the suite directory. An inline `mod x { … }`
        // needs no resolution — `visit_file` already descended into it.
        let here = path.parent().unwrap_or(dir);
        for item in &ast.items {
            let Item::Mod(m) = item else {
                continue;
            };
            if m.content.is_some() {
                continue;
            }
            let name = m.ident.to_string();
            pending.push(here.join(format!("{name}.rs")));
            pending.push(here.join(&name).join("mod.rs"));
        }
    }
    false
}

/// Whether a path — a file, or a directory read whole — carries at least one
/// **test function**.
///
/// The answer to "does this suite exist?", where `main.rs.is_file()` was the
/// answer for a round and truncating that file to zero bytes left every cell
/// asking it green. A suite that runs nothing is not a suite, and the whole
/// point of the cells that ask is that something is executed.
///
/// `#[test]`, `#[tokio::test]` and any other attribute whose last path segment
/// is `test` — a list of attribute spellings would be a hand-maintained set
/// failing in the unsafe direction the day a runner adds one.
pub fn carries_a_test(path: &Path) -> bool {
    files_at(path).into_iter().any(|file| {
        parsed(&file).is_some_and(|ast| {
            let mut scan = TestFns::default();
            scan.visit_file(&ast);
            scan.found
        })
    })
}

/// Whether a path — a file, or a directory read whole — declares any **shipped
/// item at all**: a `struct`, `enum`, `fn`, `impl`, `trait`, `const` or `type`.
///
/// The answer to "does this module exist?", where `is_dir()` was the answer and
/// emptying every file inside left the cell green. A `mod`, a `use` and a doc
/// comment are deliberately not items here: a module that only re-exports or
/// only describes carries no implementation, which is what a cell asking for
/// one means.
pub fn declares_an_item(path: &Path) -> bool {
    files_at(path).into_iter().any(|file| {
        parsed(&file).is_some_and(|ast| {
            ast.items.iter().any(|item| {
                !is_cfg_test(item_attrs(item))
                    && matches!(
                        item,
                        Item::Struct(_)
                            | Item::Enum(_)
                            | Item::Fn(_)
                            | Item::Impl(_)
                            | Item::Trait(_)
                            | Item::Const(_)
                            | Item::Type(_)
                    )
            })
        })
    })
}

/// A path taken either way: one `.rs` file, or every `.rs` file under a
/// directory. Both callers are written as a path, and which shape it is is a
/// fact about the layout rather than about the obligation.
fn files_at(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        rust_files(path)
    } else if path.is_file() {
        vec![path.to_owned()]
    } else {
        Vec::new()
    }
}

/// A top-level item's attributes. `syn` gives no uniform accessor, and the
/// shapes a `#[cfg(test)]` legitimately sits on are few.
fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Mod(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::Const(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        _ => &[],
    }
}

#[derive(Default)]
struct TestFns {
    found: bool,
}

impl<'ast> Visit<'ast> for TestFns {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.found |= node.attrs.iter().any(|attr| {
            attr.path()
                .segments
                .last()
                .is_some_and(|s| s.ident == "test")
        });
        syn::visit::visit_item_fn(self, node);
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
