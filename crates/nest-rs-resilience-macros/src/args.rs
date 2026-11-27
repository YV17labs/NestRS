//! What `#[resilient(…)]` was given: the field, each key and its value, and the
//! run they declare, each refusal of the list in the key's words.

use nest_rs_codegen::__private::{
    RESILIENT, RETRIES, TIMEOUT, needs_a_value, resilient_policy_setting, resilient_retries,
    resilient_timeout, site, takes_value, ungrouped_expr,
};
use nest_rs_codegen::{Arg, Grammar, Refusals};
use proc_macro2::{Span, TokenStream, TokenTree};
use quote::ToTokens;
use syn::ext::IdentExt;
use syn::parse::discouraged::Speculative;
use syn::parse::{ParseStream, Parser};
use syn::{Expr, ExprLit, Ident, ImplItemFn, Lit, Member, Token};

use crate::expansion::names;
use crate::method::Bound;

/// The key declaring the call replayable, read off `run_idempotent`.
pub(crate) const IDEMPOTENT: &str = "idempotent";

/// The keys saying how an attempt's answer counts: sent again, an `Err` or an
/// `Ok`, and a failure in the circuit breaker.
const RETRY_IF: &str = "retry_if";

const RETRY_OK_IF: &str = "retry_ok_if";

const RECORD_IF: &str = "record_if";

/// The key naming the method answering in place of the run's error, and the
/// one saying which errors it answers for.
pub(crate) const FALLBACK: &str = "fallback";

const FALLBACK_IF: &str = "fallback_if";

/// The key naming the attempt's `CancellationToken` in the body.
pub(crate) const CANCEL: &str = "cancel";

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
pub(crate) struct Args {
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
    pub(crate) fallback: Option<Ident>,
    fallback_if: Option<Expr>,
    /// The name `cancel` binds the attempt's token under, when it gives one.
    pub(crate) cancel: Option<Ident>,
}

impl Args {
    /// `key` as the list wrote it, when it wrote one.
    pub(crate) fn key(&self, key: &str) -> Option<&Ident> {
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

    /// The run the list declares for `method`.
    pub(crate) fn plan(&self, method: &ImplItemFn) -> Plan<'_> {
        let fallback = self.key(FALLBACK).zip(self.fallback.as_ref());
        let judged = self
            .record_if
            .as_ref()
            .filter(|predicate| reads_an_argument(predicate, method))
            .and(self.key(RECORD_IF));
        Plan {
            field: &self.field,
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
            cancel: self.cancel.as_ref(),
        }
    }
}

/// Whether `predicate` may read an argument of `method` once the one attempt
/// answered: its tokens name one, as [`names`] reads a body, or an argument is
/// bound by a macro's pattern, whose names no attribute reads. Read by tokens,
/// so a word of an argument's spelling only lends, as every predicate once did;
/// a predicate reading an argument nothing names here does not exist, since
/// a closure reaches an argument by its name alone.
fn reads_an_argument(predicate: &Expr, method: &ImplItemFn) -> bool {
    let bound = Bound::by(method);
    let tokens = predicate.to_token_stream();
    !bound.unparsed.is_empty()
        || bound
            .names
            .iter()
            .any(|binding| names(tokens.clone(), &binding.ident.unraw().to_string()))
}

/// How a method's body runs under its policy.
pub(crate) struct Plan<'a> {
    pub(crate) field: &'a Ident,
    /// `idempotent` as written, when the call is sent again: kept for its span.
    pub(crate) idempotent: Option<&'a Ident>,
    /// The key that has each attempt borrow the arguments rather than own
    /// them, as written: `idempotent`, whose next attempt reads them again,
    /// `fallback`, handed them next, or a `record_if` naming one, which may
    /// read it once the one attempt answered.
    pub(crate) lends: Option<&'a Ident>,
    pub(crate) timeout_millis: Option<u64>,
    pub(crate) retries: Option<u32>,
    pub(crate) retry_if: Option<&'a Expr>,
    pub(crate) retry_ok_if: Option<&'a Expr>,
    pub(crate) record_if: Option<&'a Expr>,
    pub(crate) fallback: Option<Fallback<'a>>,
    pub(crate) cancel: Option<&'a Ident>,
}

/// The method answering in place of a run's error.
pub(crate) struct Fallback<'a> {
    /// `fallback` as written, kept for its span.
    pub(crate) key: &'a Ident,
    pub(crate) method: &'a Ident,
    /// What `fallback_if` was given.
    pub(crate) when: Option<&'a Expr>,
}

/// The arguments: the field and every key written after it, each refusal kept
/// in `refusals`; `None` when the field itself is what was refused.
///
/// A refused key does not end the reading, which [`resumes`] after it, so what
/// one key says of another — the one it needs, `retries`' value beside
/// `idempotent` — is said of the whole list. Where the reading cannot resume,
/// a key that may stand in what is left is unread, never absent.
pub(crate) fn parse_args(args: TokenStream, refusals: &mut Refusals) -> Option<Args> {
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
        example: "|error| matches!(error, ResilienceError::Refused { .. })",
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
/// at the next one: over what is written there as a key's value, when a comma
/// or the list's end follows it. A value that reads as no expression, and
/// anything else before that comma, ends the reading.
///
/// The next comma is no boundary by itself: `pick::<A, B>` holds one outside
/// every group, and a key read from `B` would be refused though no one wrote it.
fn resumes(input: ParseStream<'_>) -> bool {
    let ahead = input.fork();
    if !skip_value(&ahead) || !(ahead.is_empty() || ahead.parse::<Token![,]>().is_ok()) {
        return false;
    }
    input.advance_to(&ahead);
    true
}

/// Reads past what follows a key's name as its value, `= expression` or
/// `(…)`, or nothing: whether what is there reads as one.
fn skip_value(input: ParseStream<'_>) -> bool {
    if input.peek(Token![=]) {
        input
            .parse::<Token![=]>()
            .and_then(|_| input.parse::<Expr>())
            .is_ok()
    } else if input.peek(syn::token::Paren) {
        input.parse::<TokenTree>().is_ok()
    } else {
        true
    }
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
            if !skip_value(&ahead) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    use crate::resilient::{decorated, resilient};
    use crate::testing::{refusals, track};

    /// A predicate may read an argument once the one attempt answered: under a
    /// `record_if` naming one, that attempt borrows the arguments, its token a
    /// `let` of its block; under one naming none, it owns them, as with no key.
    #[test]
    fn a_method_run_once_lends_its_arguments_to_its_one_attempt_where_its_predicate_names_one() {
        let item = quote! {
            async fn ship(&self, order: Order, held: Guard) -> Answer {
                send(&order, &held).await
            }
        };
        for record_if in [
            quote!(|_| order.urgent()),
            quote!(|error| error.counts(held)),
            quote!(|_| matches!(order, Order::Urgent)),
            quote!(|_| tracing::enabled!("{order:?}")),
        ] {
            let once = resilient(quote!(carrier, record_if = #record_if), item.clone()).to_string();
            assert!(once.contains("run :: < Answer , _ , _ , _ , _ >"), "{once}");
            assert!(
                once.contains(&format!("{record_if}) , || async {{ if false")),
                "{once}"
            );
            assert!(!once.contains("move"), "{once}");
            assert!(!once.contains("with_fallback"), "{once}");
        }
        for record_if in [
            quote!(Down::counts),
            quote!(|error| error.counts()),
            quote!(|_| self.order.urgent()),
            quote!(|_| Kind::order(true)),
        ] {
            let owned =
                resilient(quote!(carrier, record_if = #record_if), item.clone()).to_string();
            assert!(
                owned.contains(&format!(
                    "{record_if}) , move || async move {{ let _ = (& held , & order ,) ; if false"
                )),
                "{owned}"
            );
        }
        let patterned = resilient(
            quote!(carrier, record_if = Down::counts),
            quote! {
                async fn ship(&self, order!(weight): u32) -> Answer {
                    send(weight).await
                }
            },
        )
        .to_string();
        assert!(
            patterned.contains("Down :: counts) , || async {"),
            "{patterned}"
        );
        let bound = resilient(
            quote!(carrier, cancel = token, record_if = |_| order.urgent()),
            item,
        )
        .to_string();
        assert!(
            bound.contains("| __cancel | async { let token = __cancel ; if false"),
            "{bound}"
        );
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
        let read = decorated(
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
        let read = decorated(
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
                "fallback_if = |error| matches!(error, ResilienceError::Refused { .. })",
            ),
            (CANCEL, "cancel = token"),
        ] {
            let args: TokenStream = format!("carrier, {key}").parse().expect("arguments");
            let refused = refusals(args, track());
            let said = format!("#[resilient] `{key}` needs a value — write `{example}`");
            assert!(refused.contains(&said), "{refused:#?}");
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
            let read = decorated(args.clone(), track());
            assert!(read.is_ok(), "#[resilient({args})]");
        }
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
            let expanded = decorated(list.clone(), track())
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
}
