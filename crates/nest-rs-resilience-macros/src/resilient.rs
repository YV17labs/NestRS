//! `#[resilient(field, …)]`: an `async fn` method's body run under the policy a
//! field of `self` holds — once or again, each answer counted as the method's
//! predicates say, its attempt's token bound under the name the method gives
//! it, and the error its run ends on answered by the fallback it names.

use nest_rs_codegen::__private::{
    HostBorrow, RESILIENT, RETRIES, TIMEOUT, answer_span, is_resilient, mixed_site_ident,
    needs_a_value, opens_on_operation_field_mark, resilient_policy_setting, resilient_retries,
    resilient_timeout, shared_receiver, site, takes_value, ungrouped_expr,
};
use nest_rs_codegen::{Arg, Grammar, Refusals};
use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::discouraged::Speculative;
use syn::parse::{ParseStream, Parser};
use syn::spanned::Spanned;
use syn::{
    Expr, FnArg, GenericArgument, Ident, ImplItemFn, Pat, PatIdent, PatType, PathArguments,
    ReturnType, Stmt, Token, Type, TypeParamBound,
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
            ..self.once()
        }
    }

    /// The run a refused method is kept as: once, its one attempt owning the
    /// arguments, under the field and the token's name alone.
    fn once(&self) -> Plan<'_> {
        Plan {
            field: &self.field,
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
    /// `idempotent` as written, when the call is sent again after a failure:
    /// kept for its span.
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
        refusals.push(syn::Error::new(
            Span::call_site(),
            "#[resilient] decorates an `async fn` method with a body, run under the policy a \
             field of `self` holds, and this is no method — put it on the method making the \
             call",
        ));
        return Err(refused(item, refusals));
    };
    let mut on_a_generated_field = false;
    let read = match method.sig.asyncness {
        Some(_) => instrumented(&mut method.block.stmts, &mut |part| {
            if let Part::Written(stmts) = part {
                on_a_generated_field = opens_on_operation_field_mark(stmts);
            }
        }),
        None => Ok(()),
    };
    if on_a_generated_field {
        return Err(Refused {
            kept: item,
            refusal: syn::Error::new(
                Span::call_site(),
                "#[resilient] reached the field `#[operations]` generates for this operation, not \
                 the method written: `#[operations]` reads the decorator by its name — import it \
                 as `resilient`, and it stays on the method, whose `self` holds the policy",
            ),
        });
    }
    let unread = refusals.handle(read).is_none();
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

/// A refused method, in the nearest form its body compiles in as written —
/// as if it answered `Result<T, E>` — so that no error but its refusals is
/// reported: as written where it answers an `impl Trait`, which its body alone
/// names, whatever else refused it; with no body where the one it has is an
/// instrument's the decorator cannot type as an attempt, or may read a token
/// whose name the decorator did not bind, its parameters read in that body's
/// place; as written where it is no `async fn`; run once where the method
/// could be expanded at all; and checked against a stand-in that never runs
/// where its receiver or its field is what was refused.
fn kept(mut method: ImplItemFn, args: Option<&Args>, shape: &Shape) -> TokenStream {
    if !shape.opaque && (shape.unread || shape.unbound) {
        no_body(&mut method);
        return method.into_token_stream();
    }
    if !shape.asynchronous || shape.opaque {
        let token = args.and_then(|args| args.cancel.as_ref());
        if let Some(token) = token.filter(|_| !shape.unbound) {
            let stmts = &mut method.block.stmts;
            stmts.insert(
                leading_items(stmts),
                syn::parse_quote!(let #token = ::nest_rs_resilience::CancellationToken::new();),
            );
        }
        return method.into_token_stream();
    }
    match (args, shape.receiver) {
        (Some(args), Some(receiver)) => expand(method, receiver, &args.once()),
        _ => stand_in(method),
    }
}

/// How many items open `stmts`, ahead of its first statement.
fn leading_items(stmts: &[Stmt]) -> usize {
    stmts
        .iter()
        .take_while(|stmt| matches!(stmt, Stmt::Item(_)))
        .count()
}

/// Leaves `method` no body: each name its parameters bind, read as it is
/// bound, then an answer never given. The body that read them is gone, and a
/// lint attribute in its place is `E0453` in a crate forbidding the lint. A
/// parameter binding names no attribute reads is taken under none, `_`.
fn no_body(method: &mut ImplItemFn) {
    let mut read = Vec::new();
    for input in &mut method.sig.inputs {
        let FnArg::Typed(typed) = input else {
            continue;
        };
        let mut bound = Bound::default();
        bound.read(&typed.pat);
        if bound.unparsed.is_empty() {
            read.extend(bound.names.iter().map(|binding| {
                let name = &binding.ident;
                match (&binding.by_ref, &binding.mutability) {
                    (None, Some(_)) => quote!(&mut #name),
                    _ => quote!(&#name),
                }
            }));
        } else {
            *typed.pat = Pat::Wild(syn::PatWild {
                attrs: Vec::new(),
                underscore_token: Token![_](typed.pat.span()),
            });
        }
    }
    let mut stmts = Vec::new();
    if !read.is_empty() {
        stmts.push(syn::parse_quote!(let _ = (#(#read,)*);));
    }
    stmts.push(Stmt::Expr(
        Expr::Verbatim(quote!(::core::unreachable!())),
        None,
    ));
    method.block.stmts = stmts;
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
                                 presence: the call is sent again after a failure or a timeout",
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
                FALLBACK => {
                    args.fallback = refusals.handle(name(
                        &value(&arg)?,
                        FALLBACK,
                        "the name of an `async fn` method of `Self`, alone, e.g. `fallback = stale`",
                    ));
                }
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
/// and [`resumes`] reads on from there.
fn value(arg: &Arg<'_>) -> syn::Result<Expr> {
    let input = arg.value()?;
    let ahead = input.fork();
    let value = ahead.parse()?;
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

/// `arg`'s value as the predicate its key takes: any expression but a
/// literal, which is no function whatever its key — refused here, in
/// `refusals`, where rustc would follow the key's sentence with one naming the
/// expansion. A value that reads as no expression is the error handed back.
fn predicate(arg: &Arg<'_>, refusals: &mut Refusals) -> syn::Result<Option<Expr>> {
    let value = value(arg)?;
    let row = PREDICATES.iter().find(|row| row.key == arg.key());
    let (Expr::Lit(literal), Some(row)) = (ungrouped_expr(&value), row) else {
        return Ok(Some(value));
    };
    let what = format!(
        "a {}, a function's path or a closure such as `{}`",
        row.over, row.example
    );
    refusals.push(syn::Error::new_spanned(
        literal,
        takes_value(RESILIENT, Some(row.key), &what),
    ));
    Ok(None)
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

/// The field, named first and alone — refused when what leads the list is a
/// key, the decorator's own or a setting of the policy's, told where that one
/// is set as it is after the field, or anything but a field's name.
fn field(input: ParseStream<'_>) -> syn::Result<Ident> {
    if let Ok(leading) = input.fork().call(Ident::parse_any) {
        let key = leading.unraw().to_string();
        if KEYS.contains(&key.as_str()) {
            return Err(syn::Error::new(
                leading.span(),
                format!(
                    "#[resilient] reads the field holding the policy first, and `{leading}` is \
                     one of its keys — write the field before it: `#[resilient(carrier, \
                     {leading})]`",
                ),
            ));
        }
        if let Some(elsewhere) = resilient_policy_setting(&key) {
            return Err(syn::Error::new(leading.span(), elsewhere));
        }
    }
    let fork = input.fork();
    match fork.parse::<Ident>() {
        Ok(_) if fork.is_empty() || fork.peek(Token![,]) => input.parse(),
        _ => Err(syn::Error::new(
            input.span(),
            "#[resilient] names the field of `self` holding the policy by its name, alone — \
             write `#[resilient(carrier)]` to run the method under `self.carrier`",
        )),
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
        refusals.push(syn::Error::new_spanned(
            second,
            "#[resilient] is written twice here — a method runs under one policy, and a second \
             would run the first's whole run again at each of its attempts. Keep one; a method \
             calling two dependencies is two methods, each decorated",
        ));
    }
    let asynchronous = method.sig.asyncness.is_some();
    if !asynchronous {
        refusals.push(syn::Error::new_spanned(
            method.sig.fn_token,
            "#[resilient] decorates an `async fn`: the policy's run is awaited, and this method \
             is a `fn` — write `async fn`. Under `#[async_trait]`, which turns each `async fn` of \
             the impl into a `fn` returning a boxed future before this attribute reads it, write \
             the trait's method as a native `async fn` instead",
        ));
    }
    let receiver = refusals
        .handle(shared_receiver(
            method,
            "#[resilient]",
            &Ident::new("Self", Span::call_site()),
            HostBorrow::Arc,
        ))
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
    if let Some(key) = fallback
        && args.is_some_and(|args| args.fallback.as_ref() == Some(&method.sig.ident))
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
        if let Some(lent) = &lent {
            if let Some((binding, written)) = mutable_binding(&typed.pat) {
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
        if fallback.is_some() && plain_name(&typed.pat).is_none() {
            refusals.push(syn::Error::new_spanned(
                &typed.pat,
                format!(
                    "{} hands its fallback each argument under the name the method binds it to, \
                     and `{}` binds this one to no plain name — name the argument, and take it \
                     apart inside the body",
                    site(RESILIENT, Some(FALLBACK)),
                    typed.pat.to_token_stream(),
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

/// What the method answers, as the expansion writes it — every token at the
/// return type's span — and that span.
///
/// Every helper the expansion calls is bounded on the answer there, so a
/// refused answer is one error: rustc reports one predicate once per span, and
/// a stable macro can give it no wider span than the type's first token.
fn answer(method: &ImplItemFn) -> (TokenStream, Span) {
    let at = answer_span(&method.sig);
    let answer = match &method.sig.output {
        ReturnType::Type(_, ty) => respanned(ty.to_token_stream(), at),
        ReturnType::Default => quote_spanned!(at=> ()),
    };
    (answer, at)
}

/// `method`'s body, taken out of it and braced as it was written, opening after
/// its items on a `return` of the attempt's type never taken: rustc types an
/// async block's output from its body, so without it no answer is coerced.
///
/// A body `#[tracing::instrument]` wrapped answers the attempt's type inside
/// the future it built too, and where each attempt `borrows` the arguments,
/// that future borrows them as the body it holds would. `bound`, the `let`
/// naming the attempt's token where the closure's parameter cannot, leads the
/// statements.
fn body(
    method: &mut ImplItemFn,
    answer: &TokenStream,
    at: Span,
    borrows: bool,
    bound: Option<Stmt>,
) -> syn::Block {
    let mut stmts = std::mem::take(&mut method.block.stmts);
    // `expansion` refused a shape this stops at, and `kept` gave it no body to type.
    instrumented(&mut stmts, &mut |part| match part {
        Part::Answer(ty) => {
            *ty = Type::Verbatim(quote_spanned! {at=>
                <#answer as ::nest_rs_resilience::__private::ResilientAnswer>::Attempt
            });
        }
        Part::Future(future) if borrows => future.capture = None,
        Part::Future(_) | Part::Written(_) => {}
    })
    .unwrap_or_default();
    let items = leading_items(&stmts);
    stmts.insert(
        items,
        syn::Stmt::Expr(
            Expr::Verbatim(quote_spanned! {at=>
                if false {
                    return ::nest_rs_resilience::__private::attempt::<#answer>();
                }
            }),
            None,
        ),
    );
    if let Some(bound) = bound {
        stmts.insert(items, bound);
    }
    syn::Block {
        brace_token: method.block.brace_token,
        stmts,
    }
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
        quote_spanned!(field.span()=>
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
    let block = body(&mut method, &answer, at, lends.is_some(), bound);
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
    // Spanned at the key that has every attempt borrow, where rustc's own words for a
    // body consuming an argument then point, and for a future not `Send` under `idempotent`.
    let attempt = match lends {
        Some(key) => quote_spanned!(key.span()=> |#token| async #block),
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
            let names = handed.iter().map(|handed| &handed.name);
            let under: Vec<_> = handed.iter().map(|handed| &handed.under).collect();
            let answering = quote_spanned!(method.span()=> |#error| {
                #(let #under = #names;)*
                #receiver.#method(#(#under,)* #error)
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
    method.block.stmts = vec![syn::Stmt::Expr(Expr::Verbatim(call), None)];
    method.into_token_stream()
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
}

/// Each argument of `method` as its fallback is handed it, in order, located
/// `at` the key that hands it on.
fn arguments(method: &ImplItemFn, at: Span) -> Vec<Handed> {
    typed_inputs(method)
        .filter_map(|typed| plain_name(&typed.pat))
        .enumerate()
        .map(|(index, bound)| {
            let mut name = bound.clone();
            name.set_span(bound.span().located_at(at));
            let under = Ident::new(
                &format!("__handed_{index}"),
                Span::mixed_site().located_at(at),
            );
            Handed { name, under }
        })
        .collect()
}

/// `method` with its body checked as one attempt against a stand-in that
/// reads no field and never runs.
fn stand_in(mut method: ImplItemFn) -> TokenStream {
    let (answer, at) = answer(&method);
    let block = body(&mut method, &answer, at, false, None);
    let checked = quote_spanned! {at=>
        ::nest_rs_resilience::__private::refused::<#answer, _, _>(move |_| async move #block)
    };
    method.block.stmts = vec![syn::Stmt::Expr(Expr::Verbatim(checked), None)];
    method.into_token_stream()
}

/// `Some(value)` or `None`, as the expansion writes it.
fn optional(value: Option<impl ToTokens>) -> TokenStream {
    match value {
        Some(value) => quote!(::core::option::Option::Some(#value)),
        None => quote!(::core::option::Option::None),
    }
}

/// `tokens`, every one at `span`.
fn respanned(tokens: TokenStream, span: Span) -> TokenStream {
    tokens
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Group(group) => {
                let mut moved = Group::new(group.delimiter(), respanned(group.stream(), span));
                moved.set_span(span);
                TokenTree::Group(moved)
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
             Result < u32 , ResilienceError < E > > > () ; } quote (zone) . await }",
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
                 attempt :: < Result < () , ResilienceError < E > > > () ; } send (order) . await }"
            ),
            "{expanded}"
        );
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

    /// The attempt's type is the body's first answer, after the items it
    /// declares first and before every statement, so rustc checks each answer
    /// the body writes against it and no item follows a statement.
    #[test]
    fn the_body_answers_the_attempts_type_first_after_its_leading_items() {
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
                "async move { use std :: fmt :: Write as _ ; const LABEL : & str = \"parcel\" ; \
                 if false { return :: nest_rs_resilience :: __private :: attempt :: < Answer > () \
                 ; } let label = LABEL ;"
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
            ":: nest_rs_resilience :: __private :: every) , | __error | { let __handed_0 = zone \
             ; let __handed_1 = weight ; self . stale (__handed_0 , __handed_1 , __error) })) . \
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
            ", refused) , | __error | { let __handed_0 = zone ; let __handed_1 = weight ; self . \
             stale (__handed_0 , __handed_1 , __error) })) . await",
        ] {
            assert!(again.contains(written), "{written} in {again}");
        }
    }

    /// The token is the closure's parameter where the one attempt owns what it
    /// reads, and a `let` of the attempt's block where attempts borrow: after
    /// the body's leading items, ahead of the return never taken.
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
            once.contains("move | token | async move { use std :: fmt :: Write as _ ; if false"),
            "{once}"
        );
        for lending in [
            quote!(carrier, idempotent, cancel = token),
            quote!(carrier, cancel = token, fallback = stale),
        ] {
            let lent = resilient(lending, item.clone()).to_string();
            assert!(
                lent.contains(
                    "| __cancel | async { use std :: fmt :: Write as _ ; let token = __cancel ; \
                     if false"
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
            quote!(stale()),
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
                 method binds it to, and `ref depot` binds this one to no plain name — name the \
                 argument, and take it apart inside the body",
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
                "async fn ship (& self , mut order : Order) -> Answer { let _ = (& mut order ,) ; \
                 :: core :: unreachable ! () }",
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
                "-> Answer { let _ = (& zone , & mut weight , & depot , & label , & carrier , & \
                 first , & inner ,) ; :: core :: unreachable ! () }"
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
            "async fn track (& self) -> Answer { :: core :: unreachable ! () }"
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
                    "-> Answer { let _ = (& token , & r#type ,) ; :: core :: unreachable ! () }"
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
    /// the method is kept as written whatever else refuses it, its token bound
    /// where the decorator read a name no argument has.
    #[test]
    fn a_refused_method_answering_impl_trait_is_kept_as_written_whatever_else_refuses_it() {
        let item = quote! {
            async fn opaque(&self, token: String) -> impl Quoted {
                token
            }
        };
        let as_written = "async fn opaque (& self , token : String) -> impl Quoted { token }";
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
            unread.ends_with("let _entered = __tracing_attr_span . enter () ; order }"),
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
                "async fn opaque (& self , order : Order) -> impl Quoted { use std :: fmt :: \
                 Write as _ ; let stop = :: nest_rs_resilience :: CancellationToken :: new () ; \
                 stop . cancelled () . await ; order }",
            ),
            (
                quote! {
                    fn boxed(&self) -> Answer {
                        Box::pin(async move { stop.is_cancelled() })
                    }
                },
                "fn boxed (& self) -> Answer { let stop = :: nest_rs_resilience :: \
                 CancellationToken :: new () ; Box :: pin (async move { stop . is_cancelled () \
                 }) }",
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
                "(& self , token : String , _ : u32 , _ : Path < (Depot , u32) > ,) -> Answer { \
                 let _ = (& token ,) ; :: core :: unreachable ! () }"
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
                retry_if = |error, _unused| error.busy(),
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
            kept.ends_with("-> Answer { let _ = (& order ,) ; :: core :: unreachable ! () }"),
            "{kept}"
        );
    }

    /// Handed to the field `#[operations]` generates, the decorator says so and
    /// nothing else: what it would refuse of that field — its arguments, its
    /// receiver's type — is not the developer's to fix.
    #[test]
    fn a_decorator_handed_to_a_generated_field_is_refused_once_and_the_field_kept_as_it_is() {
        let mark = nest_rs_codegen::__private::operation_field_mark();
        let field = quote! {
            async fn preview(&self, ctx: &Context<'_>) -> Result<FieldAnswer<String>> {
                #mark
                run_field(SITE, ctx, move || async move { resolve(ctx).await }).await
            }
        };
        for args in [
            quote!(upstream),
            quote!(upstream, retries = 0, breaker_open = 1),
        ] {
            let refused = refusals(args.clone(), field.clone());
            assert_eq!(refused.len(), 1, "{refused:#?}");
            assert!(
                refused[0].starts_with("#[resilient] reached the field `#[operations]` generates")
                    && refused[0].contains("import it as `resilient`"),
                "{refused:#?}"
            );
            assert_eq!(kept(args, field.clone()), field.to_string());
        }
        // An instrument the field carries above the decorator wraps the mark with the body.
        let wrapped = instrumented_above(quote! {
            #mark
            run_field(SITE, ctx, move || async move { resolve(ctx).await }).await
        });
        let refused = refusals(quote!(upstream), wrapped);
        assert_eq!(refused.len(), 1, "{refused:#?}");
        assert!(refused[0].starts_with("#[resilient] reached the field"));
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
    /// key earns after the field, rather than as a field the list names wrong.
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
                key.to_owned(),
            ] {
                let refused = refusals(first.parse().expect("arguments"), item.clone());
                assert_eq!(refused, after, "#[resilient({first})]");
            }
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
                 attempt :: < Answer > () ; } send (order) . await }"
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
            let ty: Type = syn::parse2(ty).expect("a type");
            assert!(impl_trait(&ty).is_some(), "{}", ty.to_token_stream());
        }
        let ty: Type = syn::parse2(quote!(Result<Box<dyn Display>, E>)).expect("a type");
        assert!(impl_trait(&ty).is_none());
    }
}
