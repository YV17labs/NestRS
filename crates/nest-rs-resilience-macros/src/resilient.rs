//! `#[resilient(field, idempotent, timeout = "…", retries = N)]`: an `async fn`
//! method's body run under the policy a field of `self` holds.

use nest_rs_codegen::__private::{
    HostBorrow, RESILIENT, RETRIES, TIMEOUT, answer_span, is_resilient, needs_a_value,
    opens_on_operation_field_mark, resilient_policy_setting, resilient_retries, resilient_timeout,
    shared_receiver, site,
};
use nest_rs_codegen::{Grammar, Refusals};
use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::{
    Expr, FnArg, GenericArgument, Ident, ImplItemFn, Pat, PathArguments, ReturnType, Stmt, Token,
    Type, TypeParamBound,
};

use crate::instrumented::{Part, instrumented};

/// The key declaring the call replayable, read off `run_idempotent`.
const IDEMPOTENT: &str = "idempotent";

const KEYS: [&str; 3] = [IDEMPOTENT, TIMEOUT, RETRIES];

const GRAMMAR: Grammar = Grammar::new(RESILIENT, &KEYS)
    .bare(bare)
    .elsewhere(resilient_policy_setting);

/// A key that says nothing without another, and why.
struct Needs {
    key: &'static str,
    needs: &'static str,
    why: &'static str,
}

const NEEDS: [Needs; 1] = [Needs {
    key: RETRIES,
    needs: IDEMPOTENT,
    why: "a call run once is never sent again — declare the call `idempotent` where sending it \
          twice leaves the dependency as sending it once does, or drop `retries`",
}];

/// What `#[resilient(…)]` was given.
struct Args {
    field: Ident,
    /// Every key the list read, as written: the replayed closure is spanned at
    /// `idempotent`, so what rustc says of it points at the declaration.
    keys: Vec<Ident>,
    /// The list was read to its end. A refused key ends its reading, and a key
    /// past it is unread, never absent.
    whole: bool,
    timeout_millis: Option<u64>,
    /// The value as written, judged beside `idempotent` alone.
    retries: Option<Expr>,
}

impl Args {
    /// `key` as the list wrote it, when it read one.
    fn key(&self, key: &str) -> Option<&Ident> {
        self.keys.iter().find(|written| written.unraw() == key)
    }

    /// Refuse each key written without the one it needs, in a list read whole
    /// alone: past a refused key, the one needed may stand unread.
    fn refuse_unmet_needs(&self, refusals: &mut Refusals) {
        if !self.whole {
            return;
        }
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
                        need.why,
                    ),
                ));
            }
        }
    }
}

/// How a method's body runs under its policy.
enum Run<'a> {
    Once,
    /// Again after a failure, the key declaring it kept for its span.
    Idempotent(&'a Ident),
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
    let args = parse_args(args, &mut refusals);
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
    let shape = Shape {
        unread,
        ..check(&method, args.as_ref(), &mut refusals)
    };
    let retries = args.as_ref().and_then(|args| {
        args.refuse_unmet_needs(&mut refusals);
        let value = args.retries.as_ref()?;
        args.key(IDEMPOTENT)?;
        refusals.handle(resilient_retries(value))
    });
    match (refusals.finish(), args, shape.receiver) {
        (Ok(()), Some(args), Some(receiver)) => {
            let run = match args.key(IDEMPOTENT) {
                Some(key) => Run::Idempotent(key),
                None => Run::Once,
            };
            Ok(expand(
                method,
                receiver,
                &args.field,
                &run,
                args.timeout_millis,
                retries,
            ))
        }
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
/// reported: run once where the method could be expanded at all, checked
/// against a stand-in that never runs where its receiver or its field is what
/// was refused, as written where it is no `async fn` or names no type, and
/// with no body where the one it has is an instrument's the decorator cannot
/// type as an attempt.
fn kept(mut method: ImplItemFn, args: Option<&Args>, shape: &Shape) -> TokenStream {
    if shape.unread {
        method.attrs.push(syn::parse_quote!(#[allow(unused)]));
        method.block.stmts = vec![Stmt::Expr(
            Expr::Verbatim(quote!(::core::unreachable!())),
            None,
        )];
        return method.into_token_stream();
    }
    if !shape.asynchronous || shape.opaque {
        return method.into_token_stream();
    }
    match (args, shape.receiver) {
        (Some(args), Some(receiver)) => {
            expand(method, receiver, &args.field, &Run::Once, None, None)
        }
        _ => stand_in(method),
    }
}

/// The arguments: the field and every key that read, each refusal kept in
/// `refusals`; `None` when the field itself is what was refused.
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
            whole: true,
            timeout_millis: None,
            retries: None,
        };
        if input.is_empty() {
            return Ok(args);
        }
        input.parse::<Token![,]>()?;
        let keys = GRAMMAR.parse(input, |arg| {
            let written = arg.ident().clone();
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
                TIMEOUT => args.timeout_millis = refusals.handle(resilient_timeout(&arg.expr()?)),
                RETRIES => args.retries = Some(arg.expr()?),
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
            args.keys.push(written);
            Ok(())
        });
        if let Err(refusal) = keys {
            refusals.push(refusal);
            args.whole = false;
            // What follows the refused key is read by nothing.
            input.step(|cursor| {
                let mut rest = *cursor;
                while let Some((_, next)) = rest.token_tree() {
                    rest = next;
                }
                Ok(((), rest))
            })?;
        }
        Ok(args)
    })
    .parse2(args);
    refusals.handle(parsed)
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
    let example = match key {
        TIMEOUT => "timeout = \"2s\"",
        RETRIES => "retries = 3",
        _ => return needs_a_value(attr, key),
    };
    format!(
        "{} needs a value — write `{example}`",
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
    if args.is_some_and(|args| args.key(IDEMPOTENT).is_some()) {
        for input in &method.sig.inputs {
            let FnArg::Typed(typed) = input else {
                continue;
            };
            if let Some((binding, written)) = mutable_binding(&typed.pat) {
                refusals.push(syn::Error::new(
                    binding,
                    format!(
                        "{} attempts the body again with the arguments the method was called \
                         with, so none is bound `mut`: one attempt's change to `{written}` would \
                         reach the next — bind it without `mut`, and change a copy inside the body",
                        site(RESILIENT, Some(IDEMPOTENT)),
                    ),
                ));
            }
            if let Some(reference) = exclusive_reference(&typed.ty) {
                refusals.push(syn::Error::new_spanned(
                    reference,
                    format!(
                        "{} attempts the body again with the arguments the method was called \
                         with, so none is a `&mut`: one attempt's change through `{}` would reach \
                         the next — take it by shared reference or by value, and change a copy \
                         inside the body",
                        site(RESILIENT, Some(IDEMPOTENT)),
                        typed.pat.to_token_stream(),
                    ),
                ));
            }
        }
    }
    Shape {
        asynchronous,
        receiver,
        opaque: opaque.is_some(),
        unread: false,
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
/// that future borrows them as the body it holds would.
fn body(method: &mut ImplItemFn, answer: &TokenStream, at: Span, borrows: bool) -> syn::Block {
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
    let items = stmts
        .iter()
        .take_while(|stmt| matches!(stmt, syn::Stmt::Item(_)))
        .count();
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
    syn::Block {
        brace_token: method.block.brace_token,
        stmts,
    }
}

/// `method` with its body run under the policy `field` holds, as `run` says,
/// the field read through `receiver`, the method's own `self`.
fn expand(
    mut method: ImplItemFn,
    receiver: Token![self],
    field: &Ident,
    run: &Run<'_>,
    timeout_millis: Option<u64>,
    retries: Option<u32>,
) -> TokenStream {
    let (answer, at) = answer(&method);
    // `self` resolves as the receiver does, which a `macro_rules!` may write apart
    // from the attribute, and sits at the field, where the field's refusal points.
    let receiver = Token![self](receiver.span.located_at(field.span()));
    let policy = quote_spanned!(field.span()=>
        ::nest_rs_resilience::__private::policy(&#receiver.#field)
    );
    let name = method.sig.ident.unraw().to_string();
    let timeout = optional(timeout_millis);
    let retries = optional(retries);
    let site = quote! {
        ::nest_rs_resilience::__private::resilient_method(
            ::core::any::type_name::<Self>,
            #name,
            #timeout,
            #retries,
        )
    };
    let block = body(&mut method, &answer, at, matches!(run, Run::Idempotent(_)));
    let call = match run {
        Run::Idempotent(key) => {
            let replayed = quote_spanned!(key.span()=> |_| async #block);
            quote_spanned! {at=>
                ::nest_rs_resilience::__private::run_idempotent::<#answer, _, _, _>(
                    #policy, #site, #replayed,
                )
                .await
            }
        }
        Run::Once => {
            let once = quote!(move |_| async move #block);
            quote_spanned! {at=>
                ::nest_rs_resilience::__private::run::<#answer, _, _, _>(#policy, #site, #once)
                    .await
            }
        }
    };
    method.block.stmts = vec![syn::Stmt::Expr(Expr::Verbatim(call), None)];
    method.into_token_stream()
}

/// `method` with its body checked as one attempt against a stand-in that
/// reads no field and never runs.
fn stand_in(mut method: ImplItemFn) -> TokenStream {
    let (answer, at) = answer(&method);
    let block = body(&mut method, &answer, at, false);
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
            "run_idempotent :: < Result < u32 , ResilienceError < E > > , _ , _ , _ >",
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
            expanded.contains("run :: < Result < () , ResilienceError < E > > , _ , _ , _ >"),
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

    /// A refused key ends the list's reading, so `idempotent` after it is
    /// unread, not absent: the one refusal is the key's, and none says
    /// `retries` needs what the method wrote.
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
            let refused = refusals(
                unread.clone(),
                quote!(
                    async fn track(&self) -> Answer {
                        send().await
                    }
                ),
            );
            assert_eq!(refused.len(), 1, "#[resilient({unread})]: {refused:#?}");
            assert!(
                !refused[0].contains("needs `idempotent`"),
                "#[resilient({unread})]: {refused:#?}"
            );
        }
    }

    /// A key needed that was read before the refused one is known written, so
    /// what stands beside it is still judged; one that may stand past it is not.
    #[test]
    fn a_need_met_before_a_refused_key_is_judged_and_one_unread_is_not() {
        let item = quote!(
            async fn track(&self) -> Answer {
                send().await
            }
        );
        let met = refusals(
            quote!(carrier, idempotent, retries = 11, retry = 1),
            item.clone(),
        );
        assert_eq!(met.len(), 2, "{met:#?}");
        assert!(met[0].starts_with("unknown #[resilient] argument `retry`"));
        assert!(met[1].starts_with("#[resilient] `retries`: must be at most 10"));
        let unread = refusals(quote!(carrier, retries = 0, retry = 1), item);
        assert_eq!(unread.len(), 1, "{unread:#?}");
        assert!(unread[0].starts_with("unknown #[resilient] argument `retry`"));
    }

    /// Every key that needs another is refused in one sentence, naming both.
    #[test]
    fn every_key_needing_another_is_refused_without_it_in_one_sentence() {
        for need in &NEEDS {
            let args: TokenStream = format!("carrier, {} = 1", need.key)
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
                    need.key, need.needs, need.why
                )]
            );
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
        let item = instrumented_above(quote!(send(&order).await));
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
            kept.ends_with("-> Answer { :: core :: unreachable ! () }"),
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
