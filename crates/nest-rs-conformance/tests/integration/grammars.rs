//! The grammars join: every decorator whose arguments are `key = value` pairs,
//! against the four refusals such a grammar owes.
//!
//! `CLAUDE.md`: *"Where the standard does not [permit it], that key is a
//! **compile error naming the fact that makes it impossible** — never an ignored
//! argument, never a bare 'unknown key'."* And the testable form, one paragraph
//! down: *"**Refusals are shared, not per key.** One helper, one sentence, every
//! key it covers, one trybuild snapshot per site. Per-key refusals multiply with
//! the matrix, and what multiplies is what gets skipped."*
//!
//! **This family had no join, and that is why six silences were live at once.**
//! Every other family here caught its holes the day a join landed. The four
//! sentences live in `nest_rs_codegen::args`, seven decorators adopted all of
//! them, and four adopted none — `#[expose]`, `#[api]`, GraphQL's `#[authorize]`
//! and `#[inject]` each took a repeated key and dropped one of the two
//! declarations by source order. On `#[api]` that is published prose
//! (`CLAUDE.md` mandates the attribute *instead of* a doc comment); on
//! `#[authorize]` it is which service loads the authorized subject.
//!
//! Members are derived, never listed: a decorator is in the population when the
//! code refuses an unknown key for it through the family's sentence,
//! `nest_rs_codegen::unknown_argument`, which is precisely "this decorator has a
//! key set it refuses strangers against". A decorator taking no arguments at all
//! is not in the family and owes none of this — `DecoratorPair::reject_args` is
//! its whole grammar, and the `shapes` join owns that.
//!
//! **And no decorator can leave the population without failing a test.** The
//! literal written as the sentence's first argument was once the whole
//! derivation, and it was blind to a shape the framework already used: since the
//! job-key table the worker-job family names its decorators through
//! `JobDecorator::name()`, so `#[every]`, `#[cron]` and `#[after]` refused
//! through the sentence and were never joined. So the join *reads* the decorator
//! a call names ([`Sources::reading`]), and it reads three shapes:
//!
//! - a string literal names one decorator;
//! - a value of a table `nest_rs_codegen` declares names the member its variant
//!   spells, or every member when only data decides which ([`tables`]);
//! - a parameter of the function the call sits in makes that function a way to
//!   the sentence — `unmatched_meta`, `job_key` — whose own callers are then read
//!   the same way.
//!
//! Any other shape is a call the join cannot read, and
//! [`every_call_to_a_family_sentence_names_its_decorator_readably`] fails naming
//! it rather than letting its decorator drop out; and
//! [`every_member_of_a_codegen_table_is_joined`] reads the tables against the
//! population, so a member whose parser stops reaching the sentence fails the
//! day it does.
//!
//! **One grammar is outside by design, and it is not the framework's.**
//! `#[tool]` and `#[prompt]` hand their arguments to rmcp's own attribute
//! untouched — the framework reads `description` and `name` and nothing else,
//! so that nothing has to know rmcp's key set — and rmcp refuses an unknown or a
//! repeated key itself, in darling's words (`FromMeta`, unknown fields denied).
//! Joining them would mean recopying that key set, which moves with rmcp.
//!
//! **Two obligations, not four columns**, and the merge is argued rather than a
//! shortcut: `unmatched_meta` answers the bare-key and unknown-key questions in
//! one call by design ("the two questions are answered here, together, once"),
//! so a site adopting it satisfies both, and a site hand-rolling either owes
//! both separately. The join therefore asks for the *sentence*, whichever helper
//! produced it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nest_rs_codegen::JobDecorator;
use nest_rs_conformance::baseline;
use nest_rs_conformance::sources::{
    files_with_extension, flatten, is_cfg_test, read, relative, repo_root, rust_files,
};
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use syn::visit::Visit;

const BASELINE: &str = "grammars-baseline.txt";

/// Decorators with a `key = value` grammar. Below this the scan is reading the
/// wrong tree.
const FLOOR: usize = 10;

/// The unknown-key sentence: a call to it is what enrols a decorator.
const UNKNOWN_KEY: &str = "unknown_argument";

/// The repeated-key sentence. `reject_duplicate_argument` is its guard and is
/// read as a way to it, not listed beside it.
const REPEATED_KEY: &str = "duplicate_argument";

/// The crate that words the sentences, whose functions every grammar crate
/// reaches — so a way to the sentence declared there is one for all of them.
const CODEGEN: &str = "nest-rs-codegen";

/// How deep a `let` or a `const` is followed before the argument is called
/// unreadable — far past any chain the tree holds, and short of a cycle.
const DEPTH: usize = 4;

/// The tables of decorators `nest_rs_codegen` declares, keyed by the type a
/// function takes one as, each member as `(variant, attribute)`.
///
/// Read through the crate rather than recopied: `job.rs` is the one statement of
/// the job family, and a second list of its names here is the copy that drifts.
/// A table added to the codegen reaches this join as a call it cannot read — a
/// decorator passed as a type this map does not name — until it is enrolled
/// here, which is the reviewed act a new table owes.
fn tables() -> BTreeMap<String, Vec<(String, String)>> {
    BTreeMap::from([(
        "JobDecorator".to_owned(),
        JobDecorator::ALL
            .into_iter()
            .map(|member| (format!("{member:?}"), member.name().to_owned()))
            .collect(),
    )])
}

/// `crates/*-macros`, plus `nest-rs-codegen` — which holds `#[inject]`'s own
/// grammar and is where the sentences are worded, so excluding it would exempt
/// the one site best placed to know better.
fn macro_crates(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.join("crates").join(CODEGEN)];
    let Ok(entries) = std::fs::read_dir(root.join("crates")) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("-macros"))
        {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// One function of a grammar crate, as far as reading a decorator needs it.
struct Scope {
    /// The crate's directory name.
    krate: String,
    /// The file, from the repository root — what a finding names.
    file: String,
    name: String,
    /// The typed parameters in order, as `(binding, type)` — the type as
    /// written, whitespace removed. A receiver is not an argument and is left
    /// out, so an index here is an index into a call's arguments.
    params: Vec<(String, String)>,
    /// Every `let` in the body, binding to each initializer.
    lets: BTreeMap<String, Vec<Vec<TokenTree>>>,
    /// Every call the body makes, as the callee's name and its arguments.
    ///
    /// **Read off tokens, not off the expression tree**, because a call sits
    /// inside `format!` and `vec!` as readily as outside, and a macro's body is
    /// tokens to `syn`. A text scan is still not enough: it cannot tell a call
    /// from the doc comment above it — `args.rs` names four of its own helpers in
    /// prose.
    calls: Vec<(String, Vec<Vec<TokenTree>>)>,
}

/// A `const` or `static` of a grammar crate — its type as written and its
/// initializer — keyed by crate and name.
type Consts = BTreeMap<(String, String), (String, Vec<TokenTree>)>;

/// Every function and constant of the grammar crates below `root`.
///
/// `#[cfg(test)]` items are out: a unit test's literal names a fixture — `"probe"`
/// — and never a decorator a developer writes.
struct Sources {
    scopes: Vec<Scope>,
    consts: Consts,
}

impl Sources {
    fn collect(root: &Path) -> Self {
        let mut sources = Sources {
            scopes: Vec::new(),
            consts: Consts::new(),
        };
        for dir in macro_crates(root) {
            let Some(krate) = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            for path in rust_files(&dir.join("src")) {
                let Ok(text) = read(&path) else { continue };
                let Ok(ast) = syn::parse_file(&text) else {
                    continue;
                };
                let mut visit = Collect {
                    krate: &krate,
                    file: relative(&path, root),
                    sources: &mut sources,
                };
                visit.visit_file(&ast);
            }
        }
        sources
    }

    /// Every decorator the code names at `sentence`, with the crates naming it.
    ///
    /// A fixpoint over the ways to the sentence: the sentence itself first, then
    /// every function a call reaches it through by handing on a parameter of its
    /// own, until a pass finds no new one. Each call to one of them is then read
    /// for the decorator it names — see [`Sources::resolve`].
    fn reading(&self, sentence: &str) -> Reading {
        let tables = tables();
        let mut ways: BTreeMap<(Option<String>, String), Way> =
            BTreeMap::from([((None, sentence.to_owned()), Way { at: 0, table: None })]);
        loop {
            let mut reading = Reading::default();
            let mut found = Vec::new();
            for scope in &self.scopes {
                for (callee, args) in &scope.calls {
                    let Some(way) = way_to(&ways, &scope.krate, callee) else {
                        continue;
                    };
                    let written = args
                        .get(way.at)
                        .map_or_else(String::new, |arg| spelled(arg));
                    let names = match args.get(way.at) {
                        Some(arg) => self.resolve(arg, scope, &tables, 0),
                        None => vec![Named::Unreadable(written.clone())],
                    };
                    for named in names {
                        match named {
                            Named::Member(name) => reading.name(&name, &scope.krate),
                            Named::Table(table) => reading.name_all(&tables[&table], &scope.krate),
                            Named::Param(index) => {
                                let (_, ty) = &scope.params[index];
                                match type_named(ty, &tables) {
                                    Some(table) => {
                                        found.push((
                                            owner(scope),
                                            scope.name.clone(),
                                            index,
                                            table,
                                        ));
                                    }
                                    None => {
                                        reading.unreadable.insert(format!(
                                            "{}: `{callee}({written}, …)` hands on a `{ty}` from \
                                             `{}`'s parameters, and no table declares that type",
                                            scope.file, scope.name,
                                        ));
                                    }
                                }
                            }
                            // A function taking a table value is typed by it, so
                            // whatever the argument is, it is one of the table's
                            // members — and which one is data.
                            Named::Unreadable(_) if way.table.is_some() => {
                                let table = way.table.as_deref().unwrap_or_default();
                                reading.name_all(&tables[table], &scope.krate);
                            }
                            Named::Unreadable(what) => {
                                reading.unreadable.insert(format!(
                                    "{}: `{callee}({what}, …)` in `{}` — a decorator reaches the \
                                     sentence as neither a literal, a table's value nor a \
                                     parameter handed on",
                                    scope.file, scope.name,
                                ));
                            }
                        }
                    }
                }
            }
            let mut grew = false;
            for (krate, name, at, table) in found {
                let table = (table != NAME).then_some(table);
                grew |= ways.insert((krate, name), Way { at, table }).is_none();
            }
            if !grew {
                self.close_unreached(&ways, sentence, &tables, &mut reading);
                return reading;
            }
        }
    }

    /// A way to the sentence nothing calls. A table-typed one names every member
    /// of its table where it is declared — the parameter is still one of them —
    /// and a name-typed one hands on a name nobody gives it, which is a call the
    /// join cannot read: something reaches it by a route it does not see, a
    /// function pointer or a closure.
    fn close_unreached(
        &self,
        ways: &BTreeMap<(Option<String>, String), Way>,
        sentence: &str,
        tables: &BTreeMap<String, Vec<(String, String)>>,
        reading: &mut Reading,
    ) {
        for ((krate, name), way) in ways {
            if name == sentence {
                continue;
            }
            let called = self.scopes.iter().any(|scope| {
                (krate.is_none() || krate.as_deref() == Some(&scope.krate))
                    && scope.calls.iter().any(|(callee, _)| callee == name)
            });
            if called {
                continue;
            }
            let Some(declared) = self.scopes.iter().find(|scope| {
                &scope.name == name && krate.as_deref().is_none_or(|k| k == scope.krate)
            }) else {
                continue;
            };
            match &way.table {
                Some(table) => reading.name_all(&tables[table], &declared.krate),
                None => {
                    reading.unreadable.insert(format!(
                        "{}: `{name}` hands a decorator's name on to the sentence, and no call \
                         to it is read",
                        declared.file,
                    ));
                }
            }
        }
    }

    /// What `arg`, written inside `scope`, names.
    ///
    /// A `&` in front and a `.name()` behind are how a decorator is passed and
    /// how a table's value is spelled, so both are read through. A binding is
    /// followed through its `let`s and a constant through its initializer, to
    /// [`DEPTH`].
    fn resolve(
        &self,
        arg: &[TokenTree],
        scope: &Scope,
        tables: &BTreeMap<String, Vec<(String, String)>>,
        depth: usize,
    ) -> Vec<Named> {
        let arg = stripped(arg);
        let unreadable = || vec![Named::Unreadable(spelled(arg))];
        if depth > DEPTH {
            return unreadable();
        }
        if let [TokenTree::Literal(literal)] = arg {
            return match syn::parse_str::<syn::LitStr>(&literal.to_string()) {
                Ok(text) => vec![Named::Member(text.value())],
                Err(_) => unreadable(),
            };
        }
        if let Some((table, variant, rest)) = table_path(arg, tables) {
            return match tables[&table].iter().find(|(name, _)| *name == variant) {
                Some((_, member)) if rest == 0 => vec![Named::Member(member.clone())],
                _ => vec![Named::Table(table)],
            };
        }
        let [TokenTree::Ident(ident)] = arg else {
            return unreadable();
        };
        let ident = ident.to_string();
        if let Some(index) = scope.params.iter().position(|(name, _)| *name == ident) {
            return vec![Named::Param(index)];
        }
        if let Some(inits) = scope.lets.get(&ident) {
            return inits
                .iter()
                .flat_map(|init| self.resolve(init, scope, tables, depth + 1))
                .collect();
        }
        let constant = self
            .consts
            .get(&(scope.krate.clone(), ident.clone()))
            .or_else(|| self.consts.get(&(CODEGEN.to_owned(), ident)));
        if let Some((ty, init)) = constant {
            let named = self.resolve(init, &Scope::of_constant(scope), tables, depth + 1);
            // A constant typed by a table is one of its members, whatever its
            // initializer spells.
            if let Some(table) = type_named(ty, tables).filter(|t| *t != NAME)
                && named.iter().all(|n| matches!(n, Named::Unreadable(_)))
            {
                return vec![Named::Table(table)];
            }
            return named;
        }
        unreadable()
    }
}

/// How a function reaches the sentence: the argument it takes the decorator
/// at, and the table that argument's type is — `None` for a name.
struct Way {
    at: usize,
    table: Option<String>,
}

/// The way declared in `krate` if there is one, else the codegen's.
fn way_to<'w>(
    ways: &'w BTreeMap<(Option<String>, String), Way>,
    krate: &str,
    callee: &str,
) -> Option<&'w Way> {
    ways.get(&(Some(krate.to_owned()), callee.to_owned()))
        .or_else(|| ways.get(&(None, callee.to_owned())))
}

/// Who can call a function a scope declares: every grammar crate for the
/// codegen's, its own crate for any other.
fn owner(scope: &Scope) -> Option<String> {
    (scope.krate != CODEGEN).then(|| scope.krate.clone())
}

/// What a decorator argument names.
#[derive(Debug, PartialEq)]
enum Named {
    /// One decorator, by the attribute it is written as.
    Member(String),
    /// Every member of a table — a value of it only data decides.
    Table(String),
    /// The parameter at this index of the function the call sits in.
    Param(usize),
    /// Nothing the join can read, as written.
    Unreadable(String),
}

/// The pseudo-table a decorator's *name* is typed as — `&str` — so a way to the
/// sentence records one kind of argument in one field.
const NAME: &str = "&str";

/// The table a type as written is, `NAME` for a string, `None` for anything
/// else. Matched on the last path segment, so a qualified path reads the same.
fn type_named(ty: &str, tables: &BTreeMap<String, Vec<(String, String)>>) -> Option<String> {
    let bare = ty.trim_start_matches('&').trim_start_matches("'static");
    if matches!(bare, "str" | "String") {
        return Some(NAME.to_owned());
    }
    let last = bare.rsplit("::").next().unwrap_or(bare);
    tables.contains_key(last).then(|| last.to_owned())
}

/// `arg` without a leading `&` and a trailing `.name()`.
fn stripped(mut arg: &[TokenTree]) -> &[TokenTree] {
    while let [TokenTree::Punct(p), rest @ ..] = arg
        && p.as_char() == '&'
    {
        arg = rest;
    }
    if let [
        head @ ..,
        TokenTree::Punct(dot),
        TokenTree::Ident(method),
        TokenTree::Group(call),
    ] = arg
        && dot.as_char() == '.'
        && method == "name"
        && call.delimiter() == Delimiter::Parenthesis
        && call.stream().is_empty()
    {
        arg = head;
    }
    arg
}

/// `Table::Variant…` — the table, the variant as written, and how many tokens
/// follow it — through any leading path (`nest_rs_codegen::JobDecorator::Cron`).
fn table_path(
    arg: &[TokenTree],
    tables: &BTreeMap<String, Vec<(String, String)>>,
) -> Option<(String, String, usize)> {
    let mut at = 0;
    loop {
        let [
            TokenTree::Ident(segment),
            TokenTree::Punct(a),
            TokenTree::Punct(b),
            TokenTree::Ident(next),
            ..,
        ] = &arg[at..]
        else {
            return None;
        };
        if a.as_char() != ':' || b.as_char() != ':' {
            return None;
        }
        if tables.contains_key(&segment.to_string()) {
            return Some((segment.to_string(), next.to_string(), arg.len() - at - 4));
        }
        at += 3;
    }
}

/// The tokens as a reader would write them, for a finding.
fn spelled(arg: &[TokenTree]) -> String {
    arg.iter().cloned().collect::<TokenStream>().to_string()
}

impl Scope {
    /// The scope a constant's initializer is read in: its crate, and no
    /// parameter or binding to resolve against.
    fn of_constant(scope: &Scope) -> Scope {
        Scope {
            krate: scope.krate.clone(),
            file: scope.file.clone(),
            name: scope.name.clone(),
            params: Vec::new(),
            lets: BTreeMap::new(),
            calls: Vec::new(),
        }
    }
}

/// What a sentence was found naming.
#[derive(Default, Debug, PartialEq)]
struct Reading {
    /// Every decorator named, with the crates whose code names it.
    members: BTreeMap<String, BTreeSet<String>>,
    /// Every call whose decorator the join could not read.
    unreadable: BTreeSet<String>,
}

impl Reading {
    fn name(&mut self, decorator: &str, krate: &str) {
        self.members
            .entry(decorator.to_owned())
            .or_default()
            .insert(krate.to_owned());
    }

    fn name_all(&mut self, members: &[(String, String)], krate: &str) {
        for (_, decorator) in members {
            self.name(decorator, krate);
        }
    }
}

/// The walk filling [`Sources`] from one file.
struct Collect<'a> {
    krate: &'a str,
    file: String,
    sources: &'a mut Sources,
}

impl Collect<'_> {
    fn scope(&mut self, sig: &syn::Signature, block: &syn::Block) {
        let params = sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Receiver(_) => None,
                syn::FnArg::Typed(typed) => Some((
                    bound(&typed.pat).unwrap_or_default(),
                    typed.ty.to_token_stream().to_string().replace(' ', ""),
                )),
            })
            .collect();
        let mut lets = Lets::default();
        lets.visit_block(block);
        // A nested item is a scope of its own, so its calls are not this one's.
        let body: TokenStream = block
            .stmts
            .iter()
            .filter(|stmt| !matches!(stmt, syn::Stmt::Item(_)))
            .map(ToTokens::to_token_stream)
            .collect();
        self.sources.scopes.push(Scope {
            krate: self.krate.to_owned(),
            file: self.file.clone(),
            name: sig.ident.to_string(),
            params,
            lets: lets.0,
            calls: calls(body),
        });
    }

    fn constant(&mut self, ident: &syn::Ident, ty: &syn::Type, init: &syn::Expr) {
        self.sources.consts.insert(
            (self.krate.to_owned(), ident.to_string()),
            (
                ty.to_token_stream().to_string().replace(' ', ""),
                init.to_token_stream().into_iter().collect(),
            ),
        );
    }
}

impl<'ast> Visit<'ast> for Collect<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !is_cfg_test(&node.attrs) {
            syn::visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if !is_cfg_test(&node.attrs) {
            self.scope(&node.sig, &node.block);
            syn::visit::visit_item_fn(self, node);
        }
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if !is_cfg_test(&node.attrs) {
            self.scope(&node.sig, &node.block);
            syn::visit::visit_impl_item_fn(self, node);
        }
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if let Some(block) = &node.default {
            self.scope(&node.sig, block);
        }
        syn::visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.constant(&node.ident, &node.ty, &node.expr);
        syn::visit::visit_item_const(self, node);
    }

    fn visit_impl_item_const(&mut self, node: &'ast syn::ImplItemConst) {
        self.constant(&node.ident, &node.ty, &node.expr);
        syn::visit::visit_impl_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.constant(&node.ident, &node.ty, &node.expr);
        syn::visit::visit_item_static(self, node);
    }
}

/// Every `let` binding of a body, stopping at a nested item.
#[derive(Default)]
struct Lets(BTreeMap<String, Vec<Vec<TokenTree>>>);

impl<'ast> Visit<'ast> for Lets {
    fn visit_item(&mut self, _: &'ast syn::Item) {}

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let (Some(name), Some(init)) = (bound(&node.pat), &node.init) {
            self.0
                .entry(name)
                .or_default()
                .push(init.expr.to_token_stream().into_iter().collect());
        }
        syn::visit::visit_local(self, node);
    }
}

/// The one name a pattern binds, `None` for a destructuring one.
fn bound(pat: &syn::Pat) -> Option<String> {
    match pat {
        syn::Pat::Ident(pat) => Some(pat.ident.to_string()),
        syn::Pat::Type(pat) => bound(&pat.pat),
        _ => None,
    }
}

/// Every `name(…)` in `body`, at any depth, with its arguments split at the
/// commas between them. `fn name(…)` is a declaration and `name!(…)` a macro,
/// so neither is a call.
fn calls(body: TokenStream) -> Vec<(String, Vec<Vec<TokenTree>>)> {
    let mut flat = Vec::new();
    flatten(body, &mut flat);
    let mut out = Vec::new();
    for (at, window) in flat.windows(2).enumerate() {
        let [TokenTree::Ident(callee), TokenTree::Group(args)] = window else {
            continue;
        };
        let declared = at
            .checked_sub(1)
            .is_some_and(|before| matches!(&flat[before], TokenTree::Ident(i) if i == "fn"));
        if args.delimiter() != Delimiter::Parenthesis || declared {
            continue;
        }
        let mut split: Vec<Vec<TokenTree>> = vec![Vec::new()];
        for tree in args.stream() {
            match &tree {
                TokenTree::Punct(p) if p.as_char() == ',' => split.push(Vec::new()),
                _ => split.last_mut().expect("never empty").push(tree),
            }
        }
        if split.last().is_some_and(Vec::is_empty) {
            split.pop();
        }
        out.push((callee.to_string(), split));
    }
    out
}

/// Every trybuild snapshot in the tree, read once for every cell that asks.
///
/// The `.stderr`, because that is the compiler actually saying it: a helper
/// called in a `src/` file proves the code exists, and only a snapshot proves it
/// is reachable and worded as recorded. Matched on sentence fragments rather
/// than on a file name — `testing.md` clause 3, and this join's siblings record a
/// rename closing a cell that asked for a name.
fn snapshots(root: &Path) -> Vec<String> {
    files_with_extension(&root.join("crates"), "stderr")
        .into_iter()
        .filter_map(|path| read(&path).ok())
        .collect()
}

/// Whether one snapshot carries **every** fragment.
///
/// **All, not any, and the bare-key column is why.** Its needle was `#[{attr}] ``
/// alone, which any refusal opening with the decorator and a backtick
/// satisfies: `#[process] `throttle(window)` takes a duration…` filled the cell
/// for `#[process]`, and the same held at `#[queue]`, `#[controller]`,
/// `#[gateway]`, `#[mcp]` and `#[crud]`. Measured: deleting both real bare-key
/// snapshots **and** the refusal left the cell green, which is the hollow cell
/// `testing.md` ranks below an empty one.
fn snapshotted(snapshots: &[String], needles: &[&str]) -> bool {
    snapshots
        .iter()
        .any(|text| needles.iter().all(|needle| text.contains(needle)))
}

#[test]
fn every_key_value_grammar_refuses_the_four_ways_of_getting_it_wrong() {
    let root = repo_root();
    let sources = Sources::collect(&root);
    let grammars = sources.reading(UNKNOWN_KEY).members;
    let repeats = sources.reading(REPEATED_KEY).members;
    baseline::floor(
        grammars.len(),
        FLOOR,
        "decorator(s) with a key = value grammar",
    );

    let snapshots = snapshots(&root);
    let mut holes = BTreeSet::new();
    let mut cells = 0usize;
    for (attr, crates) in &grammars {
        let duplicate = format!("#[{attr}] takes at most one ");
        let bare = format!("#[{attr}] `");
        let unknown = format!("unknown #[{attr}] argument ");
        // Refused by a crate that words this grammar: another crate's decorator
        // of the same name is not this one.
        let refuses_a_repeat = repeats.get(attr).is_some_and(|by| !by.is_disjoint(crates));
        for (column, present) in [
            (
                "a duplicate key refused, through the shared `reject_duplicate_argument`",
                refuses_a_repeat,
            ),
            (
                "a snapshot pinning the duplicate-key refusal",
                snapshotted(&snapshots, &[&duplicate]),
            ),
            (
                "a snapshot pinning the unknown-key refusal",
                snapshotted(&snapshots, &[&unknown]),
            ),
            (
                "a snapshot pinning the bare-key refusal",
                snapshotted(&snapshots, &[&bare, "needs a value"]),
            ),
        ] {
            cells += 1;
            if !present {
                // **The crate first, and not only for context.** A baseline
                // line starting with `#` is a comment to `baseline::compare`,
                // and a cell keyed `#[authorize] :: …` is exactly that — the
                // two recorded holes read as prose and the join reported them
                // as new on every run. Naming the crate that words the grammar
                // is also the more useful key: it says where to go.
                let owners = crates.iter().cloned().collect::<Vec<_>>().join(", ");
                holes.insert(format!("{owners} #[{attr}] :: {column}"));
            }
        }
    }

    baseline::gate(
        BASELINE,
        &holes,
        cells,
        "cells",
        "decorator × grammar refusal",
        "a way of writing the arguments wrong that this decorator does not name",
    );
}

/// **No decorator leaves the population silently.** Every call to a family
/// sentence names its decorator in a shape the join reads — the population is
/// then every decorator the code refuses for, and a shape it cannot read fails
/// here, naming the call, rather than dropping its decorator from every cell the
/// way `JobDecorator::name()` dropped three.
#[test]
fn every_call_to_a_family_sentence_names_its_decorator_readably() {
    let sources = Sources::collect(&repo_root());
    for sentence in [UNKNOWN_KEY, REPEATED_KEY] {
        let unreadable = sources.reading(sentence).unreadable;
        assert!(
            unreadable.is_empty(),
            "calls to `{sentence}` whose decorator the grammars join cannot read — pass \
             the name as a literal, pass a value of a table this join enrols, or hand on \
             a parameter of the enclosing function; a new table is enrolled in \
             `grammars::tables`:\n  {}",
            unreadable.into_iter().collect::<Vec<_>>().join("\n  "),
        );
    }
}

/// **A decorator the codegen's tables name is in the population.** The table is
/// the one statement of its family, so a member missing from the join is a
/// parser that stopped reaching the unknown-key sentence — or one that never
/// did, which is how `#[every]`, `#[cron]` and `#[after]` stayed out.
#[test]
fn every_member_of_a_codegen_table_is_joined() {
    let grammars = Sources::collect(&repo_root()).reading(UNKNOWN_KEY).members;
    let missing: Vec<String> = tables()
        .into_iter()
        .flat_map(|(table, members)| {
            members
                .into_iter()
                .filter(|(_, name)| !grammars.contains_key(name))
                .map(move |(_, name)| format!("#[{name}] ({table})"))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "decorators a codegen table declares that no call to `{UNKNOWN_KEY}` names — \
         their unknown key is refused outside the family's sentence, or not at all: \
         {missing:?}",
    );
}

/// [`Sources::reading`], on a planted tree: each shape a decorator reaches the
/// sentence in, and the shapes it cannot be read in.
#[test]
fn a_decorator_is_read_through_each_shape_it_reaches_the_sentence_in() {
    const TREE: [(&str, &str); 3] = [
        (
            "crates/nest-rs-codegen/src/args.rs",
            "pub fn unknown_argument(attr: &str, name: &str) -> String { format!(\"{attr}{name}\") }\n\
             pub fn unmatched_meta(attr: &str, name: &str) -> String { unknown_argument(attr, name) }\n\
             pub fn job_key(member: JobDecorator, name: &str) -> String {\n\
                 unknown_argument(member.name(), name)\n\
             }\n\
             #[cfg(test)]\n\
             mod tests { fn probe() { super::unknown_argument(\"probe\", \"x\"); } }",
        ),
        (
            "crates/nest-rs-alpha-macros/src/lib.rs",
            "const OWN: JobDecorator = JobDecorator::Process;\n\
             fn literal() { unknown_argument(\"alpha\", \"x\"); }\n\
             fn met(meta: &str) { nest_rs_codegen::unmatched_meta(\"beta\", meta); }\n\
             fn constant() { job_key(OWN, \"x\"); }\n\
             fn variant() { job_key(JobDecorator::Cron, \"x\"); }\n\
             fn forwarded(attr: &str) { unknown_argument(attr, \"x\"); }\n\
             fn caller() { forwarded(\"gamma\"); }\n\
             fn computed(key: &str) { unknown_argument(&format!(\"#{key}\"), \"x\"); }",
        ),
        (
            "crates/nest-rs-beta-macros/src/lib.rs",
            "fn keys(stream: Stream, member: JobDecorator) { job_key(member, \"x\"); }\n\
             fn entry(key: &str) {\n\
                 let member = JobDecorator::named(key).unwrap();\n\
                 keys(stream, member);\n\
             }\n\
             fn unreached(attr: &str) { let named = attr; unknown_argument(named, \"x\"); }\n\
             fn closure() { let each = |attr: &str| unknown_argument(attr, \"x\"); }",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("grammars-shapes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let reading = Sources::collect(&root).reading(UNKNOWN_KEY);
    let _ = std::fs::remove_dir_all(&root);

    let crates = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    let every_job = ["process", "every", "cron", "after"];
    let mut members: BTreeMap<String, BTreeSet<String>> = BTreeMap::from([
        ("alpha".to_owned(), crates(&["nest-rs-alpha-macros"])),
        ("beta".to_owned(), crates(&["nest-rs-alpha-macros"])),
        ("gamma".to_owned(), crates(&["nest-rs-alpha-macros"])),
    ]);
    for job in every_job {
        members.insert(job.to_owned(), crates(&["nest-rs-beta-macros"]));
    }
    members
        .get_mut("process")
        .expect("planted")
        .insert("nest-rs-alpha-macros".to_owned());
    members
        .get_mut("cron")
        .expect("planted")
        .insert("nest-rs-alpha-macros".to_owned());

    assert_eq!(reading.members, members);
    assert_eq!(
        reading.unreadable.into_iter().collect::<Vec<_>>(),
        [
            "crates/nest-rs-alpha-macros/src/lib.rs: `unknown_argument(format ! (\"#{key}\"), …)` \
             in `computed` — a decorator reaches the sentence as neither a literal, a table's \
             value nor a parameter handed on",
            "crates/nest-rs-beta-macros/src/lib.rs: `unknown_argument(attr, …)` in `closure` — a \
             decorator reaches the sentence as neither a literal, a table's value nor a parameter \
             handed on",
            "crates/nest-rs-beta-macros/src/lib.rs: `unreached` hands a decorator's name on to \
             the sentence, and no call to it is read",
        ],
    );
}
