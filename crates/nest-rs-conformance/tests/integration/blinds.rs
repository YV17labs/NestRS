//! The blinds join: no construction that hides a name from a join is written in
//! the source the joins read.
//!
//! Every join here reads Rust as written. That is what makes them cheap and
//! exact, and it is also their one weakness, which round after round of audit
//! found again at a new join: a construction that writes the same item under
//! another name, or emits it where the join does not look, takes a member out of
//! the population **while the join stays green**. A `use … as` of a decorator's
//! sentence dropped its decorator from the grammars join; `use Capability::*`
//! dropped a backend's claim from the queue capabilities join; a `#[module]`
//! written by a `macro_rules!` passed the module-location gate; a `#[path]`
//! module was a file the dependencies join never opened. Each was answered at
//! its own join, and the next join had the same hole.
//!
//! So the answer is one join, and its vocabulary is every other join's:
//!
//! - **What a join reads by its spelling is declared, beside the code that reads
//!   it** — each join's `followed()`, a list of [`Followed`] names and the place
//!   each is read at. [`JOINS`] gathers them, and
//!   [`every_join_declares_what_it_follows`] fails on a join that has no row, so
//!   a join written tomorrow answers the question the day it lands.
//! - **What would hide one is refused** in the source every join reads — the
//!   `src/` of every crate in both workspaces, outside `#[cfg(test)]` — by
//!   [`no_source_hides_a_name_a_join_follows`]. There is no baseline: a blind
//!   spot is a failure the day it is written, never a hole that waits.
//!
//! **The constructions refused**, each named by the joins it blinds:
//!
//! 1. **A file `syn` cannot parse.** Every join reads a file through
//!    `sources::parsed` and skips one that does not parse, so such a file is
//!    absent from every population at once.
//! 2. **A `#[path]` module.** The import resolver (`nest_rs_conformance::imports`)
//!    finds a module's file at the default layout, and `naming` reads a type's
//!    stem off its file's path — `#[path]` makes the two disagree, and the reader
//!    opens the wrong file or none.
//! 3. **`extern crate … as …`.** The renamed crate enters the extern prelude,
//!    visible in every module of the crate, while the resolver reads the imports
//!    of one module at a time.
//! 4. **A rename of a followed name** — `use … as X` (an `as _` binds nothing and
//!    is left alone). The joins that must follow a rename read the crate through
//!    the shared resolver instead, and declare what they follow as a crate
//!    ([`At::Crate`]), which a rename does not hide.
//! 5. **A `type` alias of a followed type or trait** — `type Backend =
//!    QueueBackend` — a rename by another keyword.
//! 6. **An import through a followed type** — `use …::Capability::*` or
//!    `use …::Capability::Throttle` — which leaves the member written without the
//!    head the join reads it under; and **a glob of a followed crate's items**,
//!    the one import the resolver cannot follow.
//! 7. **A followed name a `macro_rules!` transcriber writes where its join does
//!    not read it**: an attribute (`#[module]`, every framework decorator), the
//!    `impl` or the supertrait of a followed trait, a followed call, a followed
//!    declaration, a followed key's literal value — and, where the join does read
//!    transcribers, a member the macro's caller chooses (`Capability::$v`).
//! 8. **A followed attribute inside `cfg_attr`.** An attribute-reading join reads
//!    the attribute's own path, and `#[cfg_attr(…, module)]`'s path is
//!    `cfg_attr`.
//!
//! **One reading is shared and corrected rather than refused.** `#[cfg(test)]`
//! is how every join tells a fixture from shipped code, and `sources::is_cfg_test`
//! read any `cfg` naming `test` as a fixture — so `#[cfg(not(test))]` and
//! `#[cfg(any(test, feature = "…"))]` hid shipped code from every join. It now
//! asks whether the predicate *implies* `test`.
//!
//! **What this join cannot see**, named rather than implied: a name a
//! procedural macro of another crate emits into the source — a decorator's
//! expansion is the macro crate's and is read there, by the joins whose
//! population is the macro crates — and a followed name built by
//! `concat_idents!`-like token pasting, which stable Rust does not offer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nest_rs_conformance::sources::{
    crate_dirs, flatten, is_cfg_test, read, relative, repo_root, rust_files,
};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::visit::Visit;
use syn::{Attribute, Item, UseTree};

use crate::{At, Followed};

/// A join's own declaration of what it reads by its spelling.
type Declares = fn() -> Vec<Followed>;

/// Every join, with what it reads by its spelling. A join that reads nothing
/// that way says why in its own `//!`.
const JOINS: &[(&str, Declares)] = &[
    ("acls", crate::acls::followed),
    ("canon", crate::canon::followed),
    ("decodes", crate::decodes::followed),
    ("dependencies", crate::dependencies::followed),
    ("docs", crate::docs::followed),
    ("edges", crate::edges::followed),
    ("entries", crate::entries::followed),
    ("env_names", crate::env_names::followed),
    ("events", crate::events::followed),
    ("filters", crate::filters::followed),
    ("grammars", crate::grammars::followed),
    ("guards", crate::guards::followed),
    ("keys", crate::keys::followed),
    ("mirrors", crate::mirrors::followed),
    ("naming", crate::naming::followed),
    ("panics", crate::panics::followed),
    ("paths", crate::paths::followed),
    ("queue_capabilities", crate::queue_capabilities::followed),
    ("seams", crate::seams::followed),
    ("shapes", crate::shapes::followed),
    ("snapshots", crate::snapshots::followed),
    ("targets", crate::targets::followed),
    ("transports", crate::transports::followed),
    ("umbrella", crate::umbrella::followed),
    ("units", crate::units::followed),
    ("upgrading", crate::upgrading::followed),
];

/// Below this the walk is reading the wrong tree.
const FLOOR: usize = 500;

/// Every followed name, from every join, once — and the joins that follow it.
fn followed() -> BTreeMap<Followed, BTreeSet<&'static str>> {
    let mut out: BTreeMap<Followed, BTreeSet<&'static str>> = BTreeMap::new();
    for &(join, declared) in JOINS {
        for name in declared() {
            out.entry(name).or_default().insert(join);
        }
    }
    out
}

/// **Every join answers the question.** The joins are the suite root's `mod`
/// list, read rather than recopied, so a join added there without a row here
/// fails — and a row is a declaration, so "nothing" is an answer its `//!`
/// argues, never a default.
#[test]
fn every_join_declares_what_it_follows() {
    let root = repo_root();
    let main = root.join("crates/nest-rs-conformance/tests/integration/main.rs");
    let ast = syn::parse_file(&read(&main).expect("the suite root is readable"))
        .expect("the suite root parses");
    let modules: BTreeSet<String> = ast
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Mod(module) if module.content.is_none() => Some(module.ident.to_string()),
            _ => None,
        })
        .filter(|name| name != "blinds")
        .collect();
    let rows: BTreeSet<String> = JOINS.iter().map(|(join, _)| (*join).to_owned()).collect();
    assert_eq!(
        rows, modules,
        "every join in `main.rs` has a row in `blinds::JOINS` — what it reads by its \
         spelling, through its own `followed()` — and no row names a join that is gone",
    );
}

/// **No source the joins read hides a name one of them follows.**
#[test]
fn no_source_hides_a_name_a_join_follows() {
    let root = repo_root();
    let followed = followed();
    let (blinds, read) = blinds_in(&root, &crate_dirs(), &followed);
    nest_rs_conformance::baseline::floor(read, FLOOR, "source file(s) read");
    assert!(
        blinds.is_empty(),
        "{} construction(s) hide a name a conformance join reads by its spelling, so the \
         join passes without seeing the member — write the name the join reads (no \
         rename, no `type` alias, no import through the type, no glob), declare the \
         module at the default layout, and keep a followed item out of `macro_rules!` \
         and `cfg_attr`:\n  {}",
        blinds.len(),
        blinds.into_iter().collect::<Vec<_>>().join("\n  "),
    );
}

/// Every construction in the `src/` of `dirs` that hides a followed name, and
/// how many files were read.
fn blinds_in(
    root: &Path,
    dirs: &[PathBuf],
    followed: &BTreeMap<Followed, BTreeSet<&'static str>>,
) -> (BTreeSet<String>, usize) {
    let mut out = BTreeSet::new();
    let mut count = 0;
    for dir in dirs {
        for path in rust_files(&dir.join("src")) {
            count += 1;
            let file = relative(&path, root);
            let text = match read(&path) {
                Ok(text) => text,
                Err(error) => {
                    out.insert(format!(
                        "{file} :: unreadable ({error}) — every join skips it"
                    ));
                    continue;
                }
            };
            let ast = match syn::parse_file(&text) {
                Ok(ast) => ast,
                Err(error) => {
                    out.insert(format!(
                        "{file} :: `syn` cannot parse it ({error}) — every join skips it"
                    ));
                    continue;
                }
            };
            let mut scan = Scan {
                file: &file,
                followed,
                out: &mut out,
            };
            scan.visit_file(&ast);
        }
    }
    (out, count)
}

struct Scan<'a> {
    file: &'a str,
    followed: &'a BTreeMap<Followed, BTreeSet<&'static str>>,
    out: &'a mut BTreeSet<String>,
}

impl Scan<'_> {
    fn refuse(&mut self, what: String, name: &Followed) {
        let joins = self.followed[name]
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        self.out
            .insert(format!("{} :: {what} — blinds {joins}", self.file));
    }

    /// The followed names read at one of `at` and spelled `name` — the segment
    /// before it, `owner`, checked against a name's owners where it has some.
    fn matching(&self, name: &str, owner: Option<&str>, at: &[At]) -> Vec<Followed> {
        self.followed
            .keys()
            .filter(|followed| {
                followed.name == name
                    && at.contains(&followed.at)
                    && (followed.through.is_empty()
                        || owner.is_some_and(|owner| followed.through.contains(&owner)))
            })
            .cloned()
            .collect()
    }

    fn use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.use_tree(&path.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(name) => {
                let mut path = prefix.clone();
                path.push(name.ident.to_string());
                self.through_a_type(&path[..path.len() - 1], &path);
            }
            UseTree::Rename(rename) => {
                let mut path = prefix.clone();
                path.push(rename.ident.to_string());
                self.through_a_type(&path[..path.len() - 1], &path);
                if rename.rename == "_" {
                    return;
                }
                let leaf = if rename.ident == "self" {
                    prefix.last().cloned().unwrap_or_default()
                } else {
                    rename.ident.to_string()
                };
                let owner = if rename.ident == "self" {
                    prefix.iter().rev().nth(1)
                } else {
                    prefix.last()
                };
                let renamable = [At::Attribute, At::Trait, At::Type, At::Call];
                for name in self.matching(&leaf, owner.map(String::as_str), &renamable) {
                    self.refuse(
                        format!("`use {} as {}` renames it", path.join("::"), rename.rename),
                        &name,
                    );
                }
            }
            UseTree::Glob(_) => {
                self.through_a_type(prefix, &[prefix.as_slice(), &["*".to_owned()]].concat());
                if let Some(first) = prefix.first() {
                    for name in self.matching(first, None, &[At::Crate]) {
                        self.refuse(
                            format!(
                                "`use {}::*` brings its items in under names the resolver \
                                 cannot follow",
                                prefix.join("::"),
                            ),
                            &name,
                        );
                    }
                }
            }
            UseTree::Group(group) => {
                for tree in &group.items {
                    self.use_tree(tree, prefix);
                }
            }
        }
    }

    /// A `use` whose path passes **through** a followed type — every segment of
    /// `through` — writes its member without the head the join reads.
    fn through_a_type(&mut self, through: &[String], path: &[String]) {
        for (at, segment) in through.iter().enumerate() {
            let owner = at.checked_sub(1).map(|before| through[before].as_str());
            for name in self.matching(segment, owner, &[At::Type]) {
                self.refuse(
                    format!(
                        "`use {}` writes a member of `{segment}` without `{segment}::`",
                        path.join("::"),
                    ),
                    &name,
                );
            }
        }
    }

    /// The followed attributes a `cfg_attr` wraps.
    fn cfg_attr(&mut self, attr: &Attribute) {
        if !attr.path().is_ident("cfg_attr") {
            return;
        }
        let syn::Meta::List(list) = &attr.meta else {
            return;
        };
        // Past the predicate: the attributes are what follows its first comma.
        let mut past = false;
        let mut wrapped = Vec::new();
        for tree in list.tokens.clone() {
            match &tree {
                TokenTree::Punct(p) if p.as_char() == ',' && !past => past = true,
                _ if past => wrapped.push(tree),
                _ => {}
            }
        }
        let mut flat = Vec::new();
        flatten(wrapped.into_iter().collect(), &mut flat);
        for tree in &flat {
            let TokenTree::Ident(ident) = tree else {
                continue;
            };
            for name in self.matching(&ident.to_string(), None, &[At::Attribute]) {
                self.refuse(
                    format!("`#[cfg_attr(…, {ident}…)]` writes it under `cfg_attr`'s path"),
                    &name,
                );
            }
        }
    }

    /// What a `macro_rules!` transcriber writes that no join reading `name`
    /// reads there.
    fn transcriber(&mut self, tokens: TokenStream) {
        let mut flat = Vec::new();
        flatten(tokens, &mut flat);
        let followed: Vec<Followed> = self.followed.keys().cloned().collect();
        for name in &followed {
            if let Some(shape) = transcribed(&flat, name) {
                self.refuse(
                    format!("a `macro_rules!` writes {shape}, which no join expands"),
                    name,
                );
            }
        }
    }
}

/// The shape in which `flat` — a transcriber's tokens, groups kept and
/// descended into — writes `name` where its join does not read it, if it does.
fn transcribed(flat: &[TokenTree], name: &Followed) -> Option<String> {
    let ident_at =
        |at: usize, text: &str| matches!(flat.get(at), Some(TokenTree::Ident(i)) if i == text);
    let punct_at = |at: usize, ch: char| matches!(flat.get(at), Some(TokenTree::Punct(p)) if p.as_char() == ch);
    let n = name.name.as_str();
    let read = name.in_transcribers;
    (0..flat.len()).find_map(|at| match name.at {
        At::Attribute
            if !read
                && punct_at(at, '#')
                && matches!(flat.get(at + 1), Some(TokenTree::Group(group))
                    if group.delimiter() == Delimiter::Bracket
                        && nest_rs_conformance::sources::idents(group.stream()).contains(n)) =>
        {
            Some(format!("the attribute `#[{n}]`"))
        }
        At::Trait if !read && ident_at(at, n) && implemented_at(flat, at + 1) => {
            Some(format!("an `impl {n} for`"))
        }
        At::Trait if !read && ident_at(at, "trait") && bounds_of_trait(flat, at).contains(n) => {
            Some(format!("a trait bounded by `{n}`"))
        }
        At::Type if !read && ident_at(at, n) => Some(format!("the type `{n}`")),
        At::Type
            if ident_at(at, n)
                && punct_at(at + 1, ':')
                && punct_at(at + 2, ':')
                && punct_at(at + 3, '$') =>
        {
            Some(format!("`{n}::$…`, a member its caller chooses"))
        }
        At::Call
            if !read
                && ident_at(at, n)
                && (matches!(flat.get(at + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis)
                    || (punct_at(at + 1, '!')
                        && matches!(flat.get(at + 2), Some(TokenTree::Group(_))))) =>
        {
            Some(format!("a call to `{n}`"))
        }
        At::Declaration if !read && ident_at(at, "fn") && ident_at(at + 1, n) => {
            Some(format!("`fn {n}`"))
        }
        At::Key
            if !read
                && ident_at(at, n)
                && punct_at(at + 1, ':')
                && matches!(flat.get(at + 2), Some(TokenTree::Literal(_))) =>
        {
            Some(format!("a literal `{n}:`"))
        }
        _ => None,
    })
}

/// Whether the tokens from `at` read `for`, or `<…> for` — the trait half of an
/// `impl`.
fn implemented_at(flat: &[TokenTree], mut at: usize) -> bool {
    // `flatten` keeps a group and then its contents, so a generic argument list
    // is puncts and idents here: skip the balanced `<…>` run.
    if matches!(flat.get(at), Some(TokenTree::Punct(p)) if p.as_char() == '<') {
        let mut depth = 0usize;
        while let Some(tree) = flat.get(at) {
            if let TokenTree::Punct(p) = tree {
                match p.as_char() {
                    '<' => depth += 1,
                    '>' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            at += 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            at += 1;
        }
    }
    matches!(flat.get(at), Some(TokenTree::Ident(i)) if i == "for")
}

/// The idents a `trait` declared at `at` names as supertraits: those after its
/// first `:` and before its body.
fn bounds_of_trait(flat: &[TokenTree], at: usize) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut bounded = false;
    for tree in &flat[at + 1..] {
        match tree {
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => break,
            TokenTree::Punct(p) if p.as_char() == ';' => break,
            TokenTree::Punct(p) if p.as_char() == ':' => bounded = true,
            TokenTree::Ident(ident) if bounded => {
                out.insert(ident.to_string());
            }
            _ => {}
        }
    }
    out
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_item(&mut self, node: &'ast Item) {
        let attrs: &[Attribute] = match node {
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
        };
        if is_cfg_test(attrs) {
            return;
        }
        match node {
            Item::Mod(module) if module.attrs.iter().any(|a| a.path().is_ident("path")) => {
                self.out.insert(format!(
                    "{} :: `#[path]` on `mod {}` — the import resolver finds a module at its \
                     default layout, and a join reading a type's stem off its file reads \
                     another path than the module's",
                    self.file, module.ident,
                ));
            }
            Item::ExternCrate(krate) => {
                if let Some((_, rename)) = &krate.rename
                    && rename != "_"
                {
                    self.out.insert(format!(
                        "{} :: `extern crate {} as {rename}` — the extern prelude carries the \
                         name into every module, and the import resolver reads one module's \
                         imports at a time",
                        self.file, krate.ident,
                    ));
                }
            }
            Item::Use(item) => self.use_tree(&item.tree, &mut Vec::new()),
            // A rename only when the alias *is* the type: `type B = QueueBackend`
            // makes `B::new` a construction the join cannot spell, while a
            // `type GuardSpec = Arc<dyn Guard>` is another type that holds it.
            // A trait has no `type` alias in stable Rust.
            Item::Type(alias) => {
                if let syn::Type::Path(aliased) = &*alias.ty
                    && let Some(last) = aliased.path.segments.last()
                {
                    let segments = &aliased.path.segments;
                    let owner = segments
                        .len()
                        .checked_sub(2)
                        .map(|at| segments[at].ident.to_string());
                    let last = last.ident.to_string();
                    for name in self.matching(&last, owner.as_deref(), &[At::Type]) {
                        self.refuse(
                            format!("`type {} = …{last}` renames it", alias.ident),
                            &name,
                        );
                    }
                }
            }
            Item::Macro(item) if item.mac.path.is_ident("macro_rules") => {
                let rules: Vec<TokenTree> = item.mac.tokens.clone().into_iter().collect();
                // `(matcher) => {transcriber}` — every group right after a `=>`.
                for (at, tree) in rules.iter().enumerate() {
                    if let TokenTree::Group(group) = tree
                        && at >= 2
                        && matches!(&rules[at - 1], TokenTree::Punct(p) if p.as_char() == '>')
                        && matches!(&rules[at - 2], TokenTree::Punct(p) if p.as_char() == '=')
                    {
                        self.transcriber(group.stream());
                    }
                }
            }
            _ => {}
        }
        syn::visit::visit_item(self, node);
    }

    fn visit_attribute(&mut self, node: &'ast Attribute) {
        self.cfg_attr(node);
    }

    // A `#[cfg(test)]` function or impl member is a fixture, as at item level.
    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        let attrs = match node {
            syn::ImplItem::Fn(i) => &i.attrs,
            syn::ImplItem::Const(i) => &i.attrs,
            syn::ImplItem::Type(i) => &i.attrs,
            syn::ImplItem::Macro(i) => &i.attrs,
            _ => return syn::visit::visit_impl_item(self, node),
        };
        if !is_cfg_test(attrs) {
            syn::visit::visit_impl_item(self, node);
        }
    }
}

/// Each construction the join refuses, on a planted tree — every blind spot an
/// audit found at a sibling join, written the way it was found, beside the
/// spellings that stay legal: the names a join reads, written as it reads them.
#[test]
fn each_construction_that_hides_a_name_is_refused() {
    const TREE: [(&str, &str); 6] = [
        (
            "crates/nest-rs-eta-macros/src/lib.rs",
            // macros2-10: one crate-root alias of a sentence, called from another
            // file under its plain name.
            "mod parse;\n\
             pub(crate) use nest_rs_codegen::unknown_argument as unknown;\n\
             use proc_macro2::TokenStream as TokenStream2;\n\
             use std::io::Write as _;\n\
             fn control() { nest_rs_codegen::unknown_argument(\"theta\", \"x\"); }\n",
        ),
        (
            "crates/nest-rs-eta-macros/src/parse.rs",
            "use crate::unknown;\n\
             fn keys(name: &str) { unknown(\"eta\", name); }\n",
        ),
        (
            "crates/nest-rs-delta/src/lib.rs",
            // macros2-6: a backend's claim through a glob of the variants, an
            // alias of either type, an import of a variant, a type alias.
            "use nest_rs_queue::Capability::*;\n\
             use nest_rs_queue::{Capability as Cap, QueueBackend as Backend};\n\
             use nest_rs_queue::Capability::{Throttle};\n\
             type Built = nest_rs_queue::QueueBackend;\n\
             type Holds = std::sync::Arc<QueueBackend>;\n\
             use nest_rs_queue::{Capability, QueueBackend};\n\
             static B: QueueBackend = QueueBackend::new(\"d\", Capabilities::NONE.with(Capability::DelayedPush));\n\
             extern crate serde_json as json;\n\
             extern crate serde;\n\
             use serde_json::*;\n\
             use serde_json as sj;\n\
             #[path = \"elsewhere/hidden.rs\"]\n\
             mod hidden;\n\
             mod inline { use std::error::Error as StdError; use std::io::Error as IoError; }\n\
             #[cfg(test)]\n\
             mod tests { use nest_rs_queue::Capability as C; macro_rules! m { () => { #[module] struct X; } } }\n\
             #[cfg(not(test))]\n\
             mod shipped { use tracing::warn as w; }\n",
        ),
        (
            "crates/nest-rs-zeta/src/zz.rs",
            // macros2-5: a `#[module]` a `macro_rules!` writes, and every other
            // shape a transcriber can hide a followed item in.
            "macro_rules! declare { () => { #[module] pub struct HiddenModule; }; }\n\
             macro_rules! setup { ($t:ty) => { impl ::nest_rs_core::DynamicModule for $t {} }; }\n\
             macro_rules! claim { ($v:ident) => { Capabilities::NONE.with(Capability::$v) }; }\n\
             macro_rules! seam { () => { impl M { pub fn for_root() {} } }; }\n\
             macro_rules! open { () => { nest_rs_core::operation_span!(target: T, kind: K, U, &c) }; }\n\
             macro_rules! fine { ($e:expr) => { tracing::warn!(target: T, \"m\"); Capability::Throttle }; }\n\
             #[cfg_attr(feature = \"x\", module)]\n\
             pub struct Wrapped;\n\
             #[cfg_attr(feature = \"x\", derive(Debug))]\n\
             pub struct Fine;\n",
        ),
        ("crates/nest-rs-zeta/src/broken.rs", "fn () {}"),
        ("crates/nest-rs-zeta/src/lib.rs", "mod zz;\nmod broken;\n"),
    ];
    let root =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("blinds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let dirs =
        ["eta-macros", "delta", "zeta"].map(|name| root.join(format!("crates/nest-rs-{name}")));
    let followed = BTreeMap::from([
        (
            Followed::call("unknown_argument").read_in_transcribers(),
            BTreeSet::from(["grammars"]),
        ),
        (
            Followed::type_("Capability").read_in_transcribers(),
            BTreeSet::from(["queue_capabilities"]),
        ),
        (
            Followed::type_("QueueBackend").read_in_transcribers(),
            BTreeSet::from(["queue_capabilities"]),
        ),
        (Followed::krate("serde_json"), BTreeSet::from(["decodes"])),
        (Followed::attribute("module"), BTreeSet::from(["naming"])),
        (
            Followed::implemented("DynamicModule"),
            BTreeSet::from(["naming"]),
        ),
        (
            Followed::implemented("Error").through(&["error"]),
            BTreeSet::from(["naming"]),
        ),
        (Followed::declaration("for_root"), BTreeSet::from(["seams"])),
        (Followed::call("operation_span"), BTreeSet::from(["units"])),
        (
            Followed::call("warn")
                .through(&["tracing"])
                .read_in_transcribers(),
            BTreeSet::from(["events"]),
        ),
    ]);
    let (found, read) = blinds_in(&root, &dirs, &followed);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(read, 6);
    let delta = "crates/nest-rs-delta/src/lib.rs";
    let zeta = "crates/nest-rs-zeta/src/zz.rs";
    let expected = [
        "crates/nest-rs-eta-macros/src/lib.rs :: `use nest_rs_codegen::unknown_argument as \
         unknown` renames it — blinds grammars"
            .to_owned(),
        format!("{delta} :: `#[path]` on `mod hidden` — the import resolver finds a module at its default layout, and a join reading a type's stem off its file reads another path than the module's"),
        format!("{delta} :: `extern crate serde_json as json` — the extern prelude carries the name into every module, and the import resolver reads one module's imports at a time"),
        format!("{delta} :: `type Built = …QueueBackend` renames it — blinds queue_capabilities"),
        format!("{delta} :: `use nest_rs_queue::Capability as Cap` renames it — blinds queue_capabilities"),
        format!("{delta} :: `use nest_rs_queue::Capability::*` writes a member of `Capability` without `Capability::` — blinds queue_capabilities"),
        format!("{delta} :: `use nest_rs_queue::Capability::Throttle` writes a member of `Capability` without `Capability::` — blinds queue_capabilities"),
        format!("{delta} :: `use nest_rs_queue::QueueBackend as Backend` renames it — blinds queue_capabilities"),
        format!("{delta} :: `use serde_json::*` brings its items in under names the resolver cannot follow — blinds decodes"),
        format!("{delta} :: `use std::error::Error as StdError` renames it — blinds naming"),
        format!("{delta} :: `use tracing::warn as w` renames it — blinds events"),
        "crates/nest-rs-zeta/src/broken.rs :: `syn` cannot parse it (expected identifier) — every join skips it".to_owned(),
        format!("{zeta} :: `#[cfg_attr(…, module…)]` writes it under `cfg_attr`'s path — blinds naming"),
        format!("{zeta} :: a `macro_rules!` writes `Capability::$…`, a member its caller chooses, which no join expands — blinds queue_capabilities"),
        format!("{zeta} :: a `macro_rules!` writes `fn for_root`, which no join expands — blinds seams"),
        format!("{zeta} :: a `macro_rules!` writes a call to `operation_span`, which no join expands — blinds units"),
        format!("{zeta} :: a `macro_rules!` writes an `impl DynamicModule for`, which no join expands — blinds naming"),
        format!("{zeta} :: a `macro_rules!` writes the attribute `#[module]`, which no join expands — blinds naming"),
    ];
    assert_eq!(
        found.into_iter().collect::<Vec<_>>(),
        expected
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
    );
}
