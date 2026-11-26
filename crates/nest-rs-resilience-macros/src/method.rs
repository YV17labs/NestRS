//! What the decorated method's signature says — its receiver, what its
//! arguments bind, its answer — and each refusal of it.

use nest_rs_codegen::__private::{HostBorrow, RESILIENT, is_resilient, shared_receiver, site};
use nest_rs_codegen::Refusals;
use proc_macro2::Span;
use quote::{ToTokens, quote};
use syn::ext::IdentExt;
use syn::{
    FnArg, GenericArgument, Ident, ImplItemFn, Pat, PatIdent, PatType, PathArguments, ReturnType,
    Token, Type, TypeParamBound,
};

use crate::args::{Args, CANCEL, FALLBACK, IDEMPOTENT};
use crate::resilient::TWICE;

/// The arguments `method` takes beside its receiver.
pub(crate) fn typed_inputs(method: &ImplItemFn) -> impl Iterator<Item = &PatType> {
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
pub(crate) struct Bound<'a> {
    /// Every name bound, in order, wherever a pattern binds it.
    pub(crate) names: Vec<&'a PatIdent>,
    /// Each pattern whose names no attribute reads: a macro's, expanded after
    /// every attribute on its method, and one `syn` keeps as tokens.
    pub(crate) unparsed: Vec<&'a Pat>,
}

impl<'a> Bound<'a> {
    /// What the arguments of `method` bind, beside its receiver.
    pub(crate) fn by(method: &'a ImplItemFn) -> Self {
        let mut bound = Self::default();
        for typed in typed_inputs(method) {
            bound.read(&typed.pat);
        }
        bound
    }

    /// Adds what `pat` binds, wherever it binds it.
    pub(crate) fn read(&mut self, pat: &'a Pat) {
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

/// Every refusal of what `method` is, beside its arguments' — `args` `None`
/// when its field did not read — and its receiver's `self`, when it lends the
/// shared borrow the policy's field is read through.
pub(crate) fn check(
    method: &ImplItemFn,
    args: Option<&Args>,
    refusals: &mut Refusals,
) -> Option<Token![self]> {
    if let Some(second) = method.attrs.iter().find(|attr| is_resilient(attr)) {
        refusals.push(syn::Error::new_spanned(second, TWICE));
    }
    if method.sig.asyncness.is_none() {
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
    if let ReturnType::Type(_, answered) = &method.sig.output
        && let Some(opaque) = whole_impl_trait(answered)
    {
        refusals.push(syn::Error::new_spanned(
            opaque,
            "#[resilient] reads what an attempt answers off the method's \
             `Result<T, ResilienceError<E>>`, and an `impl Trait` answered whole names neither \
             `T` nor `E` — answer `Result<impl Trait, ResilienceError<E>>`, the `impl Trait` \
             inside it, or name the type",
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
            if let Some(binding) = mutable_binding(&typed.pat)
                && unnamed.is_none()
                && let Some(mutability) = binding.mutability
            {
                let written = &binding.ident;
                refusals.push(syn::Error::new(
                    mutability.span,
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
    receiver
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
pub(crate) fn plain_name(pat: &Pat) -> Option<&Ident> {
    match pat {
        Pat::Ident(bound) if bound.by_ref.is_none() && bound.subpat.is_none() => Some(&bound.ident),
        _ => None,
    }
}

/// `ty` when it is an `impl Trait` whole, read through parentheses and an
/// invisible group.
fn whole_impl_trait(ty: &Type) -> Option<&Type> {
    match ty {
        Type::ImplTrait(_) => Some(ty),
        Type::Paren(paren) => whole_impl_trait(&paren.elem),
        Type::Group(group) => whole_impl_trait(&group.elem),
        _ => None,
    }
}

/// Writes `_` in the place of each `impl Trait` `ty` holds, wherever it sits
/// in it: rustc infers what each hides from the body, through the attempt's
/// type, as it does for the undecorated method.
pub(crate) fn infer_impl_traits(ty: &mut Type) {
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

/// The first binding `pat` makes `mut`, wherever the pattern makes it.
fn mutable_binding(pat: &Pat) -> Option<&PatIdent> {
    let mut bound = Bound::default();
    bound.read(pat);
    bound
        .names
        .into_iter()
        .find(|binding| binding.mutability.is_some())
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

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::TokenStream;
    use quote::{ToTokens, quote};
    use syn::parse::Parser;
    use syn::{Pat, Type};

    use crate::resilient::{decorated, resilient};
    use crate::testing::{refusals, track};

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
        let apart = decorated(quote!(carrier, cancel = cancellation), item);
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
                decorated(quote!(carrier, cancel = #token), track()).is_ok(),
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
    fn an_answer_that_is_an_impl_trait_whole_is_refused_and_one_inside_is_inferred() {
        let item = quote! {
            async fn opaque(&self, token: String) -> impl Quoted {
                token
            }
        };
        for args in [
            quote!(carrier),
            quote!(carrier, cancel = token),
            quote!(carrier, cancel),
            quote!(carrier, cancel = "token"),
            quote!(cancel = token),
        ] {
            let refused = refusals(args.clone(), item.clone());
            assert!(
                refused.iter().any(|refused| refused
                    .starts_with("#[resilient] reads what an attempt answers off the method's")),
                "#[resilient({args})]: {refused:#?}"
            );
        }
        for args in [
            quote!(carrier),
            quote!(carrier, idempotent, cancel = stop),
            quote!(carrier, record_if = |_| order.is_empty()),
            quote!(carrier, fallback = stale),
        ] {
            let expanded = resilient(
                args.clone(),
                quote! {
                    async fn label(&self, order: Order) -> Result<impl Display + use<>, Down> {
                        Ok(order)
                    }
                },
            )
            .to_string();
            assert!(
                expanded.contains(
                    "run :: < Result < _ , Down > , _ , _ , _ , _ > (:: nest_rs_resilience"
                ) || expanded.contains("_cancellable :: < Result < _ , Down > , _ , _ , _ , _ >"),
                "#[resilient({args})]: {expanded}"
            );
            assert!(
                expanded.contains("attempt :: < Result < _ , Down > > ()"),
                "#[resilient({args})]: {expanded}"
            );
            assert!(
                expanded.contains("-> Result < impl Display + use < > , Down >"),
                "the signature as written: {expanded}"
            );
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
            let read = decorated(unasked.clone(), item.clone());
            assert!(read.is_ok(), "#[resilient({unasked})] names no token");
        }
    }

    #[test]
    fn a_mutable_binding_is_found_wherever_the_pattern_makes_it() {
        let pat = Pat::parse_single
            .parse2(quote!((a, Wrapper { mut b, .. })))
            .expect("a pattern");
        assert_eq!(
            mutable_binding(&pat).map(|binding| binding.ident.to_string()),
            Some("b".to_owned())
        );
        let pat = Pat::parse_single.parse2(quote!((a, b))).expect("a pattern");
        assert!(mutable_binding(&pat).is_none());
    }

    #[test]
    fn an_impl_trait_is_inferred_wherever_the_answer_holds_it_and_refused_only_whole() {
        for (ty, inferred) in [
            (quote!(impl Future), "_"),
            (quote!((impl Future)), "(_)"),
            (
                quote!(Result<impl Display + use<>, ResilienceError<E>>),
                "Result < _ , ResilienceError < E > >",
            ),
            (
                quote!(Result<Vec<(u8, impl Display)>, E>),
                "Result < Vec < (u8 , _) > , E >",
            ),
            (
                quote!(Box<dyn Iterator<Item = impl Display>>),
                "Box < dyn Iterator < Item = _ > >",
            ),
            (
                quote!(Result<Box<dyn Display>, E>),
                "Result < Box < dyn Display > , E >",
            ),
        ] {
            let mut ty: Type = syn::parse2(ty).expect("a type");
            let whole = whole_impl_trait(&ty).is_some();
            infer_impl_traits(&mut ty);
            assert_eq!(ty.to_token_stream().to_string(), inferred);
            assert_eq!(
                whole,
                inferred.trim_matches(['(', ')']) == "_",
                "{inferred}"
            );
        }
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
