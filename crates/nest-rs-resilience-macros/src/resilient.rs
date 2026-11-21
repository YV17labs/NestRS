//! `#[resilient(field, idempotent, timeout = "…", retries = N)]`: an `async fn`
//! method's body run under the policy a field of `self` holds.

use nest_rs_codegen::__private::{
    HostBorrow, RESILIENT, RETRIES, TIMEOUT, answer_span, needs_a_value, resilient_retries,
    resilient_timeout, shared_receiver, site,
};
use nest_rs_codegen::{Grammar, Refusals};
use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::{
    Expr, FnArg, GenericArgument, Ident, ImplItemFn, Pat, PathArguments, ReturnType, Token, Type,
    TypeParamBound,
};

/// The key declaring the call replayable, read off `run_idempotent`.
const IDEMPOTENT: &str = "idempotent";

const KEYS: [&str; 3] = [IDEMPOTENT, TIMEOUT, RETRIES];

const GRAMMAR: Grammar = Grammar::new(RESILIENT, &KEYS).bare(bare);

/// What `#[resilient(…)]` was given.
struct Args {
    field: Ident,
    /// The key as written: the replayed closure is spanned there, so what
    /// rustc says of it points at the declaration.
    idempotent: Option<Ident>,
    timeout_millis: Option<u64>,
    /// The value as written and its key, read once the whole list is.
    retries: Option<(Expr, Ident)>,
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
    /// Its receiver lends the shared borrow the policy's field is read through.
    shared: bool,
    /// Its return type holds an `impl Trait`, which no expansion can name.
    opaque: bool,
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
    let shape = check(&method, args.as_ref(), &mut refusals);
    let retries = args.as_ref().and_then(|args| {
        let (value, key) = args.retries.as_ref()?;
        if args.idempotent.is_none() {
            refusals.push(syn::Error::new(
                key.span(),
                format!(
                    "{} needs `idempotent`: a call run once is never sent again — declare the \
                     call `idempotent` where sending it twice leaves the dependency as sending it \
                     once does, or drop `retries`",
                    site(RESILIENT, Some(RETRIES)),
                ),
            ));
            return None;
        }
        refusals.handle(resilient_retries(value))
    });
    match (refusals.finish(), args) {
        (Ok(()), Some(args)) => {
            let run = match &args.idempotent {
                Some(key) => Run::Idempotent(key),
                None => Run::Once,
            };
            Ok(expand(
                method,
                &args.field,
                &run,
                args.timeout_millis,
                retries,
            ))
        }
        (Err(refusal), args) => {
            method.attrs.retain(|attr| !is_resilient(attr));
            Err(Refused {
                kept: kept(method, args.as_ref(), &shape),
                refusal,
            })
        }
        (Ok(()), None) => Err(refused(item, Refusals::default())),
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
/// was refused, and as written where it is no `async fn` or names no type.
fn kept(method: ImplItemFn, args: Option<&Args>, shape: &Shape) -> TokenStream {
    if !shape.asynchronous || shape.opaque {
        return method.into_token_stream();
    }
    match args {
        Some(args) if shape.shared => expand(method, &args.field, &Run::Once, None, None),
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
            idempotent: None,
            timeout_millis: None,
            retries: None,
        };
        if input.is_empty() {
            return Ok(args);
        }
        input.parse::<Token![,]>()?;
        let keys = GRAMMAR.parse(input, |arg| {
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
                    args.idempotent = Some(arg.ident().clone());
                }
                TIMEOUT => args.timeout_millis = refusals.handle(resilient_timeout(&arg.expr()?)),
                RETRIES => args.retries = Some((arg.expr()?, arg.ident().clone())),
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
        });
        if let Err(refusal) = keys {
            refusals.push(refusal);
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
/// key, or anything but a field's name.
fn field(input: ParseStream<'_>) -> syn::Result<Ident> {
    if let Ok(leading) = input.fork().call(Ident::parse_any)
        && KEYS.contains(&leading.unraw().to_string().as_str())
    {
        return Err(syn::Error::new(
            leading.span(),
            format!(
                "#[resilient] reads the field holding the policy first, and `{leading}` is one of \
                 its keys — write the field before it: `#[resilient(carrier, {leading})]`",
            ),
        ));
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
    let shared = refusals
        .handle(shared_receiver(
            method,
            "#[resilient]",
            &Ident::new("Self", Span::call_site()),
            HostBorrow::Arc,
        ))
        .is_some();
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
    if args.is_some_and(|args| args.idempotent.is_some()) {
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
        shared,
        opaque: opaque.is_some(),
    }
}

/// Whether `attr` is a `#[resilient]`, however its path is written.
fn is_resilient(attr: &syn::Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == RESILIENT)
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

/// `method`'s body, taken out of it and braced as it was written.
fn body(method: &mut ImplItemFn) -> syn::Block {
    syn::Block {
        brace_token: method.block.brace_token,
        stmts: std::mem::take(&mut method.block.stmts),
    }
}

/// `method` with its body run under the policy `field` holds, as `run` says.
fn expand(
    mut method: ImplItemFn,
    field: &Ident,
    run: &Run<'_>,
    timeout_millis: Option<u64>,
    retries: Option<u32>,
) -> TokenStream {
    let (answer, at) = answer(&method);
    let policy =
        quote_spanned!(field.span()=> ::nest_rs_resilience::__private::policy(&self.#field));
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
    let block = body(&mut method);
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
    let block = body(&mut method);
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
            "| _ | async { quote (zone) . await }",
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
            expanded.contains("move | _ | async move { send (order) . await }"),
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
