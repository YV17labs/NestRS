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
//! A call is read under the name it is **written** as. A local `macro_rules!`
//! is a way of its own — each rule a scope whose metavariables hand a name on
//! exactly as a parameter does, and each `name!(…)` invocation a call to it. An
//! import alias of a sentence (`use nest_rs_codegen::unknown_argument as
//! unknown;`) is the one spelling that renames it outright, and it is refused in
//! framework source by the `blinds` join rather than followed here: followed per
//! file, an alias declared once at a crate root and called from another file
//! still dropped its decorator from the population.
//!
//! **A decorator name is not a grammar.** `#[expose]` is two — the one on the
//! `Model` and the one on a column — worded in one crate, and the struct half's
//! repeat refusal and snapshots filled the field half's cells, which refused
//! nothing. So where a call lists its key set readably, that set is the
//! grammar's identity, and each cell is filled only by what names *its* keys
//! ([`key_sets`]). The job family's key sets are its table's columns, read
//! through the codegen rather than off a call.
//!
//! **A repeat is refused for every key, or the cell is empty.** Every grammar
//! takes each key it reads through one guard, `nest_rs_codegen::WrittenKeys::
//! take_key`, which refuses an unknown key and a repeated one in the same call —
//! so a repeat is refused for every key of the grammar by construction, and the
//! cell asks whether the grammar is taken through it ([`guarded`]). It asked
//! whether *any* key of the grammar had a refusal of its own before, and deleting
//! `#[expose]`'s `via` refusal — the key that loads the wrong foreign key's rows
//! when its second spelling wins — left it green. A guard is derived, not named:
//! a function reaching both sentences with the decorator in the same parameter.
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
    files_with_extension, is_cfg_test, read, relative, repo_root, rust_files,
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

/// The repeated-key sentence. `WrittenKeys::take_key` reaches it and the
/// unknown-key one in the same call, which is what makes it the grammars' guard
/// ([`guards`]).
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
        // The unknown-key sentence lists the grammar's keys third; the repeat
        // sentence takes none.
        let keys = (sentence == UNKNOWN_KEY).then_some(2);
        let mut ways: BTreeMap<(Option<String>, String), Way> = BTreeMap::from([(
            (None, sentence.to_owned()),
            Way {
                at: 0,
                table: None,
                keys,
            },
        )]);
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
                                        // The key set handed on the same way, when
                                        // it is a parameter of this function too.
                                        let keys = way
                                            .keys
                                            .and_then(|k| args.get(k))
                                            .and_then(|arg| param_of(arg, scope));
                                        found.push((
                                            owner(scope),
                                            scope.name.clone(),
                                            index,
                                            table,
                                            keys,
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
            for (krate, name, at, table, keys) in found {
                let table = (table != NAME).then_some(table);
                grew |= ways
                    .insert((krate, name), Way { at, table, keys })
                    .is_none();
            }
            if !grew {
                self.close_unreached(&ways, sentence, &tables, &mut reading);
                reading.ways = ways;
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
        // A `macro_rules!` metavariable, `$attr`, is its rule's parameter.
        let arg = match arg {
            [TokenTree::Punct(dollar), rest @ ..]
                if dollar.as_char() == '$' && matches!(rest, [TokenTree::Ident(_)]) =>
            {
                rest
            }
            other => other,
        };
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
/// at, the table that argument's type is — `None` for a name — and the argument
/// it takes the grammar's key set at, when one reaches the unknown-key
/// sentence's list.
#[derive(Clone)]
struct Way {
    at: usize,
    table: Option<String>,
    keys: Option<usize>,
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

/// The index of the parameter of `scope` an argument names — `keys` or
/// `&keys` — or `None` for anything else.
fn param_of(arg: &[TokenTree], scope: &Scope) -> Option<usize> {
    match stripped(arg) {
        [TokenTree::Ident(ident)] => scope
            .params
            .iter()
            .position(|(name, _)| ident == name.as_str()),
        _ => None,
    }
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
/// follow it — through any leading path, absolute or not
/// (`::nest_rs_codegen::JobDecorator::Cron`).
fn table_path(
    mut arg: &[TokenTree],
    tables: &BTreeMap<String, Vec<(String, String)>>,
) -> Option<(String, String, usize)> {
    if let [TokenTree::Punct(a), TokenTree::Punct(b), rest @ ..] = arg
        && a.as_char() == ':'
        && b.as_char() == ':'
    {
        arg = rest;
    }
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
#[derive(Default)]
struct Reading {
    /// Every decorator named, with the crates whose code names it.
    members: BTreeMap<String, BTreeSet<String>>,
    /// Every call whose decorator the join could not read.
    unreadable: BTreeSet<String>,
    /// Every function reaching the sentence, keyed by the crate that may call
    /// it (`None` for the codegen's) and its name.
    ways: BTreeMap<(Option<String>, String), Way>,
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

    /// One scope per rule of a `macro_rules! name`, named `name!`: its
    /// metavariables, in the order the matcher binds them, are its parameters —
    /// typed as a name, since a rule hands on whatever it is given — and its
    /// transcriber is its body.
    fn macro_rules(&mut self, name: &syn::Ident, body: TokenStream) {
        let trees: Vec<TokenTree> = body.into_iter().collect();
        let mut at = 0;
        while let (
            Some(TokenTree::Group(matcher)),
            Some(TokenTree::Punct(eq)),
            Some(TokenTree::Punct(gt)),
            Some(TokenTree::Group(transcriber)),
        ) = (
            trees.get(at),
            trees.get(at + 1),
            trees.get(at + 2),
            trees.get(at + 3),
        ) {
            if eq.as_char() != '=' || gt.as_char() != '>' {
                break;
            }
            let mut flat = Vec::new();
            nest_rs_conformance::sources::flatten(matcher.stream(), &mut flat);
            let params = flat
                .windows(2)
                .filter_map(|pair| match pair {
                    [TokenTree::Punct(dollar), TokenTree::Ident(var)]
                        if dollar.as_char() == '$' =>
                    {
                        Some((var.to_string(), NAME.to_owned()))
                    }
                    _ => None,
                })
                .collect();
            self.sources.scopes.push(Scope {
                krate: self.krate.to_owned(),
                file: self.file.clone(),
                name: format!("{name}!"),
                params,
                lets: BTreeMap::new(),
                calls: calls(transcriber.stream()),
            });
            at += 4;
            if matches!(trees.get(at), Some(TokenTree::Punct(p)) if p.as_char() == ';') {
                at += 1;
            }
        }
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
    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        if node.mac.path.is_ident("macro_rules")
            && let Some(name) = &node.ident
            && !is_cfg_test(&node.attrs)
        {
            self.macro_rules(name, node.mac.tokens.clone());
        }
        syn::visit::visit_item_macro(self, node);
    }

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
/// commas between them — `name::<T>(…)` included, because
/// `reject_duplicate_argument` is generic and a turbofish must not hide a call
/// from the join. `fn name(…)` is a declaration, so it is not a call. A macro
/// invocation `name!(…)` is recorded as a call to `name!` — a name no function
/// can have, so it only ever reaches a local `macro_rules!` way.
fn calls(body: TokenStream) -> Vec<(String, Vec<Vec<TokenTree>>)> {
    let mut out = Vec::new();
    calls_in(body, &mut out);
    out
}

fn calls_in(tokens: TokenStream, out: &mut Vec<(String, Vec<Vec<TokenTree>>)>) {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    for (at, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Group(group) => calls_in(group.stream(), out),
            TokenTree::Ident(callee) => {
                let declared = at.checked_sub(1).is_some_and(
                    |before| matches!(&trees[before], TokenTree::Ident(i) if i == "fn"),
                );
                if declared {
                    continue;
                }
                if let Some(TokenTree::Group(args)) = trees.get(past_turbofish(&trees, at + 1))
                    && args.delimiter() == Delimiter::Parenthesis
                {
                    out.push((callee.to_string(), arguments(args.stream())));
                }
                if let (Some(TokenTree::Punct(bang)), Some(TokenTree::Group(args))) =
                    (trees.get(at + 1), trees.get(at + 2))
                    && bang.as_char() == '!'
                    && args.delimiter() != Delimiter::None
                {
                    out.push((format!("{callee}!"), arguments(args.stream())));
                }
            }
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

/// The index just past a turbofish opening at `at` — `::<A, B<C>>` — or `at`
/// itself when none does. An arrow's `>` (`fn() -> T`) closes nothing.
fn past_turbofish(trees: &[TokenTree], at: usize) -> usize {
    let opens = matches!(
        trees.get(at..at + 3),
        Some([TokenTree::Punct(a), TokenTree::Punct(b), TokenTree::Punct(c)])
            if a.as_char() == ':' && b.as_char() == ':' && c.as_char() == '<'
    );
    if !opens {
        return at;
    }
    let mut depth = 0usize;
    let mut arrow = false;
    for (offset, tree) in trees.iter().enumerate().skip(at + 2) {
        let TokenTree::Punct(punct) = tree else {
            arrow = false;
            continue;
        };
        match punct.as_char() {
            '<' => depth += 1,
            '>' if !arrow => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return offset + 1;
                }
            }
            _ => {}
        }
        arrow = punct.as_char() == '-';
    }
    at
}

/// A call's arguments, split at the commas between them.
fn arguments(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut split: Vec<Vec<TokenTree>> = vec![Vec::new()];
    for tree in stream {
        match &tree {
            TokenTree::Punct(p) if p.as_char() == ',' => split.push(Vec::new()),
            _ => split.last_mut().expect("never empty").push(tree),
        }
    }
    if split.last().is_some_and(Vec::is_empty) {
        split.pop();
    }
    split
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

/// Every key set a grammar's calls list, per decorator and crate — the
/// grammar's identity where one decorator name carries several.
///
/// Read off every call to a way to the unknown-key sentence that takes the key
/// set — `unknown_argument`, `unmatched_meta`, `WrittenKeys::take_key` — when the
/// list is readable: a literal array, or a binding or a constant holding one.
/// The job family's key sets are its table's columns, which the codegen states
/// and the parsers never spell ([`table_key_sets`]). Any other grammar is keyed
/// by its decorator alone.
fn key_sets(sources: &Sources) -> BTreeMap<(String, String), BTreeSet<Vec<String>>> {
    let tables = tables();
    let reading = sources.reading(UNKNOWN_KEY);
    let ways = &reading.ways;
    let mut out: BTreeMap<(String, String), BTreeSet<Vec<String>>> = BTreeMap::new();
    for scope in &sources.scopes {
        for (callee, args) in &scope.calls {
            let Some(way) = way_to(ways, &scope.krate, callee) else {
                continue;
            };
            let (Some(attr), Some(keys)) = (
                args.get(way.at),
                way.keys
                    .and_then(|k| args.get(k))
                    .and_then(|list| sources.listed(list, scope, 0)),
            ) else {
                continue;
            };
            for named in sources.resolve(attr, scope, &tables, 0) {
                if let Named::Member(attr) = named {
                    out.entry((attr, scope.krate.clone()))
                        .or_default()
                        .insert(keys.clone());
                }
            }
        }
    }
    for (attr, keys) in table_key_sets() {
        for krate in reading.members.get(&attr).into_iter().flatten() {
            out.entry((attr.clone(), krate.clone()))
                .or_default()
                .insert(keys.clone());
        }
    }
    out
}

/// The key set each member of a codegen table takes — its column, read through
/// the codegen's own accessor rather than recopied.
fn table_key_sets() -> Vec<(String, Vec<String>)> {
    JobDecorator::ALL
        .into_iter()
        .map(|member| {
            let mut keys: Vec<String> = nest_rs_codegen::job_keys(member)
                .map(|key| key.name().to_owned())
                .collect();
            keys.sort();
            (member.name().to_owned(), keys)
        })
        .collect()
}

/// The guards: every function reaching **both** sentences with the decorator
/// in the same argument — the one call that refuses an unknown key and a
/// repeated one together, so a grammar taken through it refuses a repeat for
/// every key it has. `WrittenKeys::take_key` is one, and so is anything handing
/// a decorator on to it (`job_key`).
fn guards(sources: &Sources) -> BTreeMap<(Option<String>, String), Way> {
    let unknown = sources.reading(UNKNOWN_KEY).ways;
    let repeated = sources.reading(REPEATED_KEY).ways;
    unknown
        .into_iter()
        .filter(|(key, way)| {
            key.1 != UNKNOWN_KEY && repeated.get(key).is_some_and(|other| other.at == way.at)
        })
        .collect()
}

/// Every grammar taken through a guard, per decorator and crate, with the key
/// set the guard is handed — a table guard's member's column, which the guard
/// reads itself (`job_key`), or `None` where the list is unreadable.
fn guarded(sources: &Sources) -> BTreeMap<(String, String), BTreeSet<Option<Vec<String>>>> {
    let tables = tables();
    let columns: BTreeMap<String, Vec<String>> = table_key_sets().into_iter().collect();
    let guards = guards(sources);
    let mut out: BTreeMap<(String, String), BTreeSet<Option<Vec<String>>>> = BTreeMap::new();
    for scope in &sources.scopes {
        for (callee, args) in &scope.calls {
            let Some(guard) = way_to(&guards, &scope.krate, callee) else {
                continue;
            };
            let Some(attr) = args.get(guard.at) else {
                continue;
            };
            let keys = guard
                .keys
                .and_then(|k| args.get(k))
                .and_then(|list| sources.listed(list, scope, 0));
            for named in sources.resolve(attr, scope, &tables, 0) {
                let members = match named {
                    Named::Member(attr) => vec![attr],
                    Named::Table(table) => tables[&table]
                        .iter()
                        .map(|(_, name)| name.clone())
                        .collect(),
                    Named::Param(_) | Named::Unreadable(_) => continue,
                };
                for member in members {
                    let keys = match (&keys, &guard.table) {
                        (None, Some(_)) => columns.get(&member).cloned(),
                        _ => keys.clone(),
                    };
                    out.entry((member, scope.krate.clone()))
                        .or_default()
                        .insert(keys);
                }
            }
        }
    }
    out
}

impl Sources {
    /// The string literals a key-set argument lists, read through a `&`, a
    /// binding and a constant.
    fn listed(&self, arg: &[TokenTree], scope: &Scope, depth: usize) -> Option<Vec<String>> {
        if depth > DEPTH {
            return None;
        }
        match stripped(arg) {
            [TokenTree::Group(group)] if group.delimiter() == Delimiter::Bracket => {
                let mut keys = Vec::new();
                for item in arguments(group.stream()) {
                    let [TokenTree::Literal(literal)] = item.as_slice() else {
                        return None;
                    };
                    keys.push(
                        syn::parse_str::<syn::LitStr>(&literal.to_string())
                            .ok()?
                            .value(),
                    );
                }
                keys.sort();
                Some(keys)
            }
            [TokenTree::Ident(ident)] => {
                let ident = ident.to_string();
                if let Some([init]) = scope.lets.get(&ident).map(Vec::as_slice) {
                    return self.listed(init, scope, depth + 1);
                }
                let (_, init) = self
                    .consts
                    .get(&(scope.krate.clone(), ident.clone()))
                    .or_else(|| self.consts.get(&(CODEGEN.to_owned(), ident)))?;
                self.listed(init, &Scope::of_constant(scope), depth + 1)
            }
            _ => None,
        }
    }
}

/// Whether a snapshot line opening with `prefix` names, in the backticked span
/// right after it, one of `keys` — `#[process] takes at most one `throttle(limit)``
/// names `limit`.
fn names_a_key(snapshots: &[String], prefix: &str, suffix: &str, keys: &[String]) -> bool {
    snapshots.iter().flat_map(|text| text.lines()).any(|line| {
        let Some(at) = line.find(prefix) else {
            return false;
        };
        let rest = &line[at + prefix.len()..];
        let Some(end) = rest.find('`') else {
            return false;
        };
        let named = &rest[..end];
        line.contains(suffix) && keys.iter().any(|key| named.contains(key.as_str()))
    })
}

/// Whether a snapshot's unknown-key refusal for `attr` offers every one of
/// `keys` — the key set that line lists is the grammar that refused.
fn offers_every_key(snapshots: &[String], attr: &str, keys: &[String]) -> bool {
    let unknown = format!("unknown #[{attr}] argument ");
    snapshots.iter().flat_map(|text| text.lines()).any(|line| {
        line.contains(&unknown)
            && line.split_once("; expected ").is_some_and(|(_, offered)| {
                keys.iter().all(|key| offered.contains(&format!("`{key}`")))
            })
    })
}

#[test]
fn every_key_value_grammar_refuses_the_four_ways_of_getting_it_wrong() {
    let root = repo_root();
    let sources = Sources::collect(&root);
    let grammars = sources.reading(UNKNOWN_KEY).members;
    baseline::floor(
        grammars.len(),
        FLOOR,
        "decorator(s) with a key = value grammar",
    );

    let snapshots = snapshots(&root);
    let key_sets = key_sets(&sources);
    let guarded = guarded(&sources);
    let mut holes = BTreeSet::new();
    let mut cells = 0usize;
    for (attr, crates) in &grammars {
        // One decorator name, one grammar per key set its crates list — or one
        // grammar keyed by the name alone where no key set is readable.
        let mut by_keys: BTreeMap<Option<Vec<String>>, BTreeSet<String>> = BTreeMap::new();
        for krate in crates {
            match key_sets.get(&(attr.clone(), krate.clone())) {
                Some(sets) => {
                    for keys in sets {
                        by_keys
                            .entry(Some(keys.clone()))
                            .or_default()
                            .insert(krate.clone());
                    }
                }
                None => {
                    by_keys.entry(None).or_default().insert(krate.clone());
                }
            }
        }
        let several = by_keys.len() > 1;
        for (keys, owners) in &by_keys {
            let duplicate = format!("#[{attr}] takes at most one ");
            let bare = format!("#[{attr}] `");
            let unknown = format!("unknown #[{attr}] argument ");
            // Taken through a guard by a crate that words this grammar, with
            // this grammar's key set — or, for a grammar keyed by its name
            // alone, with any.
            let refuses_a_repeat = owners.iter().any(|krate| {
                guarded
                    .get(&(attr.clone(), krate.clone()))
                    .is_some_and(|sets| match keys {
                        Some(keys) => {
                            sets.contains(&Some(keys.clone())) || (sets.contains(&None) && !several)
                        }
                        None => !sets.is_empty(),
                    })
            });
            let (repeat_pinned, unknown_pinned, bare_pinned) = match keys {
                Some(keys) => (
                    names_a_key(&snapshots, &format!("{duplicate}`"), "", keys),
                    offers_every_key(&snapshots, attr, keys),
                    names_a_key(&snapshots, &bare, "needs a value", keys),
                ),
                None => (
                    snapshotted(&snapshots, &[&duplicate]),
                    snapshotted(&snapshots, &[&unknown]),
                    snapshotted(&snapshots, &[&bare, "needs a value"]),
                ),
            };
            for (column, present) in [
                (
                    "every key taken through the shared `WrittenKeys::take_key`, which \
                     refuses a repeat",
                    refuses_a_repeat,
                ),
                (
                    "a snapshot pinning the duplicate-key refusal",
                    repeat_pinned,
                ),
                ("a snapshot pinning the unknown-key refusal", unknown_pinned),
                ("a snapshot pinning the bare-key refusal", bare_pinned),
            ] {
                cells += 1;
                if !present {
                    // **The crate first, and not only for context.** A baseline
                    // line starting with `#` is a comment to `baseline::compare`,
                    // and a cell keyed `#[authorize] :: …` is exactly that — the
                    // two recorded holes read as prose and the join reported them
                    // as new on every run. Naming the crate that words the grammar
                    // is also the more useful key: it says where to go. The key
                    // set joins it only where one name carries several grammars.
                    let owners = owners.iter().cloned().collect::<Vec<_>>().join(", ");
                    let grammar = match (several, keys) {
                        (true, Some(keys)) => format!(" ({})", keys.join(", ")),
                        _ => String::new(),
                    };
                    holes.insert(format!("{owners} #[{attr}]{grammar} :: {column}"));
                }
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
             fn turbofish() { unknown_argument::<Vec<u8>>(\"delta\", \"x\"); }\n\
             fn absolute() {\n\
                 unknown_argument(::nest_rs_codegen::JobDecorator::After.name(), \"x\");\n\
             }\n\
             fn computed(key: &str) { unknown_argument(&format!(\"#{key}\"), \"x\"); }\n\
             macro_rules! refuse {\n\
                 ($attr:expr, $name:expr) => { nest_rs_codegen::unknown_argument($attr, $name) };\n\
             }\n\
             fn through_a_macro(name: &str) { refuse!(\"zeta\", name); }",
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

    let crates = |names: &[&str]| -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    };
    let (alpha, beta) = ("nest-rs-alpha-macros", "nest-rs-beta-macros");
    let members = BTreeMap::from([
        // A literal, at the sentence and at `unmatched_meta`.
        ("alpha".to_owned(), crates(&[alpha])),
        ("beta".to_owned(), crates(&[alpha])),
        // A literal given to a function that hands it on.
        ("gamma".to_owned(), crates(&[alpha])),
        // A turbofish does not hide a call.
        ("delta".to_owned(), crates(&[alpha])),
        // Nor a local `macro_rules!` handing its metavariable on, which used to
        // drop its decorator in silence. An import alias of a way is the
        // `blinds` join's to refuse, not a shape read here.
        ("zeta".to_owned(), crates(&[alpha])),
        // A table's variant names its member — through a constant, written
        // directly, and through an absolute path — and a value only data decides
        // names every member.
        ("process".to_owned(), crates(&[alpha, beta])),
        ("cron".to_owned(), crates(&[alpha, beta])),
        ("after".to_owned(), crates(&[alpha, beta])),
        ("every".to_owned(), crates(&[beta])),
    ]);

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

/// [`key_sets`] and [`guarded`], on a planted tree: one decorator name worded
/// as two grammars in one crate — the shape `#[expose]` has — is two key sets,
/// and the guard taking one grammar's keys says nothing of the other. The field
/// half here refuses its unknown key by hand and its repeats nowhere — the state
/// `#[expose]`'s `via` was left in when its own refusal was deleted, which the
/// cell read as green while *any* key of the decorator refused a repeat.
#[test]
fn a_grammar_refuses_a_repeat_only_when_its_keys_are_taken_through_the_guard() {
    const TREE: [(&str, &str); 2] = [
        (
            "crates/nest-rs-codegen/src/args.rs",
            "pub fn unknown_argument(attr: &str, name: &str, expected: &[&str]) -> String { \
                 format!(\"{attr}{name}{expected:?}\") }\n\
             pub fn duplicate_argument(attr: &str, name: &str) -> String { format!(\"{attr}{name}\") }\n\
             pub struct WrittenKeys(Vec<String>);\n\
             impl WrittenKeys {\n\
                 pub fn take_key(&mut self, attr: &str, keys: &[&str], at: &str, name: &str) {\n\
                     if !keys.contains(&name) { unknown_argument(attr, name, keys); }\n\
                     if self.0.contains(name) { duplicate_argument(attr, name); }\n\
                 }\n\
             }\n\
             pub fn job_key(member: JobDecorator, written: &mut WrittenKeys, name: &str) {\n\
                 let column = columns(member);\n\
                 written.take_key(member.name(), &column, name, name);\n\
             }",
        ),
        (
            "crates/nest-rs-omega-macros/src/lib.rs",
            "const FIELD_KEYS: [&str; 2] = [\"via\", \"complexity\"];\n\
             fn on_the_struct(written: &mut WrittenKeys, name: &str) {\n\
                 written.take_key(\"omega\", &[\"name\", \"service\"], name, name);\n\
             }\n\
             fn on_a_field(name: &str) { unknown_argument(\"omega\", name, &FIELD_KEYS); }\n\
             fn a_job(written: &mut WrittenKeys, name: &str) {\n\
                 job_key(JobDecorator::Every, written, name);\n\
             }",
        ),
    ];
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("grammars-key-sets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crate::plant(&root, &TREE);
    let sources = Sources::collect(&root);
    let _ = std::fs::remove_dir_all(&root);

    let omega = ("omega".to_owned(), "nest-rs-omega-macros".to_owned());
    let sets: Vec<Vec<String>> = key_sets(&sources)
        .remove(&omega)
        .map(|sets| sets.into_iter().collect())
        .unwrap_or_default();
    assert_eq!(
        sets,
        [
            vec!["complexity".to_owned(), "via".to_owned()],
            vec!["name".to_owned(), "service".to_owned()],
        ],
    );
    let guards: Vec<String> = guards(&sources).into_keys().map(|(_, name)| name).collect();
    assert_eq!(guards, ["job_key", "take_key"]);
    let mut guarded = guarded(&sources);
    assert_eq!(
        guarded.remove(&omega),
        Some(BTreeSet::from([Some(vec![
            "name".to_owned(),
            "service".to_owned()
        ])])),
        "the struct half's keys are taken through the guard; the field half's are not",
    );
    // A table guard takes the member it is handed through that member's column,
    // which is the codegen's — never a key set the parser spells.
    let column = table_key_sets()
        .into_iter()
        .find_map(|(member, keys)| (member == "every").then_some(keys));
    assert_eq!(
        guarded.remove(&("every".to_owned(), "nest-rs-omega-macros".to_owned())),
        Some(BTreeSet::from([column])),
    );
}
