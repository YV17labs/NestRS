//! `#[resilient(field, …)]`: an `async fn` method's body run under the policy a
//! field of `self` holds — its list read, the method checked, then expanded, or
//! refused as its refusals.

use nest_rs_codegen::__private::{is_resilient, mixed_site_ident};
use nest_rs_codegen::Refusals;
use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote_spanned};
use syn::{Expr, ImplItemFn, Item, Stmt, Token, TraitItemFn};

use crate::args::parse_args;
use crate::expansion::expand;
use crate::method::check;

/// The method `item` is, its body run under the policy its field holds — or,
/// refused, its refusals.
pub(crate) fn resilient(args: TokenStream, item: TokenStream) -> TokenStream {
    match decorated(args, item) {
        Ok(expanded) => expanded,
        Err(refused) => refused.emitted(),
    }
}

/// A decorated item the attribute refused, and every refusal it earned: what
/// the attribute emits in its place says them, and nothing else is reported.
pub(crate) enum Refused {
    /// A method, whose refusals stand in its body's place.
    Method(Box<ImplItemFn>, syn::Error),
    /// What is no method the attribute could read, or one already run under a
    /// policy: kept as written, beside its refusals.
    Item(TokenStream, syn::Error),
}

impl Refused {
    /// `item` beside every refusal in `refusals`.
    fn item(item: TokenStream, refusals: Refusals) -> Self {
        let refusal = match refusals.finish() {
            Err(refusal) => refusal,
            Ok(()) => syn::Error::new(
                Span::call_site(),
                "#[resilient] refused this item without saying why — a defect in the framework, \
                 not in this code",
            ),
        };
        Self::Item(item, refusal)
    }

    /// What the attribute emits. A refused method is its refusals: its
    /// signature as written, but for `#[resilient]` and an `async` its body's
    /// own `.await` says it was meant with, and in its body's place the
    /// expansion's mark, then the refusals. An error expression answers any
    /// type and leaves no body to check, so no error of the body's and no lint
    /// of an argument it no longer reads joins them; a caller awaiting the
    /// method sees the future it was meant as.
    pub(crate) fn emitted(self) -> TokenStream {
        match self {
            Self::Method(mut method, refusal) => {
                method.attrs.retain(|attr| !is_resilient(attr));
                let written = method.block.stmts.iter().map(ToTokens::to_token_stream);
                if method.sig.asyncness.is_none()
                    && method.sig.constness.is_none()
                    && awaits(written.collect())
                {
                    method.sig.asyncness = Some(Token![async](method.sig.fn_token.span));
                }
                method.block.stmts = vec![
                    run_mark(),
                    Stmt::Expr(Expr::Verbatim(refusal.to_compile_error()), None),
                ];
                method.into_token_stream()
            }
            Self::Item(mut item, refusal) => {
                item.extend(refusal.to_compile_error());
                item
            }
        }
    }
}

pub(crate) fn decorated(args: TokenStream, item: TokenStream) -> Result<TokenStream, Refused> {
    let mut refusals = Refusals::default();
    let args = parse_args(args, &mut refusals);
    let Ok(method) = syn::parse2::<ImplItemFn>(item.clone()) else {
        refusals.push(no_method(&item));
        return Err(Refused::item(item, refusals));
    };
    let body: TokenStream = method
        .block
        .stmts
        .iter()
        .map(ToTokens::to_token_stream)
        .collect();
    // Already run under a policy, the method is told to keep one, and nothing of
    // the second's list: that list goes with it.
    if runs(body.clone()) {
        let refusal = syn::Error::new(Span::call_site(), TWICE);
        return Err(Refused::Item(item, refusal));
    }
    if let Some(instrument) = written_under(body, TRACING) {
        refusals.push(syn::Error::new(instrument, UNDER_AN_INSTRUMENT));
    }
    let receiver = check(&method, args.as_ref(), &mut refusals);
    match (refusals.finish(), args, receiver) {
        (Ok(()), Some(args), Some(receiver)) => {
            let plan = args.plan(&method);
            Ok(expand(method, receiver, &plan))
        }
        (Err(refusal), _, _) => Err(Refused::Method(Box::new(method), refusal)),
        (Ok(()), _, _) => Err(Refused::item(item, Refusals::default())),
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
pub(crate) const TWICE: &str = "#[resilient] is written twice here — a method runs under one policy, and a \
                     second would run the first's whole run again at each of its attempts. Keep \
                     one; a method calling two dependencies is two methods, each decorated";

/// The item the body of a method `#[resilient]` read opens on, expanded or
/// refused, in the macro ABI's prefix: no developer writes it, so a
/// `#[resilient]` finding it was written below another its name did not show —
/// one imported under another name, or an entry of a `#[cfg_attr]`.
const RUN: &str = "__nestrs_resilient_run";

/// The prefix of every name tracing-attributes binds in what it expands a
/// method to: a body holding one was wrapped by a `#[tracing::instrument]`
/// written above `#[resilient]`. Read as a courtesy, for the sentence: its
/// names are private, and a release writing others leaves rustc's own errors
/// at the body's answers.
const TRACING: &str = "__tracing_";

/// What a method `#[tracing::instrument]` expanded first is told.
const UNDER_AN_INSTRUMENT: &str = "#[resilient] runs the method's body as attempts of its policy, \
     and `#[tracing::instrument]` written above it has already wrapped that body in a future of \
     its own, answering the method's type where an attempt answers `Result<T, E>` — write \
     `#[resilient]` above `#[tracing::instrument]`, whose span then covers the whole run; for a \
     span per attempt, instrument a method the body calls";

/// The mark a body run under a policy opens on: an item rustc reports as no
/// dead code, its name leading with `_`, so it carries no lint level a crate's
/// `forbid` could refuse, every token the expansion's own, so no lint reads
/// it as a function the developer wrote.
pub(crate) fn run_mark() -> Stmt {
    let mark = mixed_site_ident(RUN);
    Stmt::Item(Item::Verbatim(
        quote_spanned!(Span::mixed_site()=> fn #mark() {}),
    ))
}

/// Whether `tokens`, a method's body, declare [`run_mark`] at any depth: at
/// their top where a `#[resilient]` ran on them, inside what an attribute
/// expanded between the two wrapped it in.
fn runs(tokens: TokenStream) -> bool {
    let mut after_fn = false;
    tokens.into_iter().any(|tree| {
        let marks = match &tree {
            TokenTree::Ident(name) if after_fn && name == RUN => true,
            TokenTree::Group(group) => runs(group.stream()),
            _ => false,
        };
        after_fn = matches!(&tree, TokenTree::Ident(word) if word == "fn");
        marks
    })
}

/// Where `tokens` first write a name beginning with `prefix`, at any depth.
fn written_under(tokens: TokenStream, prefix: &str) -> Option<Span> {
    tokens.into_iter().find_map(|tree| match tree {
        TokenTree::Ident(name) if name.to_string().starts_with(prefix) => Some(name.span()),
        TokenTree::Group(group) => written_under(group.stream(), prefix),
        _ => None,
    })
}

/// Whether `tokens`, a `fn`'s body, hold an `.await` of their own, which only
/// an `async fn`'s does: outside every `async` block, every item's body and
/// every macro's arguments, whose expansion may hold a block of its own. Read
/// by tokens: what is taken for one of those wrongly — the brace after a `fn`
/// pointer's type, a parenthesised operand of `!` — hides an `.await`, and the
/// method keeps the `fn` it was written as.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{emitted, instrumented_above, refusals};
    use quote::quote;

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
        assert!(refused[2].starts_with("#[resilient] reads what an attempt answers"));
        assert!(refused[3].contains("bound `mut`"));
        assert!(refused[4].contains("is a `&mut`"));
    }

    /// A body in a shape of tracing's the decorator does not read is one
    /// refusal, and is kept with no body: typed as the method's own answer,
    /// each answer it writes as an attempt would be an error of its own.
    /// A body a `#[tracing::instrument]` written above wrapped, in the shape
    /// tracing-attributes writes or another carrying its prefix, is one
    /// refusal, at the name tracing spans at its attribute, and the method is
    /// that refusal alone.
    #[test]
    fn a_body_an_instrument_wrapped_is_refused_once_saying_which_order_compiles() {
        let otherwise = quote! {
            async fn ship(&self, order: Order) -> Answer {
                let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
                let _entered = __tracing_attr_span.enter();
                send(order).await
            }
        };
        for item in [instrumented_above(quote!(send(&order).await)), otherwise] {
            for args in [
                quote!(carrier),
                quote!(carrier, idempotent),
                quote!(carrier, fallback = stale, cancel = token),
            ] {
                let refused = refusals(args.clone(), item.clone());
                assert_eq!(refused, [UNDER_AN_INSTRUMENT], "#[resilient({args})]");
                let emitted = emitted(args.clone(), item.clone());
                assert!(
                    emitted.ends_with(&format!(
                        "-> Answer {{ fn __nestrs_resilient_run () {{ }} :: core :: compile_error ! \
                         {{ \"{UNDER_AN_INSTRUMENT}\" }} }}"
                    )),
                    "#[resilient({args})]: {emitted}"
                );
            }
        }
    }

    /// A refused method, whatever refused it and whatever its shape, is its
    /// signature as written but for `#[resilient]` — `async` added where its
    /// body awaits at its own level — then the run's mark and one
    /// `compile_error!` per refusal in its body's place, and nothing else.
    #[test]
    fn a_refused_method_is_its_refusals_in_its_bodys_place() {
        for (args, item, signature, count) in [
            (
                quote!(carrier, retries = 2),
                quote!(
                    async fn ship(&self, mut order: Order) -> Answer {
                        send(order).await
                    }
                ),
                "async fn ship (& self , mut order : Order) -> Answer",
                1,
            ),
            (
                quote!(carrier, fallback = stale),
                quote!(
                    async fn ship(&mut self, mut order: Order) -> Answer {
                        send(order).await
                    }
                ),
                "async fn ship (& mut self , mut order : Order) -> Answer",
                2,
            ),
            (
                quote!(),
                quote!(
                    pub async fn ship(self: &Arc<Self>) -> Answer {
                        send().await
                    }
                ),
                "pub async fn ship (self : & Arc < Self >) -> Answer",
                1,
            ),
            (
                quote!(carrier),
                quote!(
                    fn ship(&self, order: Order) -> Answer {
                        send(order).await
                    }
                ),
                "async fn ship (& self , order : Order) -> Answer",
                1,
            ),
            (
                quote!(carrier),
                quote!(
                    fn ship(&self) -> Answer {
                        Box::pin(async move { send().await })
                    }
                ),
                "fn ship (& self) -> Answer",
                1,
            ),
            (
                quote!(carrier),
                quote!(
                    const fn ship(&self) -> Answer {
                        send().await
                    }
                ),
                "const fn ship (& self) -> Answer",
                1,
            ),
            (
                quote!(carrier, cancel),
                quote! {
                    #[doc = "Ships."]
                    #[resilient(carrier, idempotent)]
                    #[inline]
                    async fn ship(&self, parcel!(weight): u32) -> Answer { send(token).await }
                },
                "# [doc = \"Ships.\"] # [inline] async fn ship (& self , parcel ! (weight) : u32) \
                 -> Answer",
                2,
            ),
        ] {
            let refused = refusals(args.clone(), item.clone());
            assert_eq!(
                refused.len(),
                count,
                "#[resilient({args})] {item}: {refused:#?}"
            );
            let errors: String = refused
                .iter()
                .map(|refusal| format!(":: core :: compile_error ! {{ {refusal:?} }} "))
                .collect();
            assert_eq!(
                emitted(args.clone(), item.clone()),
                format!("{signature} {{ fn __nestrs_resilient_run () {{ }} {errors}}}"),
                "#[resilient({args})] {item}"
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
        assert!(
            emitted(quote!(carrier, idempotent), declared.clone())
                .starts_with(&format!("{declared} :: core :: compile_error !")),
            "kept as declared"
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
                assert_eq!(
                    emitted(second, run.clone()),
                    format!("{run} :: core :: compile_error ! {{ {TWICE:?} }}")
                );
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
            let kept_run: TokenStream = emitted(first.clone(), written.clone())
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
        assert!(decorated(quote!(carrier), named).is_ok());
    }

    /// A refused `fn` is emitted as the `async fn` it was meant as where its
    /// body awaits at its own level, and as the `fn` written otherwise: a
    /// boxed future's, an item's or a block's `.await` is no await of its own.
    #[test]
    fn a_refused_fn_is_the_async_fn_its_body_awaits_as_or_the_fn_written() {
        for (receiver, body) in [
            (
                quote!(&self,),
                quote!(let sent = send(&order).await?; Ok(sent)),
            ),
            (quote!(), quote!(send(order).await)),
        ] {
            let meant = emitted(
                quote!(carrier, fallback = stale),
                quote!(fn ship(#receiver order: Order) -> Answer { #body }),
            );
            let signature = quote!(async fn ship(#receiver order: Order) -> Answer);
            assert!(meant.starts_with(&signature.to_string()), "{meant}");
        }
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
            let written = emitted(
                quote!(carrier, cancel = token),
                quote! {
                    fn ship(&self, order: Order) -> Answer {
                        #body
                    }
                },
            );
            assert!(
                written.starts_with(
                    "fn ship (& self , order : Order) -> Answer { fn __nestrs_resilient_run () { } \
                     :: core :: compile_error !"
                ),
                "{body}: {written}"
            );
        }
    }
}
