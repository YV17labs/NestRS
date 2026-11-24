//! A method's body as `#[tracing::instrument]` leaves it when it is written
//! above `#[resilient]` and so expands first: the parts of that expansion the
//! decorator re-types and re-borrows to run it as one attempt.
//!
//! Read off tracing-attributes 0.1.31 (`expand.rs`, `gen_block`), an `async fn`:
//!
//! ```text
//! let __tracing_attr_span = span!(…);
//! let __tracing_instrument_future = async move {
//!     if false { let __tracing_attr_fake_return: Answer = loop {}; return …; }
//!     { body }
//! };
//! if !__tracing_attr_span.is_disabled() { …instrument(…).await } else { ….await }
//! ```
//!
//! Under `err` or `ret` the future's first statement awaits a second
//! `async move` block, and that one holds the return never taken and the body.

use proc_macro2::Span;
use syn::{Expr, ExprAsync, Pat, Stmt, Type};

const SPAN: &str = "__tracing_attr_span";
const FUTURE: &str = "__tracing_instrument_future";
const FAKE_RETURN: &str = "__tracing_attr_fake_return";

/// One part of a body `#[tracing::instrument]` wrapped.
pub(crate) enum Part<'a> {
    /// An `async` block tracing wrote around the body, moving the arguments in.
    Future(&'a mut ExprAsync),
    /// The type tracing ascribed to that block's answer: the method's own.
    Answer(&'a mut Type),
    /// The statements the method was written with — its whole body when no
    /// instrument wrapped it.
    Written(&'a mut Vec<Stmt>),
}

/// Hand `each` every part of `stmts`, a method's body, outermost first, through
/// each `#[tracing::instrument]` that wrapped it — or refuse a body carrying
/// tracing's names in a shape other than the one above, which the decorator
/// could type as no attempt.
pub(crate) fn instrumented(
    stmts: &mut Vec<Stmt>,
    each: &mut dyn FnMut(Part<'_>),
) -> syn::Result<()> {
    let Some(Stmt::Local(bound)) = stmts.iter_mut().find(|stmt| binds(stmt, FUTURE)) else {
        return match stmts.iter().find(|stmt| binds(stmt, SPAN)) {
            Some(Stmt::Local(span)) => Err(unread(span.let_token.span)),
            _ => {
                each(Part::Written(stmts));
                Ok(())
            }
        };
    };
    let found = bound.let_token.span;
    let Some(Expr::Async(future)) = bound.init.as_mut().map(|init| &mut *init.expr) else {
        return Err(unread(found));
    };
    each(Part::Future(future));
    let attempt = if fake_return(future.block.stmts.first_mut()).is_some() {
        future
    } else {
        let Some(awaited) = future.block.stmts.first_mut().and_then(awaited) else {
            return Err(unread(found));
        };
        each(Part::Future(awaited));
        awaited
    };
    let [first, Stmt::Expr(Expr::Block(written), None)] = attempt.block.stmts.as_mut_slice() else {
        return Err(unread(found));
    };
    let Some(answer) = fake_return(Some(first)) else {
        return Err(unread(found));
    };
    each(Part::Answer(answer));
    instrumented(&mut written.block.stmts, each)
}

/// Whether `stmt` is a `let` binding `name` and nothing else.
fn binds(stmt: &Stmt, name: &str) -> bool {
    let Stmt::Local(local) = stmt else {
        return false;
    };
    let bound = match &local.pat {
        Pat::Type(typed) => &*typed.pat,
        bound => bound,
    };
    matches!(bound, Pat::Ident(bound) if bound.ident == name)
}

/// The type of tracing's return never taken, when `stmt` is the `if false`
/// holding it.
fn fake_return(stmt: Option<&mut Stmt>) -> Option<&mut Type> {
    let Some(Stmt::Expr(Expr::If(never), _)) = stmt else {
        return None;
    };
    let Some(Stmt::Local(returned)) = never.then_branch.stmts.first_mut() else {
        return None;
    };
    match &mut returned.pat {
        Pat::Type(typed) if matches!(&*typed.pat, Pat::Ident(bound) if bound.ident == FAKE_RETURN) => {
            Some(&mut *typed.ty)
        }
        _ => None,
    }
}

/// The `async` block `stmt` awaits first: bound by a `let` under `ret`, the
/// scrutinee of a `match` under `err` alone.
fn awaited(stmt: &mut Stmt) -> Option<&mut ExprAsync> {
    let mut expr = match stmt {
        Stmt::Local(local) => &mut *local.init.as_mut()?.expr,
        Stmt::Expr(expr, _) => expr,
        Stmt::Item(_) | Stmt::Macro(_) => return None,
    };
    loop {
        expr = match expr {
            Expr::Async(block) => return Some(block),
            Expr::Await(awaited) => &mut *awaited.base,
            Expr::Match(matched) => &mut *matched.expr,
            _ => return None,
        };
    }
}

/// The refusal of a body carrying tracing's names in a shape this file does not
/// read, at the `let` found, which tracing spans at its attribute.
fn unread(found: Span) -> syn::Error {
    syn::Error::new(
        found,
        "#[resilient] under a `#[tracing::instrument]` types the body that attribute wrapped as \
         one attempt, and found it in no shape it reads — write `#[resilient]` above \
         `#[tracing::instrument]`, whose span then covers the whole run",
    )
}

#[cfg(test)]
mod tests {
    use quote::{ToTokens, quote};
    use syn::parse_quote;

    use super::*;

    /// What `instrumented` hands of `body`: the `move` of each future, the
    /// answer's type and the statements written, in the order handed.
    fn parts(body: &mut syn::Block) -> syn::Result<Vec<String>> {
        let mut parts = Vec::new();
        instrumented(&mut body.stmts, &mut |part| {
            parts.push(match part {
                Part::Future(future) => match future.capture {
                    Some(_) => "async move".to_owned(),
                    None => "async".to_owned(),
                },
                Part::Answer(ty) => ty.to_token_stream().to_string(),
                Part::Written(stmts) => quote!(#(#stmts)*).to_string(),
            });
        })?;
        Ok(parts)
    }

    /// tracing-attributes 0.1.31's `fake_return_edge` around `body`.
    fn attempt(body: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        quote! {
            {
                #[allow(unknown_lints, unreachable_code, clippy::diverging_sub_expression)]
                if false {
                    let __tracing_attr_fake_return: Answer<String> = loop {};
                    return __tracing_attr_fake_return;
                }
                { #body }
            }
        }
    }

    /// tracing-attributes 0.1.31's body of an `async fn` building `future`.
    fn wrapped(future: proc_macro2::TokenStream) -> syn::Block {
        parse_quote!({
            {}
            let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
            let __tracing_instrument_future = #future;
            if !__tracing_attr_span.is_disabled() {
                ::tracing::Instrument::instrument(__tracing_instrument_future, __tracing_attr_span)
                    .await
            } else {
                __tracing_instrument_future.await
            }
        })
    }

    #[test]
    fn a_body_no_instrument_wrapped_is_handed_whole() {
        let mut body: syn::Block = parse_quote!({
            let label = print(order);
            send(label).await
        });
        assert_eq!(
            parts(&mut body).expect("read"),
            [quote!(let label = print(order); send(label).await).to_string()]
        );
    }

    #[test]
    fn each_flavour_of_an_instrument_hands_its_futures_its_answer_and_the_body_written() {
        let body = quote!(send(order).await);
        let written = body.to_string();
        let plain = attempt(body.clone());
        let mut bare = wrapped(quote!(async move #plain));
        assert_eq!(
            parts(&mut bare).expect("read"),
            ["async move", "Answer < String >", written.as_str()]
        );
        for future in [
            quote!(async move {
                let __match_scrutinee = async move #plain.await;
                match __match_scrutinee {
                    Ok(x) => Ok(x),
                    Err(e) => Err(e),
                }
            }),
            quote!(async move {
                match async move #plain.await {
                    Ok(x) => Ok(x),
                    Err(e) => Err(e),
                }
            }),
            quote!(async move {
                let x = async move #plain.await;
                x
            }),
        ] {
            let mut observed = wrapped(future);
            assert_eq!(
                parts(&mut observed).expect("read"),
                [
                    "async move",
                    "async move",
                    "Answer < String >",
                    written.as_str()
                ]
            );
        }
    }

    #[test]
    fn two_instruments_are_read_one_inside_the_other() {
        let inner = wrapped({
            let plain = attempt(quote!(send(order).await));
            quote!(async move #plain)
        });
        let inner = &inner.stmts;
        let outer = attempt(quote!(#(#inner)*));
        let mut body = wrapped(quote!(async move #outer));
        assert_eq!(
            parts(&mut body).expect("read"),
            [
                "async move",
                "Answer < String >",
                "async move",
                "Answer < String >",
                quote!(send(order).await).to_string().as_str(),
            ]
        );
    }

    /// The developer's own `async move`, and tracing's names below the
    /// statements written, are the body's: only its futures are handed.
    #[test]
    fn nothing_below_the_statements_written_is_read_as_tracings() {
        let written = quote! {
            let sent = async move {
                let __tracing_attr_span = order.span();
                let __tracing_instrument_future = async move { send(order).await };
                __tracing_instrument_future.await
            };
            sent.await
        };
        let mut body = wrapped({
            let plain = attempt(written.clone());
            quote!(async move #plain)
        });
        assert_eq!(
            parts(&mut body).expect("read"),
            [
                "async move",
                "Answer < String >",
                written.to_string().as_str()
            ]
        );
        let mut bare: syn::Block = parse_quote!({ #written });
        assert_eq!(parts(&mut bare).expect("read"), [written.to_string()]);
    }

    #[test]
    fn a_shape_tracing_does_not_write_is_refused_saying_which_order_compiles() {
        for mut unread in [
            parse_quote!({
                let __tracing_attr_span = ::tracing::span!(::tracing::Level::INFO, "ship");
                let _entered = __tracing_attr_span.enter();
                send(order).await
            }),
            wrapped(quote!(::std::boxed::Box::pin(
                async move { send(order).await }
            ))),
            wrapped(quote!(async move { send(order).await })),
            wrapped({
                let plain = attempt(quote!(send(order).await));
                quote!(async move { let traced = (#plain, 1); traced.0 })
            }),
            wrapped({
                let plain = attempt(quote! {
                    let __tracing_instrument_future = send(order);
                    __tracing_instrument_future.await
                });
                quote!(async move #plain)
            }),
        ] {
            let refused = parts(&mut unread).expect_err("no shape tracing writes");
            assert!(
                refused
                    .to_string()
                    .contains("write `#[resilient]` above `#[tracing::instrument]`"),
                "{refused}"
            );
        }
    }
}
