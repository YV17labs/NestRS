//! `#[resilient(field, …)]`: an `async fn` method's body run under the policy a
//! field of `self` holds — once or again, each answer counted as the method's
//! predicates say, its attempt's token bound under the name the method gives
//! it, and the error its run ends on answered by the fallback it names.

use nest_rs_codegen::__private::{
    HostBorrow, RESILIENT, RETRIES, TIMEOUT, answer_span, cfg_attrs, is_resilient,
    mixed_site_ident, needs_a_value, resilient_policy_setting, resilient_retries,
    resilient_timeout, shared_receiver, site, takes_value, ungrouped_expr,
};
use nest_rs_codegen::{Arg, Grammar, Refusals};
use proc_macro2::{Delimiter, Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::discouraged::Speculative;
use syn::parse::{ParseStream, Parser};
use syn::spanned::Spanned;
use syn::{
    Expr, ExprLit, FnArg, GenericArgument, Ident, ImplItemFn, Item, Lit, Member, Pat, PatIdent,
    PatType, PathArguments, ReturnType, Stmt, Token, TraitItemFn, Type, TypeParamBound,
};

use crate::instrumented::{Part, instrumented};

/// The key declaring the call replayable, read off `run_idempotent`.
const IDEMPOTENT: &str = "idempotent";

/// The keys saying how an attempt's answer counts: sent again, an `Err` or an
/// `Ok`, and a failure in the circuit breaker.
const RETRY_IF: &str = "retry_if";
const RETRY_OK_IF: &str = "retry_ok_if";
const RECORD_IF: &str = "record_if";

/// The key naming the method answering in place of the run's error, and the
/// one saying which errors it answers for.
const FALLBACK: &str = "fallback";
const FALLBACK_IF: &str = "fallback_if";

/// The key naming the attempt's `CancellationToken` in the body.
const CANCEL: &str = "cancel";

const KEYS: [&str; 9] = [
    IDEMPOTENT,
    TIMEOUT,
    RETRIES,
    RETRY_IF,
    RETRY_OK_IF,
    RECORD_IF,
    FALLBACK,
    FALLBACK_IF,
    CANCEL,
];

const GRAMMAR: Grammar = Grammar::new(RESILIENT, &KEYS)
    .bare(bare)
    .elsewhere(resilient_policy_setting);

/// A key that says nothing without another.
struct Needs {
    key: &'static str,
    needs: &'static str,
}

impl Needs {
    /// Why the key says nothing alone, and the two ways out.
    fn why(&self) -> String {
        let Self { key, needs } = self;
        match *needs {
            FALLBACK => format!(
                "it says which errors the fallback answers in place of, and this method names \
                 none — name the method answering, `fallback = stale`, or drop `{key}`"
            ),
            _ => format!(
                "a call run once is never sent again — declare the call `{needs}` where sending \
                 it twice leaves the dependency as sending it once does, or drop `{key}`"
            ),
        }
    }
}

const NEEDS: [Needs; 4] = [
    Needs {
        key: RETRIES,
        needs: IDEMPOTENT,
    },
    Needs {
        key: RETRY_IF,
        needs: IDEMPOTENT,
    },
    Needs {
        key: RETRY_OK_IF,
        needs: IDEMPOTENT,
    },
    Needs {
        key: FALLBACK_IF,
        needs: FALLBACK,
    },
];

/// What `#[resilient(…)]` was given.
struct Args {
    field: Ident,
    /// Every key of the decorator's the list wrote, whatever its value: the
    /// replayed closure is spanned at `idempotent`, so what rustc says of it
    /// points at the declaration.
    keys: Vec<Ident>,
    timeout_millis: Option<u64>,
    retries: Option<u32>,
    retry_if: Option<Expr>,
    retry_ok_if: Option<Expr>,
    record_if: Option<Expr>,
    /// The method `fallback` names, when it names one.
    fallback: Option<Ident>,
    fallback_if: Option<Expr>,
    /// The name `cancel` binds the attempt's token under, when it gives one.
    cancel: Option<Ident>,
}

impl Args {
    /// `key` as the list wrote it, when it wrote one.
    fn key(&self, key: &str) -> Option<&Ident> {
        self.keys.iter().find(|written| written.unraw() == key)
    }

    /// Refuse each key written without the one it needs.
    fn refuse_unmet_needs(&self, refusals: &mut Refusals) {
        for need in &NEEDS {
            if let Some(key) = self.key(need.key)
                && self.key(need.needs).is_none()
            {
                refusals.push(syn::Error::new(
                    key.span(),
                    format!(
                        "{} needs `{}`: {}",
                        site(RESILIENT, Some(need.key)),
                        need.needs,
                        need.why(),
                    ),
                ));
            }
        }
    }

    /// The run the list declares.
    fn plan(&self) -> Plan<'_> {
        let fallback = self.key(FALLBACK).zip(self.fallback.as_ref());
        let judged = self.record_if.as_ref().and(self.key(RECORD_IF));
        Plan {
            idempotent: self.key(IDEMPOTENT),
            lends: self
                .key(IDEMPOTENT)
                .or(fallback.map(|(key, _)| key))
                .or(judged),
            timeout_millis: self.timeout_millis,
            retries: self.retries,
            retry_if: self.retry_if.as_ref(),
            retry_ok_if: self.retry_ok_if.as_ref(),
            record_if: self.record_if.as_ref(),
            fallback: fallback.map(|(key, method)| Fallback {
                key,
                method,
                when: self.fallback_if.as_ref(),
            }),
            reads: false,
            ..self.once()
        }
    }

    /// The run a refused method is kept as: once, its one attempt owning the
    /// arguments, under the field and the token's name alone, each argument
    /// read first in the place of the keys left out.
    fn once(&self) -> Plan<'_> {
        Plan {
            field: &self.field,
            reads: true,
            idempotent: None,
            lends: None,
            timeout_millis: None,
            retries: None,
            retry_if: None,
            retry_ok_if: None,
            record_if: None,
            fallback: None,
            cancel: self.cancel.as_ref(),
        }
    }
}

/// How a method's body runs under its policy.
struct Plan<'a> {
    field: &'a Ident,
    /// The method is one refused, kept without the keys that read its
    /// arguments: each is read ahead of the run, where rustc would report one
    /// only a predicate or the fallback read as unused, its fix breaking the key.
    reads: bool,
    /// `idempotent` as written, when the call is sent again: kept for its span.
    idempotent: Option<&'a Ident>,
    /// The key that has each attempt borrow the arguments rather than own
    /// them, as written: `idempotent`, whose next attempt reads them again,
    /// `fallback`, handed them next, or `record_if`, which may read them once
    /// the one attempt answered.
    lends: Option<&'a Ident>,
    timeout_millis: Option<u64>,
    retries: Option<u32>,
    retry_if: Option<&'a Expr>,
    retry_ok_if: Option<&'a Expr>,
    record_if: Option<&'a Expr>,
    fallback: Option<Fallback<'a>>,
    cancel: Option<&'a Ident>,
}

/// The method answering in place of a run's error.
struct Fallback<'a> {
    /// `fallback` as written, kept for its span.
    key: &'a Ident,
    method: &'a Ident,
    /// What `fallback_if` was given.
    when: Option<&'a Expr>,
}

/// What a method is, as far as an expansion of it is concerned.
struct Shape {
    asynchronous: bool,
    /// Its receiver's `self`, when it lends the shared borrow the policy's
    /// field is read through.
    receiver: Option<Token![self]>,
    /// Its return type holds an `impl Trait`, which no expansion can name.
    opaque: bool,
    /// Its body is a `#[tracing::instrument]` expansion in a shape the
    /// decorator does not read.
    unread: bool,
    /// Its list writes `cancel` and the decorator read no name under it, or one
    /// an argument already has or may have, bound by a pattern no attribute
    /// reads, so the body may read a token nothing binds.
    unbound: bool,
}

/// The method `item` is, its body run under the policy its field holds — or,
/// refused, the item each refusal reports beside.
pub(crate) fn resilient(args: TokenStream, item: TokenStream) -> TokenStream {
    match expansion(args, item) {
        Ok(expanded) => expanded,
        Err(Refused { kept, refusal }) => {
            let mut out = kept;
            out.extend(refusal.to_compile_error());
            out
        }
    }
}

/// A decorated item the attribute refused, and every refusal it earned. The
/// item is kept in a form that compiles whatever its body, so each refusal is
/// the one error its misuse reports.
struct Refused {
    kept: TokenStream,
    refusal: syn::Error,
}

fn expansion(args: TokenStream, item: TokenStream) -> Result<TokenStream, Refused> {
    let mut refusals = Refusals::default();
    let cancels = writes(&args, CANCEL);
    let args = parse_args(args, &mut refusals);
    let unbound = cancels && args.as_ref().is_none_or(|args| args.cancel.is_none());
    let Ok(mut method) = syn::parse2::<ImplItemFn>(item.clone()) else {
        refusals.push(no_method(&item));
        return Err(refused(item, refusals));
    };
    let mut run_already = false;
    let read = instrumented(&mut method.block.stmts, &mut |part| {
        if let Part::Written(stmts) = part {
            run_already = opens_on_run_mark(stmts);
        }
    });
    if run_already {
        return Err(Refused {
            kept: item,
            refusal: syn::Error::new(Span::call_site(), TWICE),
        });
    }
    // A `fn` an instrument wrapped is read once it is an `async fn`: its refusal
    // says to write one, and the body it is kept without is typed as nothing.
    let unread = match method.sig.asyncness {
        Some(_) => refusals.handle(read).is_none(),
        None => read.is_err(),
    };
    let checked = check(&method, args.as_ref(), &mut refusals);
    let shape = Shape {
        unread,
        unbound: unbound || checked.unbound,
        ..checked
    };
    match (refusals.finish(), args, shape.receiver) {
        (Ok(()), Some(args), Some(receiver)) => Ok(expand(method, receiver, &args.plan())),
        (Err(refusal), args, _) => {
            method.attrs.retain(|attr| !is_resilient(attr));
            Err(Refused {
                kept: kept(method, args.as_ref(), &shape),
                refusal,
            })
        }
        (Ok(()), _, _) => Err(refused(item, Refusals::default())),
    }
}

/// `item` kept as written beside every refusal in `refusals`.
fn refused(item: TokenStream, refusals: Refusals) -> Refused {
    let refusal = match refusals.finish() {
        Err(refusal) => refusal,
        Ok(()) => syn::Error::new(
            Span::call_site(),
            "#[resilient] refused this item without saying why — a defect in the framework, not \
             in this code",
        ),
    };
    Refused {
        kept: item,
        refusal,
    }
}

/// The refusal of an item that is no method with a body: a declaration ending
/// on `;`, as a trait writes one, is told what it is at its name, and anything
/// else at the attribute.
fn no_method(item: &TokenStream) -> syn::Error {
    match syn::parse2::<TraitItemFn>(item.clone()) {
        Ok(declared) if declared.default.is_none() => syn::Error::new(
            declared.sig.ident.span(),
            format!(
                "#[resilient] decorates an `async fn` method with a body, run under the policy a \
                 field of `self` holds, and `{}` is declared with none, as a trait declares a \
                 method: there is no body to run, and no field of `self` to read — put \
                 `#[resilient]` on the method of each impl",
                declared.sig.ident,
            ),
        ),
        _ => syn::Error::new(
            Span::call_site(),
            "#[resilient] decorates an `async fn` method with a body, run under the policy a \
             field of `self` holds, and this is no method — put it on the method making the \
             call",
        ),
    }
}

/// What a second `#[resilient]` on one method is told.
const TWICE: &str = "#[resilient] is written twice here — a method runs under one policy, and a \
                     second would run the first's whole run again at each of its attempts. Keep \
                     one; a method calling two dependencies is two methods, each decorated";

/// The item the body of a method `#[resilient]` read opens on, expanded or
/// kept, in the macro ABI's prefix: no developer writes it, so a `#[resilient]`
/// finding it was written below another its name did not show — one imported
/// under another name, or an entry of a `#[cfg_attr]`.
const RUN: &str = "__nestrs_resilient_run";

/// The mark a body run under a policy opens on: an item rustc reports as no
/// dead code, its name leading with `_`, so it carries no lint level a crate's
/// `forbid` could refuse, every token the expansion's own, so no lint reads
/// it as a function the developer wrote.
fn run_mark() -> Stmt {
    let mark = mixed_site_ident(RUN);
    Stmt::Item(Item::Verbatim(
        quote_spanned!(Span::mixed_site()=> fn #mark() {}),
    ))
}

/// Whether `stmts`, a method's body, opens on [`run_mark`].
fn opens_on_run_mark(stmts: &[Stmt]) -> bool {
    matches!(stmts.first(), Some(Stmt::Item(Item::Fn(mark))) if mark.sig.ident == RUN)
}

/// A refused method, in the nearest form its body compiles in as written —
/// as if it answered `Result<T, E>` — so that no error but its refusals is
/// reported:
///
/// - with no body where the one it has is an instrument's the decorator cannot
///   type as an attempt, or may read a token whose name the decorator did not
///   bind;
/// - where its answer holds an `impl Trait`, whatever else refused it: checked
///   against the stand-in as an attempt of that answer, `_` in the place of
///   what the body alone names, or as written where the answer is an
///   `impl Trait` whole, which says nothing of its attempt;
/// - as the `async fn` it was meant as where it is a `fn` whose body awaits at
///   its own level, then as any other; with no body where it is another `fn`,
///   a boxed future's as `#[async_trait]` leaves one, which answers the
///   method's own type where the developer wrote an attempt's;
/// - run once where the method could be expanded at all;
/// - and checked against a stand-in that never runs where its receiver or its
///   field is what was refused.
///
/// Every form but the bodiless one binds the token under the name `cancel`
/// gave; every form reads each name its parameters bind, the keys that read
/// them being left out, and opens on the expansion's mark, so a second
/// decorator below, read by no name, is refused beside the first.
fn kept(mut method: ImplItemFn, args: Option<&Args>, shape: &Shape) -> TokenStream {
    if !shape.opaque && (shape.unread || shape.unbound) {
        no_body(&mut method);
        return method.into_token_stream();
    }
    if shape.opaque {
        let token = args
            .and_then(|args| args.cancel.as_ref())
            .filter(|_| !shape.unbound);
        if let Some(inferred) = inferred_answer(&method).filter(|_| !shape.unread) {
            return stand_in(method, token, inferred);
        }
        let read = reads(&method, Read::BesideItsBody);
        let written = std::mem::take(&mut method.block.stmts);
        let mut stmts = vec![run_mark()];
        stmts.extend(read);
        if let Some(token) = token {
            stmts.push(
                syn::parse_quote!(let #token = ::nest_rs_resilience::CancellationToken::new();),
            );
        }
        stmts.push(as_written(method.block.brace_token, written));
        method.block.stmts = stmts;
        return method.into_token_stream();
    }
    if !shape.asynchronous {
        let written = method.block.stmts.iter().map(ToTokens::to_token_stream);
        if method.sig.constness.is_some() || !awaits(written.collect()) {
            no_body(&mut method);
            return method.into_token_stream();
        }
        method.sig.asyncness = Some(Token![async](method.sig.fn_token.span));
    }
    match (args, shape.receiver) {
        (Some(args), Some(receiver)) => expand(method, receiver, &args.once()),
        _ => {
            let answer = answer(&method);
            stand_in(method, args.and_then(|args| args.cancel.as_ref()), answer)
        }
    }
}

/// Whether `tokens`, a `fn`'s body, hold an `.await` of their own, which only
/// an `async fn`'s does: outside every `async` block, every item's body and
/// every macro's arguments, whose expansion may hold a block of its own. Read
/// by tokens: what is taken for one of those wrongly — the brace after a `fn`
/// pointer's type, a parenthesised operand of `!` — hides an `.await`, and the
/// method is kept with no body, which is right of every `fn`.
fn awaits(tokens: TokenStream) -> bool {
    let (mut after_dot, mut after_bang, mut in_another) = (false, false, false);
    for tree in tokens {
        match &tree {
            TokenTree::Ident(word) if after_dot && word == "await" => return true,
            TokenTree::Ident(word) if word == "async" || word == "fn" => in_another = true,
            TokenTree::Group(_) if after_bang => {}
            TokenTree::Group(group) if in_another && group.delimiter() == Delimiter::Brace => {
                in_another = false;
            }
            TokenTree::Group(group) if awaits(group.stream()) => return true,
            _ => {}
        }
        after_dot = matches!(&tree, TokenTree::Punct(dot) if dot.as_char() == '.');
        after_bang = matches!(&tree, TokenTree::Punct(bang) if bang.as_char() == '!');
    }
    false
}

/// How a kept method reads the names its parameters bind.
#[derive(Clone, Copy, PartialEq)]
enum Read {
    /// Beside the body written, which changes what it binds `mut`.
    BesideItsBody,
    /// In the place of a body left out: a `mut` binding is read as one, or it
    /// would be one nothing changes.
    InPlaceOfItsBody,
}

/// The statement reading each name the parameters of `method` bind, under its
/// parameter's `#[cfg]`s — `None` where they bind none an attribute reads. No
/// lint attribute stands in for it: one is `E0453` in a crate forbidding the
/// lint.
fn reads(method: &ImplItemFn, read: Read) -> Option<Stmt> {
    let mut names = Vec::new();
    for typed in typed_inputs(method) {
        let cfgs = cfg_attrs(&typed.attrs);
        let mut bound = Bound::default();
        bound.read(&typed.pat);
        names.extend(bound.names.iter().map(|binding| {
            let name = &binding.ident;
            match (&binding.by_ref, &binding.mutability) {
                (None, Some(_)) if read == Read::InPlaceOfItsBody => quote!(#(#cfgs)* &mut #name),
                _ => quote!(#(#cfgs)* &#name),
            }
        }));
    }
    (!names.is_empty()).then(|| syn::parse_quote!(let _ = (#(#names,)*);))
}

/// Leaves `method` no body: the expansion's mark, each name its parameters
/// bind, read as it is bound, then an answer never given. A parameter binding
/// names no attribute reads is taken under none, `_`.
fn no_body(method: &mut ImplItemFn) {
    for input in &mut method.sig.inputs {
        let FnArg::Typed(typed) = input else {
            continue;
        };
        let mut bound = Bound::default();
        bound.read(&typed.pat);
        if !bound.unparsed.is_empty() {
            *typed.pat = Pat::Wild(syn::PatWild {
                attrs: Vec::new(),
                underscore_token: Token![_](typed.pat.span()),
            });
        }
    }
    let read = reads(method, Read::InPlaceOfItsBody);
    method.block.stmts = run_under(read, quote!(::core::unreachable!()));
}

/// The arguments `method` takes beside its receiver.
fn typed_inputs(method: &ImplItemFn) -> impl Iterator<Item = &PatType> {
    method.sig.inputs.iter().filter_map(|input| match input {
        FnArg::Typed(typed) => Some(typed),
        FnArg::Receiver(_) => None,
    })
}

/// A name `cancel` takes, offered to a method whose token was refused the one
/// it wrote: the example every other sentence of the key gives, first.
const TOKEN_NAMES: [&str; 3] = ["token", "cancellation", "stop"];

/// What patterns bind, as far as an attribute reads them.
#[derive(Default)]
struct Bound<'a> {
    /// Every name bound, in order, wherever a pattern binds it.
    names: Vec<&'a PatIdent>,
    /// Each pattern whose names no attribute reads: a macro's, expanded after
    /// every attribute on its method, and one `syn` keeps as tokens.
    unparsed: Vec<&'a Pat>,
}

impl<'a> Bound<'a> {
    /// What the arguments of `method` bind, beside its receiver.
    fn by(method: &'a ImplItemFn) -> Self {
        let mut bound = Self::default();
        for typed in typed_inputs(method) {
            bound.read(&typed.pat);
        }
        bound
    }

    /// Adds what `pat` binds, wherever it binds it.
    fn read(&mut self, pat: &'a Pat) {
        match pat {
            Pat::Ident(binding) => {
                self.names.push(binding);
                if let Some((_, sub)) = &binding.subpat {
                    self.read(sub);
                }
            }
            Pat::Tuple(tuple) => tuple.elems.iter().for_each(|elem| self.read(elem)),
            Pat::TupleStruct(tuple) => tuple.elems.iter().for_each(|elem| self.read(elem)),
            Pat::Struct(fields) => fields.fields.iter().for_each(|field| self.read(&field.pat)),
            Pat::Slice(slice) => slice.elems.iter().for_each(|elem| self.read(elem)),
            Pat::Reference(reference) => self.read(&reference.pat),
            Pat::Paren(paren) => self.read(&paren.pat),
            Pat::Type(typed) => self.read(&typed.pat),
            // Every case of an or-pattern binds the same names.
            Pat::Or(or) => or.cases.iter().take(1).for_each(|case| self.read(case)),
            Pat::Macro(_) | Pat::Verbatim(_) => self.unparsed.push(pat),
            // A constant, a literal, a path, a range, `..` and `_` bind no name.
            _ => {}
        }
    }

    /// Whether a name read is `name`, written raw or not.
    fn binds(&self, name: &str) -> bool {
        self.names
            .iter()
            .any(|binding| binding.ident.unraw() == name)
    }
}

/// Whether `args` writes `key` outside every group, read or not: what a list
/// refused before its end may still hold.
fn writes(args: &TokenStream, key: &str) -> bool {
    args.clone()
        .into_iter()
        .any(|tree| matches!(tree, TokenTree::Ident(written) if written.unraw() == key))
}

/// The arguments: the field and every key written after it, each refusal kept
/// in `refusals`; `None` when the field itself is what was refused.
///
/// A refused key does not end the reading, which [`resumes`] after it, so what
/// one key says of another — the one it needs, `retries`' value beside
/// `idempotent` — is said of the whole list. Where the reading cannot resume,
/// a key that may stand in what is left is unread, never absent.
fn parse_args(args: TokenStream, refusals: &mut Refusals) -> Option<Args> {
    let parsed = (|input: ParseStream<'_>| {
        if input.is_empty() {
            return Err(syn::Error::new(
                Span::call_site(),
                "#[resilient] names the field of `self` holding the policy — write \
                 `#[resilient(carrier)]` to run the method under `self.carrier`, an \
                 `Arc<ResiliencePolicy<I>>` injected with `#[inject]`",
            ));
        }
        let field = field(input)?;
        let mut args = Args {
            field,
            keys: Vec::new(),
            timeout_millis: None,
            retries: None,
            retry_if: None,
            retry_ok_if: None,
            record_if: None,
            fallback: None,
            fallback_if: None,
            cancel: None,
        };
        if input.is_empty() {
            return Ok(args);
        }
        input.parse::<Token![,]>()?;
        let mut retries = None;
        let mut whole = true;
        while let Err(refusal) = GRAMMAR.parse(input, |arg| {
            // The grammar knows a key written twice within one reading alone.
            GRAMMAR.take_all(args.keys.iter().chain([arg.ident()]))?;
            args.keys.push(arg.ident().clone());
            match arg.key() {
                IDEMPOTENT => {
                    let rest = arg.input();
                    if rest.peek(Token![=]) || rest.peek(syn::token::Paren) {
                        return Err(syn::Error::new(
                            arg.ident().span(),
                            format!(
                                "{} takes no value — it is a flag, and what it declares is its \
                                 presence: the call is sent again after an `Err` or a timeout",
                                site(RESILIENT, Some(IDEMPOTENT)),
                            ),
                        ));
                    }
                }
                TIMEOUT => args.timeout_millis = refusals.handle(resilient_timeout(&value(&arg)?)),
                RETRIES => retries = Some(value(&arg)?),
                RETRY_IF => args.retry_if = predicate(&arg, refusals)?,
                RETRY_OK_IF => args.retry_ok_if = predicate(&arg, refusals)?,
                RECORD_IF => args.record_if = predicate(&arg, refusals)?,
                FALLBACK => args.fallback = refusals.handle(fallback(&value(&arg)?)),
                FALLBACK_IF => args.fallback_if = predicate(&arg, refusals)?,
                CANCEL => {
                    args.cancel = refusals.handle(name(
                        &value(&arg)?,
                        CANCEL,
                        "the name the body reads its attempt's `CancellationToken` under, e.g. \
                         `cancel = token`",
                    ));
                }
                unread => {
                    return Err(syn::Error::new_spanned(
                        arg.ident(),
                        format!(
                            "{}: the grammar gives this decorator the key and its parser does \
                             not read it — a defect in the framework, not in this code",
                            site(RESILIENT, Some(unread)),
                        ),
                    ));
                }
            }
            Ok(())
        }) {
            refusals.push(refusal);
            if !resumes(input) {
                whole = false;
                // A parser leaving tokens unread is refused in `syn`'s words.
                input.step(|cursor| {
                    let mut rest = *cursor;
                    while let Some((_, next)) = rest.token_tree() {
                        rest = next;
                    }
                    Ok(((), rest))
                })?;
            }
        }
        if whole {
            args.refuse_unmet_needs(refusals);
        }
        if let Some(value) = retries
            && args.key(IDEMPOTENT).is_some()
        {
            args.retries = refusals.handle(resilient_retries(&value));
        }
        Ok(args)
    })
    .parse2(args);
    refusals.handle(parsed)
}

/// `arg`'s value, read ahead of the stream, which stays before a value that is
/// no expression: a parse that fails leaves its stream wherever it gave up,
/// and [`resumes`] reads on from there. Nothing after the `=` is the key
/// written bare, and anything else that reads as no expression is refused at
/// the token `syn` gave up on, in the key's words, never `syn`'s.
fn value(arg: &Arg<'_>) -> syn::Result<Expr> {
    let input = arg.value()?;
    if input.is_empty() || input.peek(Token![,]) {
        return Err(syn::Error::new(
            arg.ident().span(),
            bare(RESILIENT, arg.key()),
        ));
    }
    let ahead = input.fork();
    let value = ahead.parse().map_err(|unread: syn::Error| {
        syn::Error::new(
            unread.span(),
            takes_value(
                RESILIENT,
                Some(arg.key()),
                "an expression after its `=`, and this reads as none",
            ),
        )
    })?;
    input.advance_to(&ahead);
    Ok(value)
}

/// A key taking a predicate: what its value is a function over, and one a
/// developer could write.
struct Predicate {
    key: &'static str,
    over: &'static str,
    example: &'static str,
}

const PREDICATES: [Predicate; 4] = [
    Predicate {
        key: RETRY_IF,
        over: "`Fn(&E) -> bool` over the method's error",
        example: "|error| error.is_transient()",
    },
    Predicate {
        key: RETRY_OK_IF,
        over: "`Fn(&T) -> bool` over what the method answers",
        example: "|answer| answer.is_pending()",
    },
    Predicate {
        key: RECORD_IF,
        over: "`Fn(&E) -> bool` over the method's error",
        example: "|error| !error.is_declined()",
    },
    Predicate {
        key: FALLBACK_IF,
        over: "`Fn(&ResilienceError<E>) -> bool` over the error the run ended on",
        example: "|error| matches!(error, ResilienceError::Open { .. })",
    },
];

/// `arg`'s value as the predicate its key takes: any expression but one whose
/// own tokens say it is no function over one value answering at once — a
/// literal, a closure taking no argument or several, an `async` closure —
/// refused here, in `refusals`, where rustc would follow the key's sentence
/// with one naming the expansion, count a closure's arguments at the
/// attribute, or ask an `async` closure's argument for a type. A value that
/// reads as no expression is the error handed back.
fn predicate(arg: &Arg<'_>, refusals: &mut Refusals) -> syn::Result<Option<Expr>> {
    let value = value(arg)?;
    let Some(row) = PREDICATES.iter().find(|row| row.key == arg.key()) else {
        return Ok(Some(value));
    };
    let written = unparenthesized(&value);
    let why = match written {
        Expr::Lit(_) => String::new(),
        Expr::Closure(closure) if closure.asyncness.is_some() => {
            " — an `async` closure answers a future, and a predicate answers `bool` at once".into()
        }
        Expr::Closure(closure) if closure.inputs.len() != 1 => {
            let takes = match closure.inputs.len() {
                0 => "no argument".to_owned(),
                several => format!("{several} arguments"),
            };
            format!(" — this closure takes {takes}, and a predicate takes the one value it judges")
        }
        _ => return Ok(Some(value)),
    };
    let what = format!(
        "a {}, a function's path or a closure such as `{}`{why}",
        row.over, row.example
    );
    refusals.push(syn::Error::new_spanned(
        written,
        takes_value(RESILIENT, Some(row.key), &what),
    ));
    Ok(None)
}

/// `value` without the parentheses and invisible groups around it.
fn unparenthesized(value: &Expr) -> &Expr {
    let mut written = value;
    loop {
        match written {
            Expr::Group(group) => written = &group.expr,
            Expr::Paren(paren) => written = &paren.expr,
            _ => return written,
        }
    }
}

/// `value` as a name written alone, or the refusal saying `key` takes `what`.
fn name(value: &Expr, key: &str, what: &str) -> syn::Result<Ident> {
    let value = ungrouped_expr(value);
    if let Expr::Path(path) = value
        && path.attrs.is_empty()
        && path.qself.is_none()
        && let Some(name) = path.path.get_ident()
        // `self` and `Self` are paths of one segment, and name nothing here.
        && syn::parse2::<Ident>(name.to_token_stream()).is_ok()
    {
        return Ok(name.clone());
    }
    Err(syn::Error::new_spanned(
        value,
        takes_value(RESILIENT, Some(key), what),
    ))
}

/// `value` as the name of the method answering in place of the run's error. A
/// name given type arguments, which rustc's help writes for a fallback whose
/// type parameter no argument fixes, and a call are each told why the key
/// takes neither.
fn fallback(value: &Expr) -> syn::Result<Ident> {
    const WHAT: &str = "the name of an `async fn` method of `Self`, alone, e.g. `fallback = stale`";
    let why = match ungrouped_expr(value) {
        Expr::Path(path)
            if path
                .path
                .segments
                .last()
                .is_some_and(|named| !named.arguments.is_none()) =>
        {
            " — it takes no type argument: a fallback is handed the method's arguments, then the \
             error, and answers what the method answers, so name one whose type parameters those \
             fix"
        }
        Expr::Call(_) | Expr::MethodCall(_) => {
            " — it is named, never called: the decorator calls it with the method's arguments, \
             then the error"
        }
        _ => "",
    };
    name(value, FALLBACK, &format!("{WHAT}{why}"))
}

/// Whether the list reads on past the key just refused, `input` then standing
/// at the next one: over what is written there as a key's value, `= expression`
/// or `(…)`, when a comma or the list's end follows it. A value that reads as
/// no expression, and anything else before that comma, ends the reading.
///
/// The next comma is no boundary by itself: `pick::<A, B>` holds one outside
/// every group, and a key read from `B` would be refused though no one wrote it.
fn resumes(input: ParseStream<'_>) -> bool {
    let ahead = input.fork();
    let value = if ahead.peek(Token![=]) {
        ahead
            .parse::<Token![=]>()
            .and_then(|_| ahead.parse::<Expr>())
            .map(drop)
    } else if ahead.peek(syn::token::Paren) {
        ahead.parse::<TokenTree>().map(drop)
    } else {
        Ok(())
    };
    if value.is_err() || !(ahead.is_empty() || ahead.parse::<Token![,]>().is_ok()) {
        return false;
    }
    input.advance_to(&ahead);
    true
}

/// The field: the list's first entry, a name written alone, whatever that name
/// is. One a key that takes a value has too (`fallback`) is no key written
/// right with none, so it is the field it reads as. A first entry is a key
/// ahead of its field, and refused, where the list says so: it is given a
/// value, or a name that is no key follows it, the field written late. Where
/// nothing tells the two apart, the refusal says how each is written:
/// `idempotent`, the one key written alone, unless the key follows it, and a
/// setting of the policy's, which no list takes.
fn field(input: ParseStream<'_>) -> syn::Result<Ident> {
    let fork = input.fork();
    let leading = fork.call(Ident::parse_any).ok();
    let alone = fork.is_empty() || fork.peek(Token![,]);
    if let Some(leading) = &leading {
        let name = leading.unraw().to_string();
        let rest = if alone {
            Rest::after(&fork)
        } else {
            Rest::default()
        };
        let ahead = fork.peek(Token![=]) || fork.peek(syn::token::Paren) || rest.field;
        let unsure = alone && !rest.field;
        if KEYS.contains(&name.as_str()) {
            let either = match (ahead, name == IDEMPOTENT && unsure && !rest.idempotent) {
                (true, _) => Some(String::new()),
                (false, true) => Some(format!(
                    "; a field itself named `{leading}` is named before the key, \
                     `#[resilient({leading}, {leading})]`, or renamed"
                )),
                (false, false) => None,
            };
            if let Some(either) = either {
                return Err(syn::Error::new(
                    leading.span(),
                    format!(
                        "#[resilient] reads the field holding the policy first, and `{leading}` \
                         is one of its keys — write the field before it: `#[resilient(carrier, \
                         {leading})]`{either}",
                    ),
                ));
            }
        } else if let Some(elsewhere) = resilient_policy_setting(&name) {
            let either = if unsure {
                format!("; a field itself named `{leading}` is renamed")
            } else {
                String::new()
            };
            return Err(syn::Error::new(
                leading.span(),
                format!("{elsewhere}{either}"),
            ));
        }
    }
    let fork = input.fork();
    match fork.parse::<Ident>() {
        Ok(_) if alone => input.parse(),
        Ok(field) if fork.peek(Ident::peek_any) || fork.peek(Lit) => Err(no_comma(&field, &fork)),
        _ => Err(no_field(input)),
    }
}

/// What a list writes after its first entry, as far as it reads as one.
#[derive(Default)]
struct Rest {
    /// A name written alone that is no key, nor a setting of the policy's:
    /// the field, written after a key.
    field: bool,
    /// `idempotent`, which written twice is the key once at most.
    idempotent: bool,
}

impl Rest {
    /// What follows `first`, a stream standing after the list's first entry.
    fn after(first: ParseStream<'_>) -> Self {
        let ahead = first.fork();
        let mut rest = Self::default();
        while ahead.parse::<Token![,]>().is_ok()
            && let Ok(written) = ahead.call(Ident::parse_any)
        {
            let name = written.unraw().to_string();
            let alone = ahead.is_empty() || ahead.peek(Token![,]);
            if name == IDEMPOTENT {
                rest.idempotent = true;
            } else if alone
                && !KEYS.contains(&name.as_str())
                && resilient_policy_setting(&name).is_none()
            {
                rest.field = true;
            }
            let value = if ahead.peek(Token![=]) {
                ahead
                    .parse::<Token![=]>()
                    .and_then(|_| ahead.parse::<Expr>())
                    .map(drop)
            } else if ahead.peek(syn::token::Paren) {
                ahead.parse::<TokenTree>().map(drop)
            } else {
                Ok(())
            };
            if value.is_err() {
                break;
            }
        }
        rest
    }
}

/// The refusal of a field what follows it is parted from by no comma, the
/// list shown with one keeping everything written `after` the field.
fn no_comma(field: &Ident, after: ParseStream<'_>) -> syn::Error {
    let written: TokenStream = after.fork().parse().unwrap_or_default();
    syn::Error::new(
        after.span(),
        format!(
            "#[resilient] reads the field of `self` holding the policy, then each key after a \
             comma, and the comma after `{field}` is missing — write `#[resilient({field}, \
             {written})]`"
        ),
    )
}

/// The refusal of what leads the list in a field's place: a call, which rustc
/// offers a host answering its policy through a method, and a tuple struct's
/// position are each told what the decorator reads instead.
fn no_field(input: ParseStream<'_>) -> syn::Error {
    const NAMES: &str = "#[resilient] names the field of `self` holding the policy by its name, \
                         alone";
    let ahead = input.fork();
    let written = ahead
        .parse::<Expr>()
        .ok()
        .filter(|_| ahead.is_empty() || ahead.peek(Token![,]));
    let position = |index: &dyn ToTokens| {
        format!(
            "{NAMES}, and `{}` is a tuple struct's field, which has a position and no name — \
             hold the policy in a named field, `carrier: Arc<ResiliencePolicy<I>>`, and write \
             `#[resilient(carrier)]`",
            index.to_token_stream(),
        )
    };
    match written.as_ref().map(ungrouped_expr) {
        Some(called @ (Expr::Call(_) | Expr::MethodCall(_))) => syn::Error::new_spanned(
            called,
            format!(
                "{NAMES}, and this is a call: the policy is read from a field, never through a \
                 method — hold the `Arc<ResiliencePolicy<I>>` in a field of the host, and write \
                 that field's name, `#[resilient(carrier)]`"
            ),
        ),
        Some(
            literal @ Expr::Lit(ExprLit {
                lit: Lit::Int(index),
                ..
            }),
        ) => syn::Error::new_spanned(literal, position(index)),
        Some(Expr::Field(field)) if matches!(field.member, Member::Unnamed(_)) => {
            syn::Error::new_spanned(field, position(&field.member))
        }
        _ => syn::Error::new(
            input.span(),
            format!(
                "{NAMES} — write `#[resilient(carrier)]` to run the method under `self.carrier`"
            ),
        ),
    }
}

/// A key written bare, offered with a value it takes.
fn bare(attr: &str, key: &str) -> String {
    let value = match key {
        TIMEOUT => "\"2s\"",
        RETRIES => "3",
        FALLBACK => "stale",
        CANCEL => "token",
        _ => match PREDICATES.iter().find(|predicate| predicate.key == key) {
            Some(predicate) => predicate.example,
            None => return needs_a_value(attr, key),
        },
    };
    format!(
        "{} needs a value — write `{key} = {value}`",
        site(attr, Some(key))
    )
}

/// Every refusal of what `method` is, beside its arguments' — `args` `None`
/// when its field did not read — and the shape it has.
fn check(method: &ImplItemFn, args: Option<&Args>, refusals: &mut Refusals) -> Shape {
    if let Some(second) = method.attrs.iter().find(|attr| is_resilient(attr)) {
        refusals.push(syn::Error::new_spanned(second, TWICE));
    }
    let asynchronous = method.sig.asyncness.is_some();
    if !asynchronous {
        let fn_token = method.sig.fn_token;
        refusals.push(match &method.sig.constness {
            Some(constness) => syn::Error::new_spanned(
                quote!(#constness #fn_token),
                "#[resilient] decorates an `async fn`: the policy's run is awaited, and this \
                 method is a `const fn`, which is never `async` — write `async fn`, without \
                 `const`",
            ),
            None => syn::Error::new_spanned(
                fn_token,
                "#[resilient] decorates an `async fn`: the policy's run is awaited, and this \
                 method is a `fn` — write `async fn`. Under `#[async_trait]`, which turns each \
                 `async fn` of the impl into a `fn` returning a boxed future before this \
                 attribute reads it, write the trait's method as a native `async fn` instead",
            ),
        });
    }
    let lent = shared_receiver(
        method,
        "#[resilient]",
        &Ident::new("Self", Span::call_site()),
        HostBorrow::Arc,
    )
    .map_err(|refusal| match method.sig.receiver() {
        Some(_) => refusal,
        None => hostless(method, &refusal),
    });
    let receiver = refusals
        .handle(lent)
        .and_then(|()| method.sig.receiver())
        .map(|receiver| receiver.self_token);
    let opaque = match &method.sig.output {
        ReturnType::Type(_, answered) => impl_trait(answered),
        ReturnType::Default => None,
    };
    if let Some(opaque) = opaque {
        refusals.push(syn::Error::new_spanned(
            opaque,
            "#[resilient] writes the method's return type into its expansion, where `impl Trait` \
             names no type — name the type the method answers, or box it as a `Box<dyn …>`",
        ));
    }
    let fallback = args.and_then(|args| args.key(FALLBACK));
    let itself = method.sig.ident.unraw();
    if let Some(key) = fallback
        && args.is_some_and(|args| {
            args.fallback
                .as_ref()
                .is_some_and(|named| named.unraw() == itself)
        })
    {
        refusals.push(syn::Error::new(
            key.span(),
            format!(
                "{} names the method answering in place of this one's error, and `{}` is this \
                 method itself, which would run under its policy again — name another method",
                site(RESILIENT, Some(FALLBACK)),
                method.sig.ident,
            ),
        ));
    }
    let lent = if args.is_some_and(|args| args.key(IDEMPOTENT).is_some()) {
        Some(Lent::AGAIN)
    } else {
        fallback.map(|_| Lent::TO_THE_FALLBACK)
    };
    let bound = Bound::by(method);
    let token = args.and_then(|args| args.cancel.as_ref());
    let hidden = token.filter(|token| bound.binds(&token.unraw().to_string()));
    if let Some(token) = hidden {
        let example = TOKEN_NAMES
            .iter()
            .find(|name| !bound.binds(name))
            .map(|name| format!(", e.g. `cancel = {name}`"))
            .unwrap_or_default();
        refusals.push(syn::Error::new(
            token.span(),
            format!(
                "{} names the attempt's `CancellationToken` in the body, and `{token}` is an \
                 argument of this method, which the body would read no more under that name — \
                 name the token otherwise{example}",
                site(RESILIENT, Some(CANCEL)),
            ),
        ));
    }
    let unseen = token.map_or(&[][..], |_| bound.unparsed.as_slice());
    // Under `fallback`, such a pattern is refused below as binding no plain name: one fix.
    for pat in unseen.iter().filter(|_| fallback.is_none()) {
        refusals.push(syn::Error::new_spanned(
            pat,
            format!(
                "{} names the attempt's `CancellationToken` in the body, where an argument of \
                 that name would be read no more, and `{}` binds names this attribute cannot \
                 read — name the argument, and take it apart inside the body",
                site(RESILIENT, Some(CANCEL)),
                written(pat),
            ),
        ));
    }
    for typed in typed_inputs(method) {
        // What a fallback cannot hand on under a name is refused once, in the words
        // whose fix cures a `mut` it binds too: naming the argument.
        let unnamed = fallback.filter(|_| plain_name(&typed.pat).is_none());
        if let Some(lent) = &lent {
            if let Some((binding, written)) = mutable_binding(&typed.pat)
                && unnamed.is_none()
            {
                refusals.push(syn::Error::new(
                    binding,
                    format!(
                        "{} {} the arguments the method was called with, so none is bound `mut`: \
                         one attempt's change to `{written}` would reach {} — bind it without \
                         `mut`, and change a copy inside the body",
                        site(RESILIENT, Some(lent.key)),
                        lent.does,
                        lent.reached,
                    ),
                ));
            }
            if let Some(reference) = exclusive_reference(&typed.ty) {
                refusals.push(syn::Error::new_spanned(
                    reference,
                    format!(
                        "{} {} the arguments the method was called with, so none is a `&mut`: \
                         one attempt's change through `{}` would reach {} — take it by shared \
                         reference or by value, and change a copy inside the body",
                        site(RESILIENT, Some(lent.key)),
                        lent.does,
                        typed.pat.to_token_stream(),
                        lent.reached,
                    ),
                ));
            }
        }
        if unnamed.is_some() {
            let (binds, remedy) = match &*typed.pat {
                Pat::Ident(PatIdent {
                    by_ref: Some(_),
                    mutability,
                    ident,
                    subpat: None,
                    ..
                }) => {
                    let then = match mutability {
                        Some(_) => "change a copy",
                        None => "borrow it",
                    };
                    (
                        "by reference",
                        format!("bind it by value, `{ident}`, and {then} inside the body"),
                    )
                }
                _ => (
                    "to no plain name",
                    "name the argument, and take it apart inside the body".to_owned(),
                ),
            };
            refusals.push(syn::Error::new_spanned(
                &typed.pat,
                format!(
                    "{} hands its fallback each argument under the name the method binds it to, \
                     and `{}` binds this one {binds} — {remedy}",
                    site(RESILIENT, Some(FALLBACK)),
                    written(&typed.pat),
                ),
            ));
        }
    }
    Shape {
        asynchronous,
        receiver,
        opaque: opaque.is_some(),
        unread: false,
        unbound: hidden.is_some() || !unseen.is_empty(),
    }
}

/// `refusal`, a method's for taking no receiver, saying too what a free
/// function is told: its tokens are an associated function's, and a receiver
/// is no remedy it can follow.
fn hostless(method: &ImplItemFn, refusal: &syn::Error) -> syn::Error {
    let at: &dyn ToTokens = match method.sig.inputs.first() {
        Some(first) => first,
        None => &method.sig,
    };
    syn::Error::new_spanned(
        at,
        format!(
            "{refusal}. A free function has no host to hold a policy in a field: the method \
             making the call is decorated, a provider's or that of a struct built by hand"
        ),
    )
}

/// `pat` as a sentence quotes it: a macro's call as it is written, where its
/// tokens print a space after the `!`.
fn written(pat: &Pat) -> String {
    let Pat::Macro(call) = pat else {
        return pat.to_token_stream().to_string();
    };
    let path: String = call
        .mac
        .path
        .to_token_stream()
        .to_string()
        .split_whitespace()
        .collect();
    let (open, close) = match call.mac.delimiter {
        syn::MacroDelimiter::Paren(_) => ('(', ')'),
        syn::MacroDelimiter::Brace(_) => ('{', '}'),
        syn::MacroDelimiter::Bracket(_) => ('[', ']'),
    };
    format!("{path}!{open}{}{close}", call.mac.tokens)
}

/// Why no attempt may change an argument it borrows: the key reading the
/// arguments after an attempt, what that key does with them, and what an
/// attempt's change to one would reach. `record_if` has the one attempt lend
/// them too and refuses nothing, a predicate's tokens not saying which
/// argument it reads: rustc refuses the one its body consumes or changes.
struct Lent {
    key: &'static str,
    does: &'static str,
    reached: &'static str,
}

impl Lent {
    const AGAIN: Self = Self {
        key: IDEMPOTENT,
        does: "attempts the body again with",
        reached: "the next",
    };
    const TO_THE_FALLBACK: Self = Self {
        key: FALLBACK,
        does: "hands its fallback",
        reached: "the fallback",
    };
}

/// The name `pat` binds a whole argument to by value, when it is one —
/// `mut` or not, which [`mutable_binding`] answers for.
fn plain_name(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(bound) if bound.by_ref.is_none() && bound.subpat.is_none() => Some(&bound.ident),
        _ => None,
    }
}

/// The first `impl Trait` `ty` holds, wherever it sits in it.
fn impl_trait(ty: &Type) -> Option<&Type> {
    match ty {
        Type::ImplTrait(_) => Some(ty),
        Type::Path(path) => path
            .qself
            .as_ref()
            .and_then(|qself| impl_trait(&qself.ty))
            .or_else(|| {
                path.path
                    .segments
                    .iter()
                    .find_map(|segment| impl_trait_within(&segment.arguments))
            }),
        Type::Reference(reference) => impl_trait(&reference.elem),
        Type::Ptr(pointer) => impl_trait(&pointer.elem),
        Type::Slice(slice) => impl_trait(&slice.elem),
        Type::Array(array) => impl_trait(&array.elem),
        Type::Paren(paren) => impl_trait(&paren.elem),
        Type::Group(group) => impl_trait(&group.elem),
        Type::Tuple(tuple) => tuple.elems.iter().find_map(impl_trait),
        Type::TraitObject(object) => object.bounds.iter().find_map(|bound| match bound {
            TypeParamBound::Trait(bound) => bound
                .path
                .segments
                .iter()
                .find_map(|segment| impl_trait_within(&segment.arguments)),
            _ => None,
        }),
        _ => None,
    }
}

/// What a method holding an `impl Trait` inside its answer answers, `_` in the
/// place of each, and its span as [`answer`] gives it — `None` where the answer
/// is an `impl Trait` whole, or the method answers nothing.
fn inferred_answer(method: &ImplItemFn) -> Option<(TokenStream, Span)> {
    let ReturnType::Type(_, answered) = &method.sig.output else {
        return None;
    };
    let mut inferred = (**answered).clone();
    infer_impl_traits(&mut inferred);
    let mut whole = &inferred;
    while let Type::Paren(syn::TypeParen { elem, .. }) | Type::Group(syn::TypeGroup { elem, .. }) =
        whole
    {
        whole = elem;
    }
    if matches!(whole, Type::Infer(_)) {
        return None;
    }
    let (_, at) = answer(method);
    Some((respanned(inferred.into_token_stream(), at), at))
}

/// Writes `_` in the place of each `impl Trait` `ty` holds, wherever
/// [`impl_trait`] finds one.
fn infer_impl_traits(ty: &mut Type) {
    if let Type::ImplTrait(opaque) = ty {
        let underscore = Token![_](opaque.impl_token.span);
        *ty = syn::parse_quote!(#underscore);
        return;
    }
    match ty {
        Type::Path(path) => {
            if let Some(qself) = &mut path.qself {
                infer_impl_traits(&mut qself.ty);
            }
            for segment in &mut path.path.segments {
                infer_impl_traits_within(&mut segment.arguments);
            }
        }
        Type::Reference(reference) => infer_impl_traits(&mut reference.elem),
        Type::Ptr(pointer) => infer_impl_traits(&mut pointer.elem),
        Type::Slice(slice) => infer_impl_traits(&mut slice.elem),
        Type::Array(array) => infer_impl_traits(&mut array.elem),
        Type::Paren(paren) => infer_impl_traits(&mut paren.elem),
        Type::Group(group) => infer_impl_traits(&mut group.elem),
        Type::Tuple(tuple) => tuple.elems.iter_mut().for_each(infer_impl_traits),
        Type::TraitObject(object) => {
            for bound in &mut object.bounds {
                if let TypeParamBound::Trait(bound) = bound {
                    for segment in &mut bound.path.segments {
                        infer_impl_traits_within(&mut segment.arguments);
                    }
                }
            }
        }
        _ => {}
    }
}

/// [`infer_impl_traits`] over a path segment's arguments.
fn infer_impl_traits_within(arguments: &mut PathArguments) {
    match arguments {
        PathArguments::AngleBracketed(angled) => {
            for argument in &mut angled.args {
                match argument {
                    GenericArgument::Type(ty) => infer_impl_traits(ty),
                    GenericArgument::AssocType(assoc) => infer_impl_traits(&mut assoc.ty),
                    _ => {}
                }
            }
        }
        PathArguments::Parenthesized(parenthesized) => {
            parenthesized
                .inputs
                .iter_mut()
                .for_each(|input| infer_impl_traits(&mut input.ty));
            if let ReturnType::Type(_, ty) = &mut parenthesized.output {
                infer_impl_traits(ty);
            }
        }
        PathArguments::None => {}
    }
}

/// The first `impl Trait` a path segment's arguments hold.
fn impl_trait_within(arguments: &PathArguments) -> Option<&Type> {
    match arguments {
        PathArguments::AngleBracketed(angled) => {
            angled.args.iter().find_map(|argument| match argument {
                GenericArgument::Type(ty) => impl_trait(ty),
                GenericArgument::AssocType(assoc) => impl_trait(&assoc.ty),
                _ => None,
            })
        }
        PathArguments::Parenthesized(parenthesized) => parenthesized
            .inputs
            .iter()
            .find_map(|input| impl_trait(&input.ty))
            .or(match &parenthesized.output {
                ReturnType::Type(_, ty) => impl_trait(ty),
                ReturnType::Default => None,
            }),
        PathArguments::None => None,
    }
}

/// The first binding `pat` makes `mut`, with its name — wherever the pattern
/// binds it.
fn mutable_binding(pat: &Pat) -> Option<(Span, &Ident)> {
    match pat {
        Pat::Ident(binding) => match &binding.mutability {
            Some(mutability) => Some((mutability.span, &binding.ident)),
            None => binding
                .subpat
                .as_ref()
                .and_then(|(_, sub)| mutable_binding(sub)),
        },
        Pat::Tuple(tuple) => tuple.elems.iter().find_map(mutable_binding),
        Pat::TupleStruct(tuple) => tuple.elems.iter().find_map(mutable_binding),
        Pat::Struct(fields) => fields
            .fields
            .iter()
            .find_map(|field| mutable_binding(&field.pat)),
        Pat::Slice(slice) => slice.elems.iter().find_map(mutable_binding),
        Pat::Reference(reference) => mutable_binding(&reference.pat),
        Pat::Paren(paren) => mutable_binding(&paren.pat),
        Pat::Type(typed) => mutable_binding(&typed.pat),
        _ => None,
    }
}

/// `ty` when it is an exclusive reference, read through parentheses, an
/// invisible group and a tuple's elements.
fn exclusive_reference(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Reference(reference) if reference.mutability.is_some() => Some(ty),
        Type::Paren(paren) => exclusive_reference(&paren.elem),
        Type::Group(group) => exclusive_reference(&group.elem),
        Type::Tuple(tuple) => tuple.elems.iter().find_map(exclusive_reference),
        _ => None,
    }
}

/// What the method answers, as the expansion writes it — every token at one
/// span, the expansion's own, located at the return type — and that span.
///
/// Every helper the expansion calls is bounded on the answer there, so a
/// refused answer is one error: rustc reports one predicate once per span, and
/// a stable macro can give it no wider span than the type's first token.
fn answer(method: &ImplItemFn) -> (TokenStream, Span) {
    // The expansion's own, so no lint reads its `.await` as one a developer wrote:
    // clippy's `large_futures` would offer a fix at the answer that none can apply.
    let at = Span::mixed_site().located_at(answer_span(&method.sig));
    let answer = match &method.sig.output {
        ReturnType::Type(_, ty) => respanned(ty.to_token_stream(), at),
        ReturnType::Default => quote_spanned!(at=> ()),
    };
    (answer, at)
}

/// `method`'s body, taken out of it and kept whole in the braces it was written
/// in, inside a block of the expansion's own opening on a `return` of the
/// attempt's type never taken: rustc types an async block's output from its
/// body, so without it no answer is coerced. Nothing is written in the body's
/// own block, where an item it declares — a `use … as Error`, a `type` of the
/// answer's name — is in scope of every statement, the expansion's included.
///
/// The expansion's block is of this crate's edition, 2024, whatever the
/// method's: what the body's tail expression leaves drops there ahead of the
/// expansion's own locals, and after the body's or ahead of them as the
/// edition its braces were written in says.
///
/// A body `#[tracing::instrument]` wrapped answers the attempt's type inside
/// the future it built too, and where each attempt `borrows` the arguments,
/// that future borrows them as the body it holds would. `captured`, the
/// statement fixing the order an attempt owning the arguments drops them in,
/// leads the expansion's block — left out under an instrument, whose own future
/// takes them from the attempt's in the order tracing's does with no policy —
/// then `bound`, the `let` naming the attempt's token where the closure's
/// parameter cannot.
fn body(
    method: &mut ImplItemFn,
    answer: &TokenStream,
    at: Span,
    borrows: bool,
    captured: Option<Stmt>,
    bound: Option<Stmt>,
) -> syn::Block {
    let mut stmts = std::mem::take(&mut method.block.stmts);
    let mut wrapped = false;
    // `expansion` refused a shape this stops at, and `kept` gave it no body to type.
    instrumented(&mut stmts, &mut |part| match part {
        Part::Answer(ty) => {
            *ty = Type::Verbatim(quote_spanned! {at=>
                <#answer as ::nest_rs_resilience::__private::ResilientAnswer>::Attempt
            });
        }
        Part::Future(future) => {
            wrapped = true;
            if borrows {
                future.capture = None;
            }
        }
        Part::Written(_) => {}
    })
    .unwrap_or_default();
    let written = method.block.brace_token;
    let mut around: Vec<Stmt> = captured.filter(|_| !wrapped).into_iter().collect();
    around.extend(bound);
    around.push(Stmt::Expr(
        Expr::Verbatim(quote_spanned! {at=>
            if false {
                return ::nest_rs_resilience::__private::attempt::<#answer>();
            }
        }),
        None,
    ));
    around.push(as_written(written, stmts));
    syn::Block {
        brace_token: syn::token::Brace(Span::mixed_site().located_at(written.span.join())),
        stmts: around,
    }
}

/// `stmts`, a method's body, as the last expression of a block the expansion
/// writes its own statements ahead of: in the braces it was `written` in,
/// whose span carries the edition its tail expression's temporaries drop by,
/// inside parentheses of the expansion's own — rustc reports braces a
/// developer wrote around one expression as unneeded where they end a block,
/// and neither them nor the parentheses where a macro wrote those.
fn as_written(written: syn::token::Brace, stmts: Vec<Stmt>) -> Stmt {
    let block = Expr::Block(syn::ExprBlock {
        attrs: Vec::new(),
        label: None,
        block: syn::Block {
            brace_token: written,
            stmts,
        },
    });
    Stmt::Expr(
        Expr::Paren(syn::ExprParen {
            attrs: Vec::new(),
            paren_token: syn::token::Paren(Span::mixed_site()),
            expr: Box::new(block),
        }),
        None,
    )
}

/// The statement an attempt owning the arguments opens on: each argument its
/// body names, by reference, last declared first, a pattern's names last bound
/// first. A closure and the future it answers drop what they captured in the
/// order their tokens first name it, where an `async fn` drops its parameters
/// last declared first: read here first, the arguments drop as the undecorated
/// method's do, whether the attempt answered, panicked, was dropped or was
/// never sent.
///
/// It reads no argument the body does not, which rustc then still reports
/// unused; none of a reference's type or bound by reference, which drops
/// nothing; and none named with a leading `_`, a read clippy reports of a
/// binding so named wherever it is: one the body names is captured where the
/// body first does, after those read here. It is written where the body names
/// two arguments or more, one having no order.
///
/// An attempt lending the arguments opens on none, under `record_if` neither,
/// whose body may consume one: read whole there, an argument the body reads a
/// part of, or names without reading, would be borrowed whole across the
/// attempt's waits — no `Send` future where it is no `Sync`, and no answer
/// reborrowing a `&mut` it holds.
fn captured(method: &ImplItemFn) -> Option<Stmt> {
    let written: TokenStream = method
        .block
        .stmts
        .iter()
        .map(ToTokens::to_token_stream)
        .collect();
    let own = Span::mixed_site();
    let (mut read, mut captured_later) = (Vec::new(), false);
    for typed in typed_inputs(method) {
        if is_reference(&typed.ty) {
            continue;
        }
        let cfgs = cfg_attrs(&typed.attrs);
        let mut bound = Bound::default();
        bound.read(&typed.pat);
        for binding in bound.names {
            let name = binding.ident.unraw().to_string();
            if binding.by_ref.is_some() || !names(written.clone(), &name) {
                continue;
            }
            if name.starts_with('_') {
                captured_later = true;
                continue;
            }
            let argument = &binding.ident;
            read.push(quote_spanned!(own=> #(#cfgs)* &#argument));
        }
    }
    let ordered = read.len() > 1 || (captured_later && !read.is_empty());
    let read = read.iter().rev();
    ordered.then(|| syn::parse_quote_spanned!(own=> let _ = (#(#read,)*);))
}

/// Whether `ty` is a reference, read through parentheses and an invisible
/// group, by spelling: an alias of one is read as owning.
fn is_reference(ty: &Type) -> bool {
    match ty {
        Type::Reference(_) => true,
        Type::Paren(paren) => is_reference(&paren.elem),
        Type::Group(group) => is_reference(&group.elem),
        _ => false,
    }
}

/// Whether `tokens`, a body, name `name`: as a word of their own at any depth,
/// a macro's arguments included, or as an argument a string literal's format
/// captures. A word after a `.`, a `::` or a `'` is a member, a path's segment
/// or a lifetime, never an argument. Read by tokens, so any other word of that
/// spelling — a local shadowing the argument, a field a struct literal sets —
/// reads as the argument.
fn names(tokens: TokenStream, name: &str) -> bool {
    let mut before = [None, None];
    for tree in tokens {
        let punct = match &tree {
            TokenTree::Punct(punct) => Some(punct.as_char()),
            _ => None,
        };
        let named = match tree {
            TokenTree::Ident(word) => {
                let another = match before {
                    [_, Some('\'')] | [Some(':'), Some(':')] => true,
                    [earlier, Some('.')] => earlier != Some('.'),
                    _ => false,
                };
                !another && word.unraw() == name
            }
            TokenTree::Group(group) => names(group.stream(), name),
            TokenTree::Literal(literal) => formats(&literal.to_string(), name),
            TokenTree::Punct(_) => false,
        };
        if named {
            return true;
        }
        before = [before[1], punct];
    }
    false
}

/// Whether `literal`, a token as written, is a string whose format captures
/// `name`: in an argument's position (`{name}`, `{name:?}`) or as a width or a
/// precision (`{:name$}`, `{:.name$}`). `{{` writes a brace, and `\u{…}` a
/// character, unless its `\` is itself escaped.
fn formats(literal: &str, name: &str) -> bool {
    if !literal.starts_with(['"', 'r']) {
        return false;
    }
    let mut rest = literal;
    while let Some((before, after)) = rest.split_once('{') {
        if let Some(escaped) = after.strip_prefix('{') {
            rest = escaped;
            continue;
        }
        let Some((placeholder, after)) = after.split_once('}') else {
            return false;
        };
        rest = after;
        let escaping = before
            .strip_suffix('u')
            .map_or(0, |head| head.len() - head.trim_end_matches('\\').len());
        if escaping % 2 == 1 && literal.starts_with('"') {
            continue;
        }
        let (argument, spec) = placeholder.split_once(':').unwrap_or((placeholder, ""));
        let mut counts = spec.rsplit('$').skip(1);
        if argument.trim() == name || counts.any(|count| ends_on_word(count, name)) {
            return true;
        }
    }
    false
}

/// Whether the word `text` ends on is `name`.
fn ends_on_word(text: &str, name: &str) -> bool {
    text.strip_suffix(name)
        .is_some_and(|before| !before.ends_with(|last: char| last.is_alphanumeric() || last == '_'))
}

/// `method` with its body run as `plan` says, the policy's field and the
/// fallback read through `receiver`, the method's own `self`.
fn expand(mut method: ImplItemFn, receiver: Token![self], plan: &Plan<'_>) -> TokenStream {
    let (answer, at) = answer(&method);
    let field = plan.field;
    // `self` resolves as the receiver does, which a `macro_rules!` may write apart
    // from the attribute, and sits at the field, where the field's refusal points.
    let this = |at: Span| Token![self](receiver.span.located_at(at));
    let policy = {
        let receiver = this(field.span());
        // The expansion's own, located at the field: a lint reading the path as one
        // the developer wrote would report it at a name they cannot change.
        quote_spanned!(Span::mixed_site().located_at(field.span())=>
            ::nest_rs_resilience::__private::policy(&#receiver.#field)
        )
    };
    let name = method.sig.ident.unraw().to_string();
    let timeout = optional(plan.timeout_millis);
    let retries = optional(plan.retries);
    let declares = plan.fallback.as_ref().map(|_| quote!(.with_fallback()));
    let site = quote! {
        ::nest_rs_resilience::__private::resilient_method(
            ::core::any::type_name::<Self>,
            #name,
            #timeout,
            #retries,
        )
        #declares
    };
    let lends = plan.lends;
    // A closure lending the arguments cannot lend its own parameter to the future it
    // answers: the attempt's block takes the token by value, under the name written.
    let (token, bound) = match (plan.cancel, lends) {
        (None, _) => (quote!(_), None),
        (Some(name), None) => (name.to_token_stream(), None),
        (Some(name), Some(_)) => {
            let handed = mixed_site_ident("__cancel");
            let bound: Stmt = syn::parse_quote!(let #name = #handed;);
            (handed.into_token_stream(), Some(bound))
        }
    };
    let handed = plan
        .fallback
        .as_ref()
        .map(|fallback| arguments(&method, fallback.key.span()));
    let captured = match (lends, plan.reads) {
        (None, false) => captured(&method),
        _ => None,
    };
    let block = body(&mut method, &answer, at, lends.is_some(), captured, bound);
    let every = quote_spanned!(at=> ::nest_rs_resilience::__private::every);
    let written = |predicate: Option<&Expr>| match predicate {
        Some(predicate) => predicate.to_token_stream(),
        None => every.clone(),
    };
    let (retry_if, record_if) = (written(plan.retry_if), written(plan.record_if));
    let retry_ok_if = match plan.retry_ok_if {
        Some(predicate) => predicate.to_token_stream(),
        None => quote_spanned!(at=> ::nest_rs_resilience::__private::none),
    };
    let named = quote_spanned!(at=> ::core::marker::PhantomData::<#answer>);
    let judging = quote! {
        ::nest_rs_resilience::__private::judging(#named, #retry_if, #retry_ok_if, #record_if)
    };
    // Spanned at the key lending the arguments, where rustc's words for a body consuming
    // one point; its head alone is the expansion's, or a lint offers `async` in the key.
    let attempt = match lends {
        Some(key) => {
            let taking = quote_spanned!(Span::mixed_site().located_at(key.span())=> |#token|);
            quote_spanned!(key.span()=> #taking async #block)
        }
        None => quote!(move |#token| async move #block),
    };
    let run = match plan.idempotent {
        Some(_) => Ident::new("run_idempotent", at),
        None => Ident::new("run", at),
    };
    let run = quote_spanned! {at=>
        ::nest_rs_resilience::__private::#run::<#answer, _, _, _, _>(
            #policy, #site, #judging, #attempt,
        )
        .await
    };
    let call = match (&plan.fallback, handed) {
        (Some(fallback), Some(handed)) => {
            let Fallback { method, when, .. } = fallback;
            let receiver = this(method.span());
            let error = Ident::new("__error", Span::mixed_site().located_at(method.span()));
            let when = written(*when);
            // Last declared first: a fallback never called drops what its closure
            // took in the order these name it, as the method would its arguments.
            let bound = handed.iter().rev().map(|Handed { name, under, cfgs }| {
                quote_spanned!(method.span()=> #(#cfgs)* let #under = #name;)
            });
            let passed = handed
                .iter()
                .map(|Handed { under, cfgs, .. }| quote!(#(#cfgs)* #under));
            let answering = quote_spanned!(method.span()=> |#error| {
                #(#bound)*
                #receiver.#method(#(#passed,)* #error)
            });
            // rustc reads an `.await` as no argument of its own to blame for what the
            // run answers, and reports it once more at the attribute; a call it does.
            let ran = quote_spanned!(at=> ::core::convert::identity(#run));
            // Two calls: the filter's borrow of an argument ends with the first, and
            // the closure moving that argument into the fallback is built for the second.
            let ended = quote! {
                ::nest_rs_resilience::__private::fall_back(#policy, #site, #ran, #when)
            };
            let fell_back = quote! {
                ::nest_rs_resilience::__private::Fallback::in_place_of(#ended, #answering)
            };
            quote_spanned!(at=> ::core::convert::identity(#fell_back).await)
        }
        _ => run,
    };
    let read = plan
        .reads
        .then(|| reads(&method, Read::BesideItsBody))
        .flatten();
    method.block.stmts = run_under(read, call);
    method.into_token_stream()
}

/// The body of a method whose own was taken into `run`, or left out for it:
/// the expansion's mark, what a kept form reads of its arguments, then `run`.
fn run_under(read: Option<Stmt>, run: TokenStream) -> Vec<Stmt> {
    let mut stmts = vec![run_mark()];
    stmts.extend(read);
    stmts.push(Stmt::Expr(Expr::Verbatim(run), None));
    stmts
}

/// An argument of the method as its fallback is handed it.
struct Handed {
    /// The name the method binds it to: resolved as the method binds it, which
    /// a `macro_rules!` may write apart from the attribute, located at the key
    /// handing it on.
    name: Ident,
    /// The expansion's own local it is passed under: rustc offers its fixes on
    /// an argument the developer wrote, and one there would rewrite the key.
    under: Ident,
    /// The `#[cfg]`s its parameter is under, which the local and the argument
    /// passed are under too: a build names the arguments it has.
    cfgs: Vec<TokenStream>,
}

/// Each argument of `method` as its fallback is handed it, in order, located
/// `at` the key that hands it on.
fn arguments(method: &ImplItemFn, at: Span) -> Vec<Handed> {
    typed_inputs(method)
        .filter_map(|typed| Some((plain_name(&typed.pat)?, cfg_attrs(&typed.attrs))))
        .enumerate()
        .map(|(index, (bound, cfgs))| {
            let mut name = bound.clone();
            name.set_span(bound.span().located_at(at));
            let under = Ident::new(
                &format!("__handed_{index}"),
                Span::mixed_site().located_at(at),
            );
            Handed { name, under, cfgs }
        })
        .collect()
}

/// `method` with its body checked as one attempt of `answer` against a
/// stand-in that reads no field and never runs, its token bound under the name
/// `cancel` gave, when it gave one, and each of its arguments read first.
fn stand_in(
    mut method: ImplItemFn,
    cancel: Option<&Ident>,
    (answer, at): (TokenStream, Span),
) -> TokenStream {
    let read = reads(&method, Read::BesideItsBody);
    let block = body(&mut method, &answer, at, false, None, None);
    let token = match cancel {
        Some(name) => name.to_token_stream(),
        None => quote!(_),
    };
    let checked = quote_spanned! {at=>
        ::nest_rs_resilience::__private::refused::<#answer, _, _>(move |#token| async move #block)
    };
    method.block.stmts = run_under(read, checked);
    method.into_token_stream()
}

/// `Some(value)` or `None`, as the expansion writes it.
fn optional(value: Option<impl ToTokens>) -> TokenStream {
    match value {
        Some(value) => quote!(::core::option::Option::Some(#value)),
        None => quote!(::core::option::Option::None),
    }
}

/// `tokens`, every one at `span` — but a `$crate`, located there and resolved
/// as it was written: the crate it names is its own context's, never a span's.
fn respanned(tokens: TokenStream, span: Span) -> TokenStream {
    tokens
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Group(group) => {
                let mut moved = Group::new(group.delimiter(), respanned(group.stream(), span));
                moved.set_span(span);
                TokenTree::Group(moved)
            }
            TokenTree::Ident(mut root) if root == "$crate" => {
                root.set_span(root.span().located_at(span));
                TokenTree::Ident(root)
            }
            mut other => {
                other.set_span(span);
                other
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refusals(args: TokenStream, item: TokenStream) -> Vec<String> {
        match expansion(args, item) {
            Ok(expanded) => panic!("expanded, refusing nothing: {expanded}"),
            Err(Refused { refusal, .. }) => refusal
                .into_iter()
                .map(|refusal| refusal.to_string())
                .collect(),
        }
    }

    fn kept(args: TokenStream, item: TokenStream) -> String {
        match expansion(args, item) {
            Ok(expanded) => panic!("expanded, refusing nothing: {expanded}"),
            Err(Refused { kept, .. }) => kept.to_string(),
        }
    }

    #[test]
    fn a_method_runs_its_body_under_the_policy_of_its_field() {
        let expanded = resilient(
            quote!(carrier, idempotent, timeout = "2s", retries = 3),
            quote! {
                pub async fn rates(&self, zone: &str) -> Result<u32, ResilienceError<E>> {
                    quote(zone).await
                }
            },
        )
        .to_string();
        for written in [
            "run_idempotent :: < Result < u32 , ResilienceError < E > > , _ , _ , _ , _ >",
            "policy (& self . carrier)",
            "resilient_method (:: core :: any :: type_name :: < Self > , \"rates\" , :: core \
             :: option :: Option :: Some (2000u64) , :: core :: option :: Option :: Some (3u32) ,)",
            "| _ | async { if false { return :: nest_rs_resilience :: __private :: attempt :: < \
             Result < u32 , ResilienceError < E > > > () ; } ({ quote (zone) . await }) }",
        ] {
            assert!(expanded.contains(written), "{written} in {expanded}");
        }
    }

    #[test]
    fn a_method_run_once_moves_its_arguments_into_its_one_attempt() {
        let expanded = resilient(
            quote!(carrier),
            quote! {
                async fn ship(&self, order: Order) -> Result<(), ResilienceError<E>> {
                    send(order).await
                }
            },
        )
        .to_string();
        assert!(
            expanded.contains("run :: < Result < () , ResilienceError < E > > , _ , _ , _ , _ >"),
            "{expanded}"
        );
        assert!(
            expanded.contains(
                "move | _ | async move { if false { return :: nest_rs_resilience :: __private :: \
                 attempt :: < Result < () , ResilienceError < E > > > () ; } ({ send (order) . await \
                 }) }"
            ),
            "{expanded}"
        );
    }

    /// The attempt owning the arguments reads first, last declared first, each
    /// one its body names: its closure and its future capture them in that
    /// order, the one an `async fn` drops its parameters in.
    #[test]
    fn an_attempt_owning_the_arguments_reads_those_its_body_names_last_declared_first() {
        let arguments = quote! {
            &self,
            held: Guard,
            (first, ref borrowed, Pair { second, .. }): (Label, Label, Pair),
            zone: &str,
            _kept: Guard,
            unread: Guard,
            #[cfg(unix)] mut r#type: Kind,
            out: Writer
        };
        let item = quote! {
            async fn append(#arguments) -> Answer {
                let line = format!("{first:>width$}", width = zone.len());
                r#type.push(second);
                send(_kept, borrowed, self.unread(), line, &held).await?;
                out.flush().await
            }
        };
        let read = "let _ = (& out , # [cfg (unix)] & r#type , & second , & first , & held ,) ;";
        let once = resilient(quote!(carrier), item.clone()).to_string();
        assert!(
            once.contains(&format!("move | _ | async move {{ {read} if false {{")),
            "{once}"
        );
        let named = resilient(quote!(carrier, cancel = token), item.clone()).to_string();
        assert!(
            named.contains(&format!("move | token | async move {{ {read} if false {{")),
            "{named}"
        );
        let read_only = quote! {
            async fn append(&self, held: Guard, out: Writer) -> Answer {
                out.write(&held).await
            }
        };
        for lending in [
            quote!(carrier, idempotent),
            quote!(carrier, record_if = counts),
            quote!(carrier, fallback = stale),
        ] {
            let lent = resilient(lending.clone(), read_only.clone()).to_string();
            assert!(
                !lent.contains("let _ ="),
                "#[resilient({lending})] leaves the arguments in the method's frame: {lent}"
            );
        }
        let refused = kept(quote!(carrier, retries = 2), item);
        assert_eq!(
            refused.matches("let _ =").count(),
            1,
            "a kept form reads every argument once, ahead of its run: {refused}"
        );

        for (arguments, body) in [
            (
                quote!(held: Guard, out: &mut Writer),
                quote!(out.write(&held).await),
            ),
            (
                quote!(held: Guard, out: Writer),
                quote!(self.out.write(held).await),
            ),
            (
                quote!(&(held, out): &(Guard, Writer)),
                quote!(out.write(held).await),
            ),
            (
                quote!(held: Guard, out: Writer),
                quote!(self.flush("{{held}} {out:?}", out).await),
            ),
        ] {
            let one = resilient(
                quote!(carrier),
                quote!(async fn append(&self, #arguments) -> Answer { #body }),
            )
            .to_string();
            assert!(
                !one.contains("let _ ="),
                "fewer than two arguments drop in an order of their own: {one}"
            );
        }

        let underscored = resilient(
            quote!(carrier),
            quote! {
                async fn append(&self, _first: Guard, held: Guard, _out: Writer) -> Answer {
                    _out.write(&held).await
                }
            },
        )
        .to_string();
        assert!(
            underscored.contains("move | _ | async move { let _ = (& held ,) ; if false {"),
            "read first, `held` is captured ahead of the one named with a `_`: {underscored}"
        );

        let wrapped = resilient(
            quote!(carrier),
            quote! {
                #[allow(unused)]
                async fn ship(&self, order: Order, label: Label) -> Answer {
                    {}
                    let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
                    let __tracing_instrument_future = async move {
                        if false {
                            let __tracing_attr_fake_return: Answer = loop {};
                            return __tracing_attr_fake_return;
                        }
                        { send(order, label).await }
                    };
                    __tracing_instrument_future.await
                }
            },
        )
        .to_string();
        assert!(
            wrapped.contains("move | _ | async move { if false {"),
            "tracing's own future takes the arguments from the attempt's: {wrapped}"
        );
    }

    #[test]
    fn a_format_string_names_the_arguments_it_captures() {
        for literal in [
            r#""{held}""#,
            r#""sent {held:?} once""#,
            r#""{ held }""#,
            r#""{0:held$}""#,
            r#""{:>held$.2}""#,
            r#""{:.held$}""#,
            r#""{out:>7.held$}""#,
            r#""{{{held}}}""#,
            r##"r#"{held}"#"##,
            r#"r"\u{held}""#,
            r#""\\u{held}""#,
        ] {
            assert!(formats(literal, "held"), "{literal}");
        }
        for literal in [
            r#""held""#,
            r#""{{held}}""#,
            r#""{upheld}""#,
            r#""{held_back}""#,
            r#""{:upheld$}""#,
            r#""{0:held}""#,
            r#""{out:held}""#,
            r#""\u{held}""#,
            r#""\\\u{held}""#,
            r#"b"{held}""#,
            r#"c"{held}""#,
            "'h'",
            "4",
        ] {
            assert!(!formats(literal, "held"), "{literal}");
        }
        for naming in [
            quote!(log!(target: "x", "{held}")),
            quote!(send(|| async { r#held.len() })),
            quote!(first..held),
            quote!(first..=held),
            quote!(Sent { at: held }),
            quote!(Sent { held }),
        ] {
            assert!(names(naming.clone(), "held"), "{naming}");
        }
        for another in [
            quote!(send(upheld, "held")),
            quote!(self.held.lock().await),
            quote!(order.0.held()),
            quote!(Kind::held(order)),
            quote!('held: loop {
                break 'held;
            }),
        ] {
            assert!(!names(another.clone(), "held"), "{another}");
        }
    }

    /// A predicate may read an argument once the one attempt answered: under
    /// `record_if` that attempt borrows the arguments, its token a `let` of its
    /// block, and the future an instrument built borrows them too.
    #[test]
    fn a_method_run_once_under_a_predicate_lends_its_arguments_to_its_one_attempt() {
        let item = quote! {
            async fn ship(&self, order: Order) -> Answer {
                send(&order).await
            }
        };
        for record_if in [quote!(|_| order.urgent()), quote!(Down::counts)] {
            let once = resilient(quote!(carrier, record_if = #record_if), item.clone()).to_string();
            assert!(once.contains("run :: < Answer , _ , _ , _ , _ >"), "{once}");
            assert!(
                once.contains(&format!("{record_if}) , | _ | async {{ if false")),
                "{once}"
            );
            assert!(!once.contains("move"), "{once}");
            assert!(!once.contains("with_fallback"), "{once}");
        }
        let bound =
            resilient(quote!(carrier, cancel = token, record_if = counts), item).to_string();
        assert!(
            bound.contains("| __cancel | async { let token = __cancel ; if false"),
            "{bound}"
        );
        let wrapped = resilient(
            quote!(carrier, record_if = counts),
            instrumented_above(quote!(send(&order).await)),
        )
        .to_string();
        assert!(
            wrapped.contains("let __tracing_instrument_future = async {"),
            "{wrapped}"
        );
    }

    /// The attempt's type is the first answer of the block the expansion
    /// writes around the body's, so rustc checks each answer the body writes
    /// against it, and the body's items still lead its own block.
    #[test]
    fn the_attempts_type_is_answered_first_ahead_of_the_block_the_body_was_written_in() {
        let expanded = resilient(
            quote!(carrier),
            quote! {
                async fn ship(&self, order: Order) -> Answer {
                    use std::fmt::Write as _;
                    const LABEL: &str = "parcel";
                    let label = LABEL;
                    send(order, label).await
                }
            },
        )
        .to_string();
        assert!(
            expanded.contains(
                "async move { if false { return :: nest_rs_resilience :: __private :: attempt :: < \
                 Answer > () ; } ({ use std :: fmt :: Write as _ ; const LABEL : & str = \"parcel\" \
                 ; let label = LABEL ;"
            ),
            "{expanded}"
        );
    }

    #[test]
    fn every_refusal_of_one_method_is_reported_at_once() {
        let refused = refusals(
            quote!(carrier, idempotent),
            quote! {
                fn ship(&mut self, mut order: Order, items: &mut Vec<u8>) -> impl Future {
                    send(order)
                }
            },
        );
        assert_eq!(refused.len(), 5, "{refused:#?}");
        assert!(refused[0].starts_with("#[resilient] decorates an `async fn`"));
        assert!(refused[1].starts_with("a `#[resilient]` method borrows its host"));
        assert!(refused[2].starts_with("#[resilient] writes the method's return type"));
        assert!(refused[3].contains("bound `mut`"));
        assert!(refused[4].contains("is a `&mut`"));
    }

    #[test]
    fn retries_without_idempotent_are_refused_before_their_value_is_read() {
        let refused = refusals(
            quote!(carrier, retries = 11),
            quote!(
                async fn ship(&self) -> Answer {
                    send().await
                }
            ),
        );
        assert_eq!(refused.len(), 1, "{refused:#?}");
        assert!(refused[0].starts_with("#[resilient] `retries` needs `idempotent`"));
    }

    fn track() -> TokenStream {
        quote!(
            async fn track(&self) -> Answer {
                send().await
            }
        )
    }

    /// `idempotent` written after a refused key is read like one before it:
    /// the one refusal is the key's, and none says `retries` needs what the
    /// method wrote.
    #[test]
    fn a_refused_key_adds_no_refusal_for_what_the_list_did_not_read() {
        for unread in [
            quote!(carrier, retries = 3, retry = 1, idempotent),
            quote!(
                carrier,
                retries = 3,
                timeout = "1s",
                timeout = "2s",
                idempotent
            ),
        ] {
            let refused = refusals(unread.clone(), track());
            assert_eq!(refused.len(), 1, "#[resilient({unread})]: {refused:#?}");
            assert!(
                !refused[0].contains("needs `idempotent`"),
                "#[resilient({unread})]: {refused:#?}"
            );
        }
    }

    /// `idempotent` stands on neither side of the refused key, so the need it
    /// leaves unmet is refused beside that key.
    #[test]
    fn a_refused_key_keeps_the_need_the_whole_list_leaves_unmet() {
        for args in [
            quote!(carrier, retries = 3, retry = 1),
            quote!(carrier, retries = 3, timeout),
            quote!(carrier, retries = 3, concurrency = 4),
            quote!(carrier, retry = 1, retries = 0),
        ] {
            let refused = refusals(args.clone(), track());
            assert_eq!(refused.len(), 2, "#[resilient({args})]: {refused:#?}");
            assert!(
                refused[1].starts_with("#[resilient] `retries` needs `idempotent`"),
                "#[resilient({args})]: {refused:#?}"
            );
        }
    }

    #[test]
    fn a_value_is_judged_on_either_side_of_a_refused_key() {
        for (args, said) in [
            (
                quote!(carrier, retries = 11, retry = 1, idempotent),
                "#[resilient] `retries`: must be at most 10",
            ),
            (
                quote!(carrier, idempotent, retries = 11, retry = 1),
                "#[resilient] `retries`: must be at most 10",
            ),
            (
                quote!(carrier, idempotent, retry = 1, retries = 0),
                "#[resilient] `retries`: `0` turns retries off",
            ),
            (
                quote!(carrier, timeout = "11m", retry = 1),
                "#[resilient] `timeout`: must be at most 600000 milliseconds",
            ),
            (
                quote!(carrier, retry = 1, timeout = "11m"),
                "#[resilient] `timeout`: must be at most 600000 milliseconds",
            ),
        ] {
            let refused = refusals(args.clone(), track());
            assert_eq!(refused.len(), 2, "#[resilient({args})]: {refused:#?}");
            for sentence in ["unknown #[resilient] argument `retry`", said] {
                assert!(
                    refused.iter().any(|refused| refused.starts_with(sentence)),
                    "#[resilient({args})]: {refused:#?}"
                );
            }
        }
    }

    /// The reading resumes after the refused key's value, the commas that
    /// value holds outside a group included, never inside it.
    #[test]
    fn the_list_is_read_on_after_a_refused_keys_value() {
        for args in [
            quote!(carrier, retries = 3, retry = pick::<A, B>, idempotent),
            quote!(carrier, retries = 3, retry = |a, b| a < b, idempotent),
            quote!(carrier, retries = 3, retry(1, 2), idempotent),
            quote!(carrier, retries = 3, retry, idempotent),
            quote!(carrier, retries = 3, "2s", idempotent),
            quote!(carrier, retries = 3, timeout, idempotent),
            quote!(carrier, retries = 3, record_if, idempotent),
            quote!(carrier, retries = 3, cancel = 7, idempotent),
            quote!(carrier, retries = 3, idempotent = true),
            quote!(carrier, retries = 3, idempotent(true), timeout = "2s"),
        ] {
            let refused = refusals(args.clone(), track());
            assert_eq!(refused.len(), 1, "#[resilient({args})]: {refused:#?}");
            assert!(
                !refused[0].contains("needs `idempotent`"),
                "#[resilient({args})]: {refused:#?}"
            );
        }
    }

    #[test]
    fn a_key_written_on_both_sides_of_a_refused_one_is_written_twice() {
        for (args, refusals_expected) in [
            (
                quote!(carrier, timeout = "1s", retry = 1, timeout = "2s"),
                2,
            ),
            (quote!(carrier, timeout, retry = 1, timeout = "2s"), 3),
        ] {
            let refused = refusals(args.clone(), track());
            assert_eq!(
                refused.len(),
                refusals_expected,
                "#[resilient({args})]: {refused:#?}"
            );
            assert_eq!(
                refused.last().map(String::as_str),
                Some("#[resilient] takes at most one `timeout`"),
                "#[resilient({args})]: {refused:#?}"
            );
        }
    }

    /// `idempotent` refused for the value it was given is still declared, so
    /// what it asks of the method is refused beside it.
    #[test]
    fn a_key_refused_for_its_value_is_still_written() {
        let refused = refusals(
            quote!(carrier, idempotent = true),
            quote!(
                async fn ship(&self, mut order: Order) -> Answer {
                    send(order).await
                }
            ),
        );
        assert_eq!(refused.len(), 2, "{refused:#?}");
        assert!(
            refused[0].starts_with("#[resilient] `idempotent` takes no value"),
            "{refused:#?}"
        );
        assert!(refused[1].contains("bound `mut`"), "{refused:#?}");
    }

    /// A value that reads as no expression, or tokens between a value and its
    /// comma: the reading ends there, and `idempotent`, which may stand in
    /// what is left, is neither missed nor judged beside.
    #[test]
    fn a_list_read_no_further_derives_nothing_from_what_it_left_unread() {
        for args in [
            quote!(carrier, retries = 3, timeout = "1s" "2s", idempotent),
            quote!(carrier, retries = 3, timeout = 1 +, idempotent),
            quote!(carrier, retries = 3, retry = 1 +, idempotent),
            quote!(carrier, retries = 3, retry_if = 1 +, idempotent),
            quote!(carrier, retries = 3, fallback = |, idempotent),
            quote!(carrier, retries = 3, timeout = match a, b),
            quote!(carrier, retries = 3, retry = match a, b),
            quote!(carrier, retries = 3, retry::key = 1, idempotent),
            quote!(carrier, retries = 0, retry 1),
        ] {
            let refused = refusals(args.clone(), track());
            assert_eq!(refused.len(), 1, "#[resilient({args})]: {refused:#?}");
            assert!(
                !refused[0].contains("needs `idempotent`"),
                "#[resilient({args})]: {refused:#?}"
            );
        }
        let read = refusals(
            quote!(carrier, idempotent, retries = 11, timeout = "1s" "2s"),
            track(),
        );
        assert_eq!(read.len(), 2, "{read:#?}");
        assert!(
            read[1].starts_with("#[resilient] `retries`: must be at most 10"),
            "what was read is judged: {read:#?}"
        );
    }

    /// Every key that needs another is refused in one sentence, naming both.
    #[test]
    fn every_key_needing_another_is_refused_without_it_in_one_sentence() {
        for need in &NEEDS {
            let args: TokenStream = format!("carrier, {} = pick", need.key)
                .parse()
                .expect("arguments");
            let refused = refusals(
                args,
                quote!(
                    async fn track(&self) -> Answer {
                        send().await
                    }
                ),
            );
            assert_eq!(
                refused,
                [format!(
                    "#[resilient] `{}` needs `{}`: {}",
                    need.key,
                    need.needs,
                    need.why()
                )]
            );
        }
    }

    /// Each predicate key hands its value to the judging as written, a path or
    /// a closure, and a key left out its default.
    #[test]
    fn a_predicate_is_handed_to_the_judging_as_written_and_one_left_out_as_its_default() {
        let item = quote! {
            async fn rates(&self, zone: &str) -> Answer {
                quote(zone).await
            }
        };
        let judged = |args: TokenStream| {
            let expanded = resilient(args, item.clone()).to_string();
            let (_, judging) = expanded.split_once("judging (").expect("a judging");
            let attempt = judging.find(") , move | _ |").or(judging.find(") , | _ |"));
            judging[..attempt.expect("then the attempt")].to_owned()
        };
        let named = ":: core :: marker :: PhantomData :: < Answer >";
        let every = ":: nest_rs_resilience :: __private :: every";
        let none = ":: nest_rs_resilience :: __private :: none";
        assert_eq!(
            judged(quote!(carrier, idempotent)),
            format!("{named} , {every} , {none} , {every}")
        );
        assert_eq!(
            judged(quote!(carrier, record_if = Down::counts)),
            format!("{named} , {every} , {none} , Down :: counts")
        );
        assert_eq!(
            judged(quote!(
                carrier,
                retry_if = |error| matches!(error, Down::Busy | Down::Away),
                idempotent,
                retry_ok_if = |quote: &Quote| quote.pending(1, 2),
                record_if = Down::counts
            )),
            format!(
                "{named} , | error | matches ! (error , Down :: Busy | Down :: Away) , | quote : \
                 & Quote | quote . pending (1 , 2) , Down :: counts"
            )
        );
    }

    /// The fallback is called on `self` with the method's arguments, by the
    /// names they are bound to, then the run's error; the one attempt lends
    /// them rather than owning them, and the site says a fallback follows. The
    /// filter is an argument of the call before the one the fallback's closure
    /// is built for, so its borrow of an argument ends before that one moves.
    #[test]
    fn a_fallback_is_handed_the_arguments_by_name_then_the_runs_error() {
        let item = quote! {
            async fn rates(&self, zone: &str, weight: u32) -> Answer {
                quote(zone, weight).await
            }
        };
        let once = resilient(quote!(carrier, fallback = stale), item.clone()).to_string();
        assert_eq!(once.matches(". with_fallback ()").count(), 2, "{once}");
        for written in [
            "run :: < Answer , _ , _ , _ , _ >",
            "| _ | async { if false",
            "Fallback :: in_place_of (:: nest_rs_resilience :: __private :: fall_back (",
            ":: nest_rs_resilience :: __private :: every) , | __error | { let __handed_1 = weight \
             ; let __handed_0 = zone ; self . stale (__handed_0 , __handed_1 , __error) })) . \
             await",
        ] {
            assert!(once.contains(written), "{written} in {once}");
        }
        assert!(!once.contains("move"), "{once}");

        let again = resilient(
            quote!(carrier, idempotent, fallback = stale, fallback_if = refused),
            item,
        )
        .to_string();
        for written in [
            "run_idempotent :: < Answer , _ , _ , _ , _ >",
            ", refused) , | __error | { let __handed_1 = weight ; let __handed_0 = zone ; self . \
             stale (__handed_0 , __handed_1 , __error) })) . await",
        ] {
            assert!(again.contains(written), "{written} in {again}");
        }
    }

    /// A parameter under a `#[cfg]`, written plainly or inside a `#[cfg_attr]`,
    /// is handed to the fallback under that condition, and read under it by a
    /// method kept with no body: each build names the arguments it has.
    #[test]
    fn a_parameter_under_a_cfg_is_handed_and_read_under_it() {
        let item = quote! {
            async fn rates(
                &self,
                #[cfg(feature = "zones")] zone: &str,
                #[cfg_attr(unix, cfg(feature = "weights"), allow(unused_variables))] weight: u32,
                parcel: u32,
            ) -> Answer {
                quote(parcel).await
            }
        };
        let zones = "# [cfg (feature = \"zones\")]";
        let weights = "# [cfg_attr (unix , cfg (feature = \"weights\"))]";
        let handed = resilient(quote!(carrier, fallback = stale), item.clone()).to_string();
        let answering = format!(
            "| __error | {{ let __handed_2 = parcel ; {weights} let __handed_1 = weight ; {zones} \
             let __handed_0 = zone ; self . stale ({zones} __handed_0 , {weights} __handed_1 , \
             __handed_2 , __error) }}"
        );
        assert!(handed.contains(&answering), "{answering} in {handed}");
        let kept = kept(quote!(carrier, fallback = stale, cancel), item);
        let read = format!(
            "{{ fn __nestrs_resilient_run () {{ }} let _ = ({zones} & zone , {weights} & weight , \
             & parcel ,) ; :: core :: unreachable ! () }}"
        );
        assert!(kept.ends_with(&read), "{read} in {kept}");
    }

    /// A method refused for its receiver is checked against the stand-in under
    /// the name its `cancel` gave: a body reading its token names one bound.
    #[test]
    fn a_method_refused_for_its_receiver_keeps_its_token() {
        for receiver in [quote!(&mut self,), quote!(self,), quote!()] {
            let item = quote! {
                async fn ship(#receiver order: Order) -> Answer {
                    send(order, token).await
                }
            };
            for keys in [
                quote!(carrier, cancel = token),
                quote!(carrier, idempotent, cancel = token),
                quote!(carrier, fallback = stale, cancel = token),
            ] {
                let kept = kept(keys.clone(), item.clone());
                assert!(
                    kept.contains("refused :: < Answer , _ , _ > (move | token | async move {"),
                    "#[resilient({keys})] on `{receiver}`: {kept}"
                );
            }
            let unnamed = kept(quote!(carrier), item);
            assert!(
                unnamed.contains("refused :: < Answer , _ , _ > (move | _ | async move {"),
                "`{receiver}`: {unnamed}"
            );
        }
    }

    /// The token is the closure's parameter where the one attempt owns what it
    /// reads, and a `let` of the attempt's block where attempts borrow: ahead
    /// of the return never taken, outside the block the body was written in.
    #[test]
    fn the_token_is_bound_under_the_name_written_as_the_attempt_takes_its_arguments() {
        let item = quote! {
            async fn ship(&self, order: Order) -> Answer {
                use std::fmt::Write as _;
                send(order, token).await
            }
        };
        let once = resilient(quote!(carrier, cancel = token), item.clone()).to_string();
        assert!(
            once.contains(
                "move | token | async move { if false { return :: nest_rs_resilience :: __private \
                 :: attempt :: < Answer > () ; } ({ use std :: fmt :: Write as _ ;"
            ),
            "{once}"
        );
        for lending in [
            quote!(carrier, idempotent, cancel = token),
            quote!(carrier, cancel = token, fallback = stale),
        ] {
            let lent = resilient(lending, item.clone()).to_string();
            assert!(
                lent.contains(
                    "| __cancel | async { let token = __cancel ; if false { return :: \
                     nest_rs_resilience :: __private :: attempt :: < Answer > () ; } ({ use std :: \
                     fmt :: Write as _ ;"
                ),
                "{lent}"
            );
        }
    }

    #[test]
    fn a_key_naming_a_method_or_a_token_takes_a_name_alone() {
        for value in [
            quote!(self.stale),
            quote!(Self::stale),
            quote!(|error| Ok(0)),
            quote!("stale"),
            quote!(self),
        ] {
            let refused = refusals(quote!(carrier, fallback = #value), track());
            assert_eq!(
                refused,
                [
                    "#[resilient] `fallback` takes the name of an `async fn` method of `Self`, \
                  alone, e.g. `fallback = stale`"
                ],
                "fallback = {value}"
            );
        }
        for value in [quote!("token"), quote!(self), quote!(ctx.token), quote!(_)] {
            let refused = refusals(quote!(carrier, cancel = #value), track());
            assert_eq!(
                refused,
                [
                    "#[resilient] `cancel` takes the name the body reads its attempt's \
                  `CancellationToken` under, e.g. `cancel = token`"
                ],
                "cancel = {value}"
            );
        }
        assert!(
            resilient(quote!(carrier, fallback = r#try, cancel = r#type), track())
                .to_string()
                .contains("| __error | { self . r#try (__error) }")
        );
    }

    /// A literal is a function over nothing: refused in the key's words, at
    /// the value, the keys after it read.
    #[test]
    fn a_predicate_given_a_literal_is_refused_saying_what_its_key_takes() {
        for predicate in &PREDICATES {
            let args: TokenStream = format!(
                "carrier, idempotent, fallback = stale, {} = true, retries = 11",
                predicate.key
            )
            .parse()
            .expect("arguments");
            let refused = refusals(args, track());
            assert_eq!(
                refused,
                [
                    format!(
                        "#[resilient] `{}` takes a {}, a function's path or a closure such as `{}`",
                        predicate.key, predicate.over, predicate.example
                    ),
                    "#[resilient] `retries`: must be at most 10 — past ten, the retries let one \
                     call send its failing dependency twelve attempts or more, the hammering a \
                     breaker exists to stop"
                        .to_owned(),
                ]
            );
        }
        let read = expansion(
            quote!(
                carrier,
                record_if = pick!(counts),
                fallback = stale,
                fallback_if = ALWAYS
            ),
            track(),
        );
        assert!(
            read.is_ok(),
            "any other expression is the type checker's to judge"
        );
    }

    /// A closure's own tokens say it is no predicate where it takes no value
    /// or more than one, or is written `async`: refused at the closure, where
    /// rustc would count its arguments at the attribute, or ask for a type.
    #[test]
    fn a_predicate_closure_of_another_arity_or_written_async_is_refused_at_the_closure() {
        for predicate in &PREDICATES {
            let takes = format!(
                "#[resilient] `{}` takes a {}, a function's path or a closure such as `{}`",
                predicate.key, predicate.over, predicate.example
            );
            for (closure, why) in [
                (
                    "|| true",
                    "this closure takes no argument, and a predicate takes the one value it judges",
                ),
                (
                    "(|value, _attempt| value.counts())",
                    "this closure takes 2 arguments, and a predicate takes the one value it judges",
                ),
                (
                    "async |value| value.counts()",
                    "an `async` closure answers a future, and a predicate answers `bool` at once",
                ),
            ] {
                let args: TokenStream = format!(
                    "carrier, idempotent, fallback = stale, {} = {closure}",
                    predicate.key
                )
                .parse()
                .expect("arguments");
                assert_eq!(refusals(args, track()), [format!("{takes} — {why}")]);
            }
        }
        let read = expansion(
            quote!(carrier, record_if = |error| async { error.counts() }),
            track(),
        );
        assert!(
            read.is_ok(),
            "what a closure of one argument answers is the type checker's to judge"
        );
    }

    #[test]
    fn a_key_written_bare_is_offered_a_value_it_takes() {
        for (key, example) in [
            (RETRY_IF, "retry_if = |error| error.is_transient()"),
            (RETRY_OK_IF, "retry_ok_if = |answer| answer.is_pending()"),
            (RECORD_IF, "record_if = |error| !error.is_declined()"),
            (FALLBACK, "fallback = stale"),
            (
                FALLBACK_IF,
                "fallback_if = |error| matches!(error, ResilienceError::Open { .. })",
            ),
            (CANCEL, "cancel = token"),
        ] {
            let args: TokenStream = format!("carrier, {key}").parse().expect("arguments");
            let refused = refusals(args, track());
            let said = format!("#[resilient] `{key}` needs a value — write `{example}`");
            assert!(refused.contains(&said), "{refused:#?}");
        }
    }

    /// Under a fallback the attempt lends the arguments its fallback is handed
    /// next: a `mut` binding, a `&mut` and a pattern binding no plain name are
    /// each one refusal, in the fallback's words unless `idempotent` has its own.
    #[test]
    fn a_fallback_refuses_an_argument_it_could_not_hand_on_as_the_method_was_called_with_it() {
        let item = quote! {
            async fn ship(
                &self,
                mut order: Order,
                items: &mut Vec<u8>,
                (zone, weight): (Zone, u32),
                _: Label,
                ref depot: Depot,
            ) -> Answer {
                send(order).await
            }
        };
        let refused = refusals(quote!(carrier, fallback = stale), item.clone());
        assert_eq!(
            refused,
            [
                "#[resilient] `fallback` hands its fallback the arguments the method was called \
                 with, so none is bound `mut`: one attempt's change to `order` would reach the \
                 fallback — bind it without `mut`, and change a copy inside the body",
                "#[resilient] `fallback` hands its fallback the arguments the method was called \
                 with, so none is a `&mut`: one attempt's change through `items` would reach the \
                 fallback — take it by shared reference or by value, and change a copy inside \
                 the body",
                "#[resilient] `fallback` hands its fallback each argument under the name the \
                 method binds it to, and `(zone , weight)` binds this one to no plain name — \
                 name the argument, and take it apart inside the body",
                "#[resilient] `fallback` hands its fallback each argument under the name the \
                 method binds it to, and `_` binds this one to no plain name — name the \
                 argument, and take it apart inside the body",
                "#[resilient] `fallback` hands its fallback each argument under the name the \
                 method binds it to, and `ref depot` binds this one by reference — bind it by \
                 value, `depot`, and borrow it inside the body",
            ]
        );
        let again = refusals(quote!(carrier, idempotent, fallback = stale), item.clone());
        assert_eq!(again.len(), 5, "{again:#?}");
        assert!(
            again[0].starts_with("#[resilient] `idempotent` attempts the body again with")
                && again[1].starts_with("#[resilient] `idempotent` attempts the body again with"),
            "{again:#?}"
        );
        let alone = refusals(quote!(carrier, idempotent), item);
        assert_eq!(alone, again[..2], "`idempotent` alone takes a pattern");
    }

    #[test]
    fn a_method_is_no_fallback_of_its_own() {
        let refused = refusals(quote!(carrier, fallback = track), track());
        assert_eq!(refused.len(), 1, "{refused:#?}");
        assert!(
            refused[0].starts_with(
                "#[resilient] `fallback` names the method answering in place of this one's \
                 error, and `track` is this method itself"
            ),
            "{refused:#?}"
        );
    }

    /// A refused method is kept run once, its one attempt owning the arguments
    /// and reading its token under the name written; where the decorator read
    /// no name for a token the list writes, it is kept with no body, which may
    /// read one, each parameter read in its place under no lint attribute.
    #[test]
    fn a_refused_method_keeps_its_token_and_none_of_the_keys_it_was_refused_under() {
        let item = quote!(
            async fn ship(&self, mut order: Order) -> Answer {
                send(order, token).await
            }
        );
        let once = kept(
            quote!(
                carrier,
                fallback = stale,
                fallback_if = refused,
                record_if = counts,
                cancel = token
            ),
            item.clone(),
        );
        assert!(
            once.contains("move | token | async move { if false"),
            "{once}"
        );
        for dropped in ["fall_back", "with_fallback", "counts", "refused ,"] {
            assert!(!once.contains(dropped), "{dropped} in {once}");
        }
        for unbound in [
            quote!(carrier, retries = 3, cancel = "token"),
            quote!(carrier, retries = 3, cancel),
            quote!(carrier, timeout = "1s" "2s", cancel = token),
            quote!(cancel = token),
            quote!(self.carrier, cancel = token),
        ] {
            let kept = kept(unbound.clone(), item.clone());
            assert_eq!(
                kept,
                "async fn ship (& self , mut order : Order) -> Answer { fn __nestrs_resilient_run () { } \
                 let _ = (& mut order ,) ; :: core :: unreachable ! () }",
                "#[resilient({unbound})]"
            );
        }
    }

    /// A method kept with no body reads every name its parameters bind, a
    /// `mut` one as it is bound, and none where it binds none.
    #[test]
    fn a_method_kept_with_no_body_reads_each_name_its_parameters_bind() {
        let patterned = kept(
            quote!(carrier, cancel),
            quote! {
                #[doc = "Ships."]
                async fn ship(
                    self: &Arc<Self>,
                    Path((zone, mut weight)): Path<(Zone, u32)>,
                    ref mut depot: Depot,
                    label @ Label { ref carrier, .. }: Label,
                    [first, ..]: [u8; 4],
                    &(Parcel(inner) | Crate(inner)): &Held,
                    _: Unread,
                ) -> Answer {
                    send(zone, token).await
                }
            },
        );
        assert!(
            patterned.ends_with(
                "-> Answer { fn __nestrs_resilient_run () { } let _ = (& zone , & mut weight , & depot , \
                 & label , & carrier , & first , & inner ,) ; :: core :: unreachable ! () }"
            ),
            "{patterned}"
        );
        assert!(
            patterned.starts_with("# [doc = \"Ships.\"] async fn ship"),
            "{patterned}"
        );
        let bare = kept(quote!(carrier, cancel), track());
        assert_eq!(
            bare,
            "async fn track (& self) -> Answer { fn __nestrs_resilient_run () { } :: core :: \
             unreachable ! () }"
        );
    }

    /// The token's name is a local of the body, where an argument of that name
    /// would be read no more: refused at the name, whichever way the attempt
    /// binds it, and kept with no body, which may read either.
    #[test]
    fn a_token_named_as_an_argument_is_refused_at_its_name() {
        let item = quote! {
            async fn verify(&self, token: String, r#type: Kind) -> Answer {
                check(token).await
            }
        };
        for (args, hidden) in [
            (quote!(carrier, cancel = token), "token"),
            (quote!(carrier, idempotent, cancel = token), "token"),
            (
                quote!(carrier, fallback = stale, cancel = r#token),
                "r#token",
            ),
            (quote!(carrier, cancel = r#type), "r#type"),
        ] {
            let refused = refusals(args.clone(), item.clone());
            assert_eq!(
                refused,
                [format!(
                    "#[resilient] `cancel` names the attempt's `CancellationToken` in the body, \
                     and `{hidden}` is an argument of this method, which the body would read no \
                     more under that name — name the token otherwise, e.g. `cancel = cancellation`"
                )],
                "#[resilient({args})]: `token` is an argument's too"
            );
            let kept = kept(args.clone(), item.clone());
            assert!(
                kept.ends_with(
                    "-> Answer { fn __nestrs_resilient_run () { } let _ = (& token , & r#type ,) ; :: \
                     core :: unreachable ! () }"
                ),
                "#[resilient({args})]: {kept}"
            );
        }
        let within = refusals(
            quote!(carrier, cancel = seal),
            quote! {
                async fn verify(&self, Signed(seal): Signed) -> Answer {
                    check(seal).await
                }
            },
        );
        assert_eq!(within.len(), 1, "a name a pattern binds: {within:#?}");
        assert!(within[0].contains("`seal` is an argument"), "{within:#?}");
        let apart = expansion(quote!(carrier, cancel = cancellation), item);
        assert!(apart.is_ok(), "a name no argument has");
    }

    /// The refusal offers a name the method could write: the first of the
    /// decorator's own examples no argument has, and none once each is one's.
    #[test]
    fn a_token_named_as_an_argument_is_offered_a_name_no_argument_has() {
        let said = |arguments: TokenStream, token: TokenStream| {
            let refused = refusals(
                quote!(carrier, cancel = #token),
                quote! {
                    async fn verify(&self, #arguments) -> Answer {
                        check().await
                    }
                },
            );
            assert_eq!(refused.len(), 1, "{refused:#?}");
            let (_, remedy) = refused[0].split_once(" — ").expect("a remedy");
            remedy.to_owned()
        };
        for (arguments, token, offered) in [
            (quote!(seal: Seal), quote!(seal), "token"),
            (quote!(token: String), quote!(token), "cancellation"),
            (quote!(cancellation: String), quote!(cancellation), "token"),
            (
                quote!(r#token: String, Signed(cancellation): Signed),
                quote!(cancellation),
                "stop",
            ),
        ] {
            assert_eq!(
                said(arguments.clone(), token),
                format!("name the token otherwise, e.g. `cancel = {offered}`"),
                "({arguments})"
            );
        }
        assert_eq!(
            said(
                quote!(token: String, cancellation: String, stop: bool),
                quote!(stop)
            ),
            "name the token otherwise",
            "each example is an argument's"
        );
        for name in TOKEN_NAMES {
            let token = Ident::new(name, Span::call_site());
            assert!(
                expansion(quote!(carrier, cancel = #token), track()).is_ok(),
                "`cancel = {name}` is a name the key takes"
            );
        }
    }

    /// An answer no expansion can name has no stand-in either: a body that
    /// answers nothing would be typed `!`, an error of the decorator's own, so
    /// the method is kept as written whatever else refuses it, its arguments
    /// read first and its token bound where the decorator read a name no
    /// argument has.
    #[test]
    fn a_refused_method_answering_impl_trait_is_kept_as_written_whatever_else_refuses_it() {
        let item = quote! {
            async fn opaque(&self, token: String) -> impl Quoted {
                token
            }
        };
        let as_written = "async fn opaque (& self , token : String) -> impl Quoted { fn \
                          __nestrs_resilient_run () { } let _ = (& token ,) ; ({ token }) }";
        for args in [
            quote!(carrier),
            quote!(carrier, cancel = token),
            quote!(carrier, cancel),
            quote!(carrier, cancel = "token"),
            quote!(cancel = token),
        ] {
            let refused = refusals(args.clone(), item.clone());
            assert!(
                refused
                    .iter()
                    .any(|refused| refused.starts_with("#[resilient] writes the method's return")),
                "#[resilient({args})]: {refused:#?}"
            );
            assert_eq!(
                kept(args.clone(), item.clone()),
                as_written,
                "#[resilient({args})]"
            );
        }
        let unread = kept(
            quote!(carrier, idempotent),
            quote! {
                async fn opaque(&self, order: Order) -> impl Quoted {
                    let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
                    let _entered = __tracing_attr_span.enter();
                    order
                }
            },
        );
        assert!(
            unread.ends_with("let _entered = __tracing_attr_span . enter () ; order }) }"),
            "an instrument's body the decorator cannot read: {unread}"
        );
        for (written, kept_as) in [
            (
                quote! {
                    async fn opaque(&self, order: Order) -> impl Quoted {
                        use std::fmt::Write as _;
                        stop.cancelled().await;
                        order
                    }
                },
                "async fn opaque (& self , order : Order) -> impl Quoted { fn \
                 __nestrs_resilient_run () { } let _ = (& order ,) ; let stop = :: \
                 nest_rs_resilience :: CancellationToken :: new () ; ({ use std :: fmt :: Write as \
                 _ ; stop . cancelled () . await ; order }) }",
            ),
            (
                quote! {
                    fn boxed(&self) -> impl Quoted {
                        Box::pin(async move { stop.is_cancelled() })
                    }
                },
                "fn boxed (& self) -> impl Quoted { fn __nestrs_resilient_run () { } let stop = \
                 :: nest_rs_resilience :: CancellationToken :: new () ; ({ Box :: pin (async move { \
                 stop . is_cancelled () }) }) }",
            ),
        ] {
            assert_eq!(kept(quote!(carrier, cancel = stop), written), kept_as);
        }
    }

    /// A macro's pattern is expanded after every attribute on its method, so
    /// the names it binds are read by none: a token's name could be one of
    /// them, and a method kept with no body takes the argument under no name.
    #[test]
    fn a_pattern_a_macro_writes_is_read_by_no_decorator() {
        let item = quote! {
            async fn weigh(
                &self,
                token: String,
                pat!(zone): u32,
                Path((depot, pat!(mut weight))): Path<(Depot, u32)>,
            ) -> Answer {
                send(zone, weight).await
            }
        };
        let method: ImplItemFn = syn::parse2(item.clone()).expect("a method");
        let bound = Bound::by(&method);
        let names: Vec<_> = bound
            .names
            .iter()
            .map(|name| name.ident.to_string())
            .collect();
        assert_eq!(names, ["token", "depot"]);
        let unparsed: Vec<_> = bound
            .unparsed
            .iter()
            .map(|pat| pat.to_token_stream().to_string())
            .collect();
        assert_eq!(unparsed, ["pat ! (zone)", "pat ! (mut weight)"]);

        let refused = refusals(quote!(carrier, cancel = zone), item.clone());
        assert_eq!(
            refused,
            [
                "#[resilient] `cancel` names the attempt's `CancellationToken` in the body, where \
                 an argument of that name would be read no more, and `pat!(zone)` binds names \
                 this attribute cannot read — name the argument, and take it apart inside the body",
                "#[resilient] `cancel` names the attempt's `CancellationToken` in the body, where \
                 an argument of that name would be read no more, and `pat!(mut weight)` binds \
                 names this attribute cannot read — name the argument, and take it apart inside \
                 the body",
            ]
        );
        let stood_in = kept(quote!(carrier, cancel = zone), item.clone());
        assert!(
            stood_in.ends_with(
                "(& self , token : String , _ : u32 , _ : Path < (Depot , u32) > ,) -> Answer { fn \
                 __nestrs_resilient_run () { } let _ = (& token ,) ; :: core :: unreachable ! () }"
            ),
            "{stood_in}"
        );
        let handed = refusals(
            quote!(carrier, fallback = stale, cancel = zone),
            item.clone(),
        );
        assert_eq!(
            handed.len(),
            2,
            "the fallback's refusal says the same fix: {handed:#?}"
        );
        assert!(
            handed
                .iter()
                .all(|refused| refused.contains("binds this one to no plain name")),
            "{handed:#?}"
        );
        for unasked in [
            quote!(carrier),
            quote!(carrier, idempotent, record_if = counts),
        ] {
            let read = expansion(unasked.clone(), item.clone());
            assert!(read.is_ok(), "#[resilient({unasked})] names no token");
        }
    }

    /// A value holding commas, a closure's parameters or a call's arguments,
    /// is one value: the keys after it are read.
    #[test]
    fn a_key_after_a_predicate_is_read_past_the_commas_its_value_holds() {
        for args in [
            quote!(
                carrier,
                retry_if = |error: &Either<A, B>| error.busy(),
                idempotent
            ),
            quote!(carrier, retry_ok_if = pick::<A, B>, idempotent),
            quote!(carrier, fallback_if = one_of(a, b), fallback = stale),
        ] {
            let read = expansion(args.clone(), track());
            assert!(read.is_ok(), "#[resilient({args})]");
        }
    }

    /// `ship` as tracing-attributes 0.1.31 leaves it when its instrument is
    /// written above the decorator, whose body is `written`.
    fn instrumented_above(written: TokenStream) -> TokenStream {
        quote! {
            async fn ship(&self, order: Order) -> Answer {
                {}
                let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
                let __tracing_instrument_future = async move {
                    #[allow(unreachable_code)]
                    if false {
                        let __tracing_attr_fake_return: Answer = loop {};
                        return __tracing_attr_fake_return;
                    }
                    { #written }
                };
                if !__tracing_attr_span.is_disabled() {
                    ::tracing::Instrument::instrument(
                        __tracing_instrument_future,
                        __tracing_attr_span
                    )
                    .await
                } else {
                    __tracing_instrument_future.await
                }
            }
        }
    }

    /// The future tracing built answers the attempt's type, as the body it
    /// holds was written to, and keeps its `move` where the one attempt owns
    /// the arguments; where each attempt borrows them, it borrows them too.
    #[test]
    fn a_body_an_instrument_wrapped_answers_the_attempts_type_in_the_future_it_built() {
        let item_again = || instrumented_above(quote!(send(&order).await));
        let item = item_again();
        let attempt = "let __tracing_attr_fake_return : < Answer as :: nest_rs_resilience :: \
                       __private :: ResilientAnswer > :: Attempt = loop { } ;";
        let once = resilient(quote!(carrier), item.clone()).to_string();
        assert!(once.contains(attempt), "{once}");
        assert!(
            once.contains("let __tracing_instrument_future = async move {"),
            "{once}"
        );
        let again = resilient(quote!(carrier, idempotent), item).to_string();
        assert!(again.contains(attempt), "{again}");
        assert!(
            again.contains("let __tracing_instrument_future = async {"),
            "{again}"
        );
        assert!(
            again.contains("| _ | async { if false { return"),
            "the attempt opens on its own type, ahead of tracing's statements: {again}"
        );
        let lent = resilient(
            quote!(carrier, fallback = stale, cancel = token),
            item_again(),
        )
        .to_string();
        assert!(
            lent.contains("let __tracing_instrument_future = async {"),
            "{lent}"
        );
        assert!(
            lent.contains("| __cancel | async { let token = __cancel ; if false { return"),
            "the token is bound ahead of tracing's statements: {lent}"
        );
    }

    /// A body in a shape of tracing's the decorator does not read is one
    /// refusal, and is kept with no body: typed as the method's own answer,
    /// each answer it writes as an attempt would be an error of its own.
    #[test]
    fn a_body_an_instrument_wrapped_otherwise_is_refused_once_and_kept_with_no_body() {
        let item = quote! {
            async fn ship(&self, order: Order) -> Answer {
                let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
                let _entered = __tracing_attr_span.enter();
                send(order).await
            }
        };
        let refused = refusals(quote!(carrier, idempotent), item.clone());
        assert_eq!(refused.len(), 1, "{refused:#?}");
        assert!(
            refused[0].contains("write `#[resilient]` above `#[tracing::instrument]`"),
            "{refused:#?}"
        );
        let kept = kept(quote!(carrier, idempotent), item);
        assert!(
            kept.ends_with(
                "-> Answer { fn __nestrs_resilient_run () { } let _ = (& order ,) ; :: core :: \
                 unreachable ! () }"
            ),
            "{kept}"
        );
    }

    /// A setting the policy holds for every caller is told whose it is and
    /// where it is set, one mechanism's key each, rather than as misspelled.
    #[test]
    fn a_setting_of_the_policys_is_refused_naming_where_it_is_set() {
        for (key, set) in [
            ("backoff_ceiling", "`BACKOFF_CEILING_MS`"),
            ("breaker_open", "`BREAKER_OPEN_SECS`"),
            ("concurrency", "`CONCURRENCY`"),
            ("rate_limit", "`RATE_LIMIT`"),
        ] {
            let args: TokenStream = format!("carrier, idempotent, {key} = 1")
                .parse()
                .expect("arguments");
            let refused = refusals(
                args,
                quote!(
                    async fn track(&self) -> Answer {
                        send().await
                    }
                ),
            );
            assert_eq!(refused.len(), 1, "{refused:#?}");
            let said = &refused[0];
            assert!(
                said.starts_with(&format!("#[resilient] takes no `{key}`: ")),
                "{said}"
            );
            assert!(
                said.ends_with(&format!("{set}, or `ResilienceConfig::{key}` in code")),
                "{said}"
            );
        }
    }

    /// A setting of the policy's written where the field belongs — with a
    /// value, before the field, or alone — is refused in the words the same
    /// key earns after the field, rather than as a field the list names wrong;
    /// alone, where it may be a field of that name, it says that one is renamed.
    #[test]
    fn a_setting_of_the_policys_written_first_is_refused_as_it_is_after_the_field() {
        let item = quote!(
            async fn track(&self) -> Answer {
                send().await
            }
        );
        for key in [
            "backoff_first",
            "breaker_open",
            "concurrency_waiters",
            "rate_limit",
        ] {
            let after = refusals(
                format!("carrier, {key} = 5").parse().expect("arguments"),
                item.clone(),
            );
            for first in [
                format!("{key} = 5"),
                format!("{key} = 5, carrier"),
                format!("{key}, carrier"),
            ] {
                let refused = refusals(first.parse().expect("arguments"), item.clone());
                assert_eq!(refused, after, "#[resilient({first})]");
            }
            let alone = refusals(key.parse().expect("arguments"), item.clone());
            assert_eq!(
                alone,
                [format!(
                    "{}; a field itself named `{key}` is renamed",
                    after[0]
                )],
                "#[resilient({key})]"
            );
            assert!(
                after[0].starts_with(&format!("#[resilient] takes no `{key}`: ")),
                "{after:?}"
            );
        }
    }

    #[test]
    fn the_field_leads_the_list() {
        let item = quote!(
            async fn ship(&self) -> Answer {
                send().await
            }
        );
        let missing = refusals(quote!(), item.clone());
        assert!(
            missing[0].starts_with("#[resilient] names the field of `self`"),
            "{missing:?}"
        );
        let keyed = refusals(quote!(idempotent, carrier), item.clone());
        assert!(
            keyed[0].contains("`idempotent` is one of its keys"),
            "{keyed:?}"
        );
        let expression = refusals(quote!(self.carrier), item);
        assert!(
            expression[0].contains("by its name, alone"),
            "{expression:?}"
        );
    }

    #[test]
    fn a_refused_method_keeps_the_nearest_form_its_body_compiles_in() {
        let item = quote!(
            async fn ship(&self, mut order: Order) -> Answer {
                send(order).await
            }
        );
        let once = kept(quote!(carrier, idempotent), item.clone());
        assert!(once.contains("run :: < Answer"), "run once: {once}");
        let stood_in = kept(quote!(), item);
        assert!(
            stood_in.contains("refused :: < Answer"),
            "no field to read: {stood_in}"
        );
        assert!(
            stood_in.contains(
                "move | _ | async move { if false { return :: nest_rs_resilience :: __private :: \
                 attempt :: < Answer > () ; } ({ send (order) . await }) }"
            ),
            "the stand-in checks the body against the attempt's type: {stood_in}"
        );
        let as_written = kept(
            quote!(carrier),
            quote!(
                fn ship(&self) -> Answer {
                    send()
                }
            ),
        );
        assert!(
            !as_written.contains("nest_rs_resilience"),
            "no `async fn`: {as_written}"
        );
    }

    #[test]
    fn a_mutable_binding_is_found_wherever_the_pattern_makes_it() {
        let pat = Pat::parse_single
            .parse2(quote!((a, Wrapper { mut b, .. })))
            .expect("a pattern");
        assert_eq!(
            mutable_binding(&pat).map(|(_, name)| name.to_string()),
            Some("b".to_owned())
        );
        let pat = Pat::parse_single.parse2(quote!((a, b))).expect("a pattern");
        assert!(mutable_binding(&pat).is_none());
    }

    #[test]
    fn an_impl_trait_is_found_wherever_the_return_type_holds_it() {
        for ty in [
            quote!(impl Future),
            quote!(Result<impl Display, ResilienceError<E>>),
            quote!(Result<Vec<(u8, impl Display)>, E>),
            quote!(Box<dyn Iterator<Item = impl Display>>),
        ] {
            let mut ty: Type = syn::parse2(ty).expect("a type");
            assert!(impl_trait(&ty).is_some(), "{}", ty.to_token_stream());
            infer_impl_traits(&mut ty);
            assert!(impl_trait(&ty).is_none(), "{}", ty.to_token_stream());
        }
        let ty: Type = syn::parse2(quote!(Result<Box<dyn Display>, E>)).expect("a type");
        assert!(impl_trait(&ty).is_none());
    }

    /// A refused method drops the keys it was refused under, and an argument
    /// only one of them read would be rustc's `unused variable`, whose fix
    /// breaks the key: every kept form reads each name its parameters bind.
    #[test]
    fn a_refused_method_kept_with_its_body_reads_each_name_its_parameters_bind() {
        let arguments = quote! {
            zone: String,
            #[cfg(unix)] mut weight: u32,
            (first, ref second): (u8, u8),
            _: Unread,
        };
        let read = "let _ = (& zone , # [cfg (unix)] & weight , & first , & second ,) ;";
        let once = kept(
            quote!(carrier, record_if = |_| !zone.is_empty(), retries = 3),
            quote! {
                async fn quote(&self, #arguments) -> Answer {
                    send().await
                }
            },
        );
        assert!(
            once.contains(&format!(
                "{read} :: nest_rs_resilience :: __private :: run :: < Answer"
            )),
            "run once: {once}"
        );
        let stood_in = kept(
            quote!(carrier, fallback = stale),
            quote! {
                async fn quote(&mut self, #arguments) -> Answer {
                    send().await
                }
            },
        );
        assert!(
            stood_in.contains(&format!(
                "{read} :: nest_rs_resilience :: __private :: refused :: < Answer"
            )),
            "checked against the stand-in: {stood_in}"
        );
        let opaque = kept(
            quote!(carrier, fallback = stale, cancel = stop),
            quote! {
                async fn quote(&self, #arguments) -> impl Quoted {
                    use std::fmt::Write as _;
                    send().await
                }
            },
        );
        assert!(
            opaque.ends_with(&format!(
                "-> impl Quoted {{ fn __nestrs_resilient_run () {{ }} {read} let stop = :: \
                 nest_rs_resilience :: CancellationToken :: new () ; ({{ use std :: fmt :: Write as \
                 _ ; send () . await }}) }}"
            )),
            "as written: {opaque}"
        );
        let unread = kept(quote!(carrier, timout = "2s"), track());
        assert!(!unread.contains("let _ ="), "no argument to read: {unread}");
        let accepted = resilient(
            quote!(carrier, fallback = stale),
            quote! {
                async fn quote(&self, zone: String) -> Answer {
                    send().await
                }
            },
        )
        .to_string();
        assert!(
            !accepted.contains("let _ ="),
            "its keys read them: {accepted}"
        );
    }

    /// Under `fallback` a pattern is refused as binding no plain name, and
    /// naming the argument cures a `mut` it binds too: one refusal, one fix,
    /// whichever key has the attempts borrow.
    #[test]
    fn a_pattern_binding_mut_under_a_fallback_is_refused_once_with_the_fix_that_cures_it() {
        let item = quote! {
            async fn weigh(
                &self,
                (mut grams, unit): (u32, u32),
                ref mut tare: u32,
                ref depot: Depot,
                whole @ Parcel { mut weight, .. }: Parcel,
            ) -> Answer {
                send(grams).await
            }
        };
        for keys in [
            quote!(carrier, fallback = stale),
            quote!(carrier, idempotent, fallback = stale),
        ] {
            let refused = refusals(keys.clone(), item.clone());
            assert_eq!(
                refused,
                [
                    "#[resilient] `fallback` hands its fallback each argument under the name the \
                     method binds it to, and `(mut grams , unit)` binds this one to no plain \
                     name — name the argument, and take it apart inside the body",
                    "#[resilient] `fallback` hands its fallback each argument under the name the \
                     method binds it to, and `ref mut tare` binds this one by reference — bind \
                     it by value, `tare`, and change a copy inside the body",
                    "#[resilient] `fallback` hands its fallback each argument under the name the \
                     method binds it to, and `ref depot` binds this one by reference — bind it \
                     by value, `depot`, and borrow it inside the body",
                    "#[resilient] `fallback` hands its fallback each argument under the name the \
                     method binds it to, and `whole @ Parcel { mut weight , .. }` binds this one \
                     to no plain name — name the argument, and take it apart inside the body",
                ],
                "#[resilient({keys})]"
            );
        }
        let alone = refusals(quote!(carrier, idempotent), item);
        assert_eq!(
            alone.len(),
            3,
            "`idempotent` alone takes a pattern: {alone:#?}"
        );
        assert!(
            alone.iter().all(|refused| refused.contains("bound `mut`")),
            "{alone:#?}"
        );
    }

    /// The method's own name is read as every other name is, raw or not.
    #[test]
    fn a_method_named_raw_is_no_fallback_of_its_own() {
        for (fallback, method) in [
            (quote!(r#track), quote!(track)),
            (quote!(track), quote!(r#track)),
            (quote!(r#track), quote!(r#track)),
        ] {
            let refused = refusals(
                quote!(carrier, fallback = #fallback),
                quote! {
                    async fn #method(&self) -> Answer {
                        send().await
                    }
                },
            );
            assert_eq!(
                refused.len(),
                1,
                "fallback = {fallback} on {method}: {refused:#?}"
            );
            assert!(
                refused[0].contains("is this method itself"),
                "fallback = {fallback} on {method}: {refused:#?}"
            );
        }
    }

    /// rustc's help for a fallback whose type parameter no argument fixes
    /// writes a type argument in the key, and a call is how a method is used
    /// anywhere else: each is told what the key takes in its place.
    #[test]
    fn a_fallback_given_type_arguments_or_called_is_told_what_to_name() {
        for value in [quote!(stale::<u8>), quote!(stale::<Vec<u8>, _>)] {
            let refused = refusals(quote!(carrier, fallback = #value), track());
            assert_eq!(
                refused,
                [
                    "#[resilient] `fallback` takes the name of an `async fn` method of `Self`, \
                     alone, e.g. `fallback = stale` — it takes no type argument: a fallback is \
                     handed the method's arguments, then the error, and answers what the method \
                     answers, so name one whose type parameters those fix"
                ],
                "fallback = {value}"
            );
        }
        for value in [quote!(stale()), quote!(self.stale(zone))] {
            let refused = refusals(quote!(carrier, fallback = #value), track());
            assert_eq!(
                refused,
                [
                    "#[resilient] `fallback` takes the name of an `async fn` method of `Self`, \
                     alone, e.g. `fallback = stale` — it is named, never called: the decorator \
                     calls it with the method's arguments, then the error"
                ],
                "fallback = {value}"
            );
        }
    }

    /// rustc's help for a host answering its policy through a method writes
    /// the call in the list, and a tuple struct's field has a position for a
    /// name: each is told the decorator reads a named field, and none else.
    #[test]
    fn a_field_written_as_a_call_or_a_position_is_told_a_named_field_is_read() {
        for called in [
            quote!(carrier()),
            quote!(self.carrier()),
            quote!(carrier(), idempotent),
        ] {
            let refused = refusals(called.clone(), track());
            assert_eq!(
                refused,
                [
                    "#[resilient] names the field of `self` holding the policy by its name, \
                     alone, and this is a call: the policy is read from a field, never through a \
                     method — hold the `Arc<ResiliencePolicy<I>>` in a field of the host, and \
                     write that field's name, `#[resilient(carrier)]`"
                ],
                "#[resilient({called})]"
            );
        }
        for (position, written) in [
            (quote!(0), "0"),
            (quote!(self.1), "1"),
            (quote!(0, idempotent), "0"),
        ] {
            let refused = refusals(position.clone(), track());
            assert_eq!(
                refused,
                [format!(
                    "#[resilient] names the field of `self` holding the policy by its name, \
                     alone, and `{written}` is a tuple struct's field, which has a position and \
                     no name — hold the policy in a named field, `carrier: \
                     Arc<ResiliencePolicy<I>>`, and write `#[resilient(carrier)]`"
                )],
                "#[resilient({position})]"
            );
        }
    }

    /// A trait's declaration is a method with no body: told what it is and
    /// where the decorator goes, at its name, and kept as written.
    #[test]
    fn a_method_a_trait_declares_is_told_the_decorator_goes_on_each_impl() {
        let declared = quote!(
            async fn status(&self, parcel: u32) -> Answer;
        );
        let refused = refusals(quote!(carrier, idempotent), declared.clone());
        assert_eq!(
            refused,
            [
                "#[resilient] decorates an `async fn` method with a body, run under the policy a \
                 field of `self` holds, and `status` is declared with none, as a trait declares \
                 a method: there is no body to run, and no field of `self` to read — put \
                 `#[resilient]` on the method of each impl"
            ]
        );
        assert_eq!(
            kept(quote!(carrier, idempotent), declared.clone()),
            declared.to_string()
        );
        let no_method = refusals(
            quote!(carrier),
            quote!(
                struct Shipments;
            ),
        );
        assert!(
            no_method[0].ends_with("this is no method — put it on the method making the call"),
            "{no_method:#?}"
        );
    }

    /// A decorated method's body opens on the expansion's own mark, so a
    /// second decorator `check` could not read by its name — imported under
    /// another, or written in a `#[cfg_attr]` — finds a body already run under
    /// a policy and says so, alone, the method kept as the first left it.
    #[test]
    fn a_second_decorator_read_by_no_name_finds_the_first_ones_run_and_is_refused() {
        let written = quote! {
            async fn ship(&self, order: Order) -> Answer {
                send(order).await
            }
        };
        let twice = |refused: Vec<String>| {
            assert_eq!(refused.len(), 1, "{refused:#?}");
            assert!(
                refused[0].starts_with("#[resilient] is written twice here"),
                "{refused:#?}"
            );
        };
        for first in [
            quote!(carrier),
            quote!(carrier, idempotent, cancel = token),
            quote!(carrier, fallback = stale, fallback_if = refused),
        ] {
            let run = resilient(first.clone(), written.clone());
            assert!(
                run.to_string()
                    .contains("-> Answer { fn __nestrs_resilient_run () { }"),
                "#[resilient({first})]: {run}"
            );
            for second in [quote!(carrier, idempotent), quote!(carrier, retry = 3)] {
                twice(refusals(second.clone(), run.clone()));
                assert_eq!(kept(second, run.clone()), run.to_string());
            }
            // An instrument between the two wraps the mark with the run.
            let body: syn::ImplItemFn = syn::parse2(run).expect("a method");
            let stmts = &body.block.stmts;
            twice(refusals(
                quote!(carrier),
                instrumented_above(quote!(#(#stmts)*)),
            ));
        }
        // A first decorator refused keeps the method in a form the second finds too.
        for first in [
            quote!(carrier, retries = 3),
            quote!(carier(), idempotent),
            quote!(carrier, cancel),
        ] {
            let kept_run: TokenStream = kept(first.clone(), written.clone())
                .parse()
                .expect("a method");
            twice(refusals(quote!(carrier), kept_run));
        }
        // A body that only names the mark is the developer's own.
        let named = quote! {
            async fn ship(&self, order: Order) -> Answer {
                let __nestrs_resilient_run = order;
                send(__nestrs_resilient_run).await
            }
        };
        assert!(expansion(quote!(carrier), named).is_ok());
    }

    /// A key whose `=` nothing follows is the key written bare, and a value
    /// that reads as no expression is refused in the key's words: `syn`'s own
    /// name neither the decorator nor the key.
    #[test]
    fn a_value_that_reads_as_no_expression_is_refused_in_its_keys_words() {
        for (args, said) in [
            (
                quote!(carrier, idempotent, retries =),
                "#[resilient] `retries` needs a value — write `retries = 3`",
            ),
            (
                quote!(carrier, fallback = , idempotent),
                "#[resilient] `fallback` needs a value — write `fallback = stale`",
            ),
            (
                quote!(carrier, idempotent, retries = =),
                "#[resilient] `retries` takes an expression after its `=`, and this reads as none",
            ),
            (
                quote!(carrier, record_if = |),
                "#[resilient] `record_if` takes an expression after its `=`, and this reads as \
                 none",
            ),
            (
                quote!(carrier, cancel = ;),
                "#[resilient] `cancel` takes an expression after its `=`, and this reads as none",
            ),
        ] {
            let refused = refusals(args.clone(), track());
            assert_eq!(refused, [said], "#[resilient({args})]");
        }
    }

    /// A `fn` whose body awaits at its own level is an `async fn` missing its
    /// `async`, and is kept as one, so neither its body nor a caller awaiting
    /// it adds an error. Any other is kept with no body: a boxed future's,
    /// as `#[async_trait]` leaves one, answers the method's own type where the
    /// developer wrote an attempt's.
    #[test]
    fn a_refused_fn_is_kept_as_the_async_fn_its_body_awaits_as_or_with_no_body() {
        let forgot = kept(
            quote!(carrier, fallback = stale),
            quote! {
                fn ship(&self, order: Order) -> Answer {
                    let sent = send(&order).await?;
                    Ok(sent)
                }
            },
        );
        assert!(
            forgot.starts_with(
                "async fn ship (& self , order : Order) -> Answer { fn __nestrs_resilient_run () \
                 { } let _ = (& order ,) ; :: nest_rs_resilience :: __private :: run :: < Answer"
            ),
            "{forgot}"
        );
        let hostless = kept(
            quote!(carrier),
            quote! {
                fn ship(order: Order) -> Answer {
                    send(order).await
                }
            },
        );
        assert!(
            hostless.starts_with("async fn ship (order : Order) -> Answer {")
                && hostless.contains("__private :: refused :: < Answer"),
            "{hostless}"
        );
        for body in [
            quote!(Box::pin(async move {
                let sent = send(&order).await?;
                Ok(sent)
            })),
            quote!(Err(Down)),
            quote! {
                async fn sent(order: &Order) -> Answer {
                    send(order).await
                }
                block_on(sent(&order))
            },
            quote!(block_on(async { send(&order).await })),
            quote!(traced!(send(&order).await)),
        ] {
            let kept = kept(
                quote!(carrier, cancel = token),
                quote! {
                    fn ship(&self, order: Order) -> Answer {
                        #body
                    }
                },
            );
            assert_eq!(
                kept,
                "fn ship (& self , order : Order) -> Answer { fn __nestrs_resilient_run () { } let _ = \
                 (& order ,) ; :: core :: unreachable ! () }",
                "{body}"
            );
        }
        let constant = kept(
            quote!(carrier),
            quote! {
                const fn ship(&self) -> Answer {
                    send().await
                }
            },
        );
        assert!(constant.starts_with("const fn ship"), "{constant}");
    }

    /// An `impl Trait` inside the answer leaves the rest of it written: the
    /// body is checked as an attempt of that answer, `_` standing for what it
    /// alone names, so an `Err(e)` or a `?` it writes adds no error of rustc's.
    /// An answer that is an `impl Trait` whole says nothing of its attempt.
    #[test]
    fn a_refused_method_answering_impl_trait_inside_its_answer_is_checked_as_an_attempt() {
        let inside = kept(
            quote!(carrier, idempotent, cancel = stop),
            quote! {
                async fn label(&self, order: Order) -> Result<impl Display, ResilienceError<Down>> {
                    let sent = send(&order, stop).await?;
                    Ok(sent)
                }
            },
        );
        for written in [
            "-> Result < impl Display , ResilienceError < Down > > { fn __nestrs_resilient_run () { } \
             let _ = (& order ,) ; :: nest_rs_resilience :: __private :: refused :: < Result < _ , \
             ResilienceError < Down > > , _ , _ > (move | stop | async move {",
            "if false { return :: nest_rs_resilience :: __private :: attempt :: < Result < _ , \
             ResilienceError < Down > > > () ; }",
        ] {
            assert!(inside.contains(written), "{written} in {inside}");
        }
        for (answer, inferred) in [
            (
                quote!(Result<Vec<(u8, impl Display)>, ResilienceError<impl Error>>),
                "Result < Vec < (u8 , _) > , ResilienceError < _ > >",
            ),
            (
                quote!(Result<Box<dyn Iterator<Item = impl Display>>, E>),
                "Result < Box < dyn Iterator < Item = _ > > , E >",
            ),
            (
                quote!(Result<&(impl Display), <impl Tr as Named>::Error>),
                "Result < & (_) , < _ as Named > :: Error >",
            ),
        ] {
            let kept = kept(
                quote!(carrier),
                quote! {
                    async fn label(&self) -> #answer {
                        send().await
                    }
                },
            );
            assert!(
                kept.contains(&format!("refused :: < {inferred} , _ , _ >")),
                "{inferred} in {kept}"
            );
        }
        for whole in [quote!(impl Quoted), quote!((impl Quoted))] {
            let kept = kept(
                quote!(carrier),
                quote! {
                    async fn label(&self) -> #whole {
                        send().await
                    }
                },
            );
            assert!(
                kept.ends_with("{ fn __nestrs_resilient_run () { } ({ send () . await }) }"),
                "as written: {kept}"
            );
        }
    }

    /// An item the body declares is in scope of its whole block, a name of
    /// the method's answer included: whatever the expansion writes for itself
    /// — the attempt's type, the token's `let`, a kept form's mark and reads —
    /// stands outside the block the body was written in, kept whole.
    #[test]
    fn nothing_of_the_expansions_is_written_inside_the_block_the_body_was_written_in() {
        let stmts = "use std :: num :: ParseIntError as Error ; let n = parse (zone) . map_err (| \
                     _ : Error | Down) ? ; Ok (n)";
        let written = format!("{{ {stmts} }}");
        let body: TokenStream = written.parse().expect("a block");
        let item = quote! {
            async fn rates(&self, zone: &str) -> Result<u32, ResilienceError<Error>> #body
        };
        let attempt = ":: nest_rs_resilience :: __private :: attempt :: < Result < u32 , \
                       ResilienceError < Error > > > ()";
        for (keys, opens) in [
            (quote!(carrier), "move | _ | async move {".to_owned()),
            (quote!(carrier, idempotent), "| _ | async {".to_owned()),
            (
                quote!(carrier, fallback = stale),
                "| _ | async {".to_owned(),
            ),
            (
                quote!(carrier, cancel = token),
                "move | token | async move {".to_owned(),
            ),
            (
                quote!(carrier, idempotent, cancel = token),
                "| __cancel | async { let token = __cancel ;".to_owned(),
            ),
        ] {
            let expanded = resilient(keys.clone(), item.clone()).to_string();
            let attempted = format!("{opens} if false {{ return {attempt} ; }} ({written}) }}");
            assert!(
                expanded.contains(&attempted),
                "#[resilient({keys})]: {attempted} in {expanded}"
            );
        }
        let stood_in = kept(quote!(), item.clone());
        assert!(
            stood_in.contains(&format!(
                "move | _ | async move {{ if false {{ return {attempt} ; }} ({written}) }}"
            )),
            "{stood_in}"
        );
        let wrapped = resilient(
            quote!(carrier, idempotent),
            instrumented_above(stmts.parse().expect("statements")),
        )
        .to_string();
        assert!(
            wrapped.contains("() ; } ({ { } let __tracing_attr_span")
                && wrapped.contains(&format!(
                    "__tracing_attr_fake_return ; }} {written} }} ; if !"
                )),
            "the body an instrument wrapped is as tracing left it, inside the attempt's: {wrapped}"
        );
        let as_written = kept(
            quote!(carrier, cancel = stop),
            quote! {
                async fn rates(&self, zone: &str) -> impl Quoted #body
            },
        );
        assert!(
            as_written.ends_with(&format!(
                "{{ fn __nestrs_resilient_run () {{ }} let _ = (& zone ,) ; let stop = :: \
                 nest_rs_resilience :: CancellationToken :: new () ; ({written}) }}"
            )),
            "{as_written}"
        );
    }

    /// A field followed by a key with no comma between them is told the comma,
    /// what was written kept in the list it is shown.
    #[test]
    fn a_field_its_keys_follow_with_no_comma_is_told_the_comma_is_missing() {
        for (list, remedy) in [
            (quote!(carrier idempotent), "carrier, idempotent"),
            (
                quote!(carrier timeout = "2s", idempotent),
                "carrier, timeout = \"2s\" , idempotent",
            ),
            (quote!(fallback idempotent), "fallback, idempotent"),
        ] {
            let refused = refusals(list.clone(), track());
            let field = list.clone().into_iter().next().expect("a field");
            assert_eq!(
                refused,
                [format!(
                    "#[resilient] reads the field of `self` holding the policy, then each key \
                     after a comma, and the comma after `{field}` is missing — write \
                     `#[resilient({remedy})]`"
                )],
                "#[resilient({list})]"
            );
        }
    }

    /// The list's first entry is the field, whatever its name: one named like
    /// a key that takes a value, written with none, is no key written right,
    /// so it is the field it reads as.
    #[test]
    fn a_field_named_like_a_key_is_read_as_the_field_it_is_written_as() {
        for (list, field) in [
            (quote!(fallback), "fallback"),
            (quote!(r#fallback), "r#fallback"),
            (quote!(timeout, idempotent), "timeout"),
            (quote!(fallback, fallback = stale), "fallback"),
            (
                quote!(cancel, cancel = token, retries = 2, idempotent),
                "cancel",
            ),
            (quote!(idempotent, idempotent), "idempotent"),
            (quote!(idempotent, timeout = "2s", idempotent), "idempotent"),
        ] {
            let expanded = expansion(list.clone(), track())
                .unwrap_or_else(|_| panic!("#[resilient({list})] refused"))
                .to_string();
            assert!(
                expanded.contains(&format!("policy (& self . {field})")),
                "#[resilient({list})]: {expanded}"
            );
        }
        let sent_again = resilient(quote!(idempotent, idempotent), track()).to_string();
        assert!(sent_again.contains("run_idempotent"), "{sent_again}");
    }

    /// A first entry named like a key is that key, its field missing, where
    /// the list says so — it is given a value, or a name that is no key
    /// follows it — and where nothing tells the two apart, `idempotent` or a
    /// setting of the policy's written alone, the refusal says how to write
    /// either.
    #[test]
    fn a_key_ahead_of_its_field_is_told_apart_from_a_field_named_like_it() {
        let ahead = |key: &str| {
            format!(
                "#[resilient] reads the field holding the policy first, and `{key}` is one of its \
                 keys — write the field before it: `#[resilient(carrier, {key})]`"
            )
        };
        for (list, key) in [
            (quote!(fallback = stale), "fallback"),
            (quote!(timeout = "2s", carrier), "timeout"),
            (quote!(idempotent(true)), "idempotent"),
            (quote!(idempotent, carrier), "idempotent"),
            (quote!(fallback, carrier), "fallback"),
            (quote!(retries, idempotent, carrier), "retries"),
        ] {
            assert_eq!(
                refusals(list.clone(), track()),
                [ahead(key)],
                "#[resilient({list})]"
            );
        }
        for list in [quote!(idempotent), quote!(idempotent, timeout = "2s")] {
            assert_eq!(
                refusals(list.clone(), track()),
                [format!(
                    "{}; a field itself named `idempotent` is named before the key, \
                     `#[resilient(idempotent, idempotent)]`, or renamed",
                    ahead("idempotent")
                )],
                "#[resilient({list})]"
            );
        }
        for list in [quote!(concurrency), quote!(concurrency, idempotent)] {
            let refused = refusals(list.clone(), track());
            assert_eq!(refused.len(), 1, "#[resilient({list})]: {refused:#?}");
            assert!(
                refused[0].starts_with("#[resilient] takes no `concurrency`: ")
                    && refused[0].ends_with(
                        "`ResilienceConfig::concurrency` in code; a field itself named \
                         `concurrency` is renamed"
                    ),
                "#[resilient({list})]: {refused:#?}"
            );
        }
    }

    /// A function taking no receiver may be a free one, which no receiver
    /// cures, and a `const fn` is never `async`: each is told what can be done.
    #[test]
    fn a_free_fn_and_a_const_fn_are_each_told_a_remedy_they_can_follow() {
        let free = refusals(
            quote!(carrier),
            quote!(
                pub async fn free() -> Answer {
                    Ok(())
                }
            ),
        );
        assert_eq!(
            free,
            [
                "a `#[resilient]` method borrows its host — `&self`, or `self: &Arc<Self>` — and \
              this one takes no receiver: a host is one instance shared by every call, and each \
              method is lent a shared borrow of it. A free function has no host to hold a policy \
              in a field: the method making the call is decorated, a provider's or that of a \
              struct built by hand"
            ]
        );
        let constant = refusals(
            quote!(carrier),
            quote!(
                const fn ship(&self) -> Answer {
                    Ok(())
                }
            ),
        );
        assert_eq!(
            constant,
            [
                "#[resilient] decorates an `async fn`: the policy's run is awaited, and this \
              method is a `const fn`, which is never `async` — write `async fn`, without `const`"
            ]
        );
    }
}
