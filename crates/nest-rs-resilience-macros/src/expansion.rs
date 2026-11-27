//! An accepted method's expansion: its body run as the attempt of the run its
//! keys declare, its arguments dropping as the undecorated method's do.

use nest_rs_codegen::__private::{answer_span, cfg_attrs, mixed_site_ident};
use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Expr, Ident, ImplItemFn, ReturnType, Stmt, Token, Type};

use crate::args::{Fallback, Plan};
use crate::method::{Bound, infer_impl_traits, plain_name, typed_inputs};
use crate::resilient::run_mark;

/// What the method answers, as the expansion writes it — `_` in the place of
/// each `impl Trait` it holds, every token at one span, the expansion's own,
/// located at the return type — and that span.
///
/// Every helper the expansion calls is bounded on the answer there, so a
/// refused answer is one error: rustc reports one predicate once per span, and
/// a stable macro can give it no wider span than the type's first token.
fn answer(method: &ImplItemFn) -> (TokenStream, Span) {
    // The expansion's own, so no lint reads its `.await` as one a developer wrote:
    // clippy's `large_futures` would offer a fix at the answer that none can apply.
    let at = Span::mixed_site().located_at(answer_span(&method.sig));
    let answer = match &method.sig.output {
        ReturnType::Type(_, ty) => {
            let mut inferred = (**ty).clone();
            infer_impl_traits(&mut inferred);
            respanned(inferred.into_token_stream(), at)
        }
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
/// `captured`, the statement fixing the order an attempt owning the arguments
/// drops them in, leads the expansion's block, then `bound`, the `let` naming
/// the attempt's token where the closure's parameter cannot.
fn body(
    method: &mut ImplItemFn,
    answer: &TokenStream,
    at: Span,
    captured: Option<Stmt>,
    bound: Option<Stmt>,
) -> syn::Block {
    let stmts = std::mem::take(&mut method.block.stmts);
    let written = method.block.brace_token;
    let mut around: Vec<Stmt> = captured.into_iter().collect();
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
/// reborrowing a `&mut` it holds. Its method opens on the statement instead,
/// naming what the whole expansion names, borrowed for that statement alone:
/// a future an attribute below wraps the body in — `#[tracing::instrument]`'s
/// — takes the arguments in that order.
fn captured(method: &ImplItemFn, written: &TokenStream) -> Option<Stmt> {
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

/// `stmts`, a body, as the tokens it is written in.
fn tokens(stmts: &[Stmt]) -> TokenStream {
    stmts.iter().map(ToTokens::to_token_stream).collect()
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
pub(crate) fn names(tokens: TokenStream, name: &str) -> bool {
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
pub(crate) fn expand(
    mut method: ImplItemFn,
    receiver: Token![self],
    plan: &Plan<'_>,
) -> TokenStream {
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
        const {
            &::nest_rs_resilience::__private::resilient_method(
                ::core::any::type_name::<Self>,
                #name,
                #timeout,
                #retries,
            )
            #declares
        }
    };
    let lends = plan.lends;
    // A closure lending the arguments cannot lend its own parameter to the future it
    // answers: the attempt's block takes the token by value, under the name written.
    let (token, bound) = match (plan.cancel, lends) {
        (None, _) => (None, None),
        (Some(name), None) => (Some(name.to_token_stream()), None),
        (Some(name), Some(_)) => {
            let handed = mixed_site_ident("__cancel");
            let bound: Stmt = syn::parse_quote!(let #name = #handed;);
            (Some(handed.into_token_stream()), Some(bound))
        }
    };
    let handed = plan
        .fallback
        .as_ref()
        .map(|fallback| arguments(&method, fallback.key.span()));
    let owned = match lends {
        None => captured(&method, &tokens(&method.block.stmts)),
        Some(_) => None,
    };
    let block = body(&mut method, &answer, at, owned, bound);
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
    let taking = |at: Span| match &token {
        Some(token) => quote_spanned!(at=> |#token|),
        None => quote_spanned!(at=> ||),
    };
    let attempt = match lends {
        Some(key) => {
            let taking = taking(Span::mixed_site().located_at(key.span()));
            quote_spanned!(key.span()=> #taking async #block)
        }
        None => {
            let taking = taking(Span::call_site());
            quote!(move #taking async move #block)
        }
    };
    let run = match (plan.idempotent, plan.cancel) {
        (None, None) => "run",
        (None, Some(_)) => "run_cancellable",
        (Some(_), None) => "run_idempotent",
        (Some(_), Some(_)) => "run_idempotent_cancellable",
    };
    let run = Ident::new(run, at);
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
    let mut stmts = vec![run_mark()];
    if lends.is_some() {
        stmts.extend(captured(&method, &call));
    }
    stmts.push(Stmt::Expr(Expr::Verbatim(call), None));
    method.block.stmts = stmts;
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
    use proc_macro2::TokenStream;
    use quote::quote;

    use crate::resilient::resilient;

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
            "const { & :: nest_rs_resilience :: __private :: resilient_method (:: core :: any :: \
             type_name :: < Self > , \"rates\" , :: core :: option :: Option :: Some (2000u64) , :: \
             core :: option :: Option :: Some (3u32) ,) }",
            ", || async { if false { return :: nest_rs_resilience :: __private :: attempt :: < \
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
                "move || async move { if false { return :: nest_rs_resilience :: __private :: \
                 attempt :: < Result < () , ResilienceError < E > > > () ; } ({ send (order) . await \
                 }) }"
            ),
            "{expanded}"
        );
    }

    /// The attempt owning the arguments reads first, last declared first, each
    /// one its body names: its closure and its future capture them in that
    /// order, the one an `async fn` drops its parameters in. An attempt lending
    /// them reads none, and its method reads them instead.
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
            once.contains(&format!("move || async move {{ {read} if false {{")),
            "{once}"
        );
        let named = resilient(quote!(carrier, cancel = token), item).to_string();
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
            quote!(carrier, record_if = |_| held.counts()),
            quote!(carrier, fallback = stale),
        ] {
            let lent = resilient(lending.clone(), read_only.clone()).to_string();
            assert!(
                lent.contains("fn __nestrs_resilient_run () { } let _ = (& out , & held ,) ;")
                    && lent.matches("let _ =").count() == 1,
                "#[resilient({lending})] reads the arguments in the method's frame alone: {lent}"
            );
        }
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
            underscored.contains("move || async move { let _ = (& held ,) ; if false {"),
            "read first, `held` is captured ahead of the one named with a `_`: {underscored}"
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
            let attempt = judging.find(") , move ||").or(judging.find(") , ||"));
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
            "|| async { if false",
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
    fn a_parameter_under_a_cfg_is_handed_under_it() {
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
        let handed = resilient(quote!(carrier, fallback = stale), item).to_string();
        let answering = format!(
            "| __error | {{ let __handed_2 = parcel ; {weights} let __handed_1 = weight ; {zones} \
             let __handed_0 = zone ; self . stale ({zones} __handed_0 , {weights} __handed_1 , \
             __handed_2 , __error) }}"
        );
        assert!(handed.contains(&answering), "{answering} in {handed}");
    }

    /// The token is the closure's parameter where the one attempt owns what it
    /// reads, and a `let` of the attempt's block where attempts borrow: ahead
    /// of the return never taken, outside the block the body was written in.
    /// Only a body naming its token runs where one is made, and one naming
    /// none takes no parameter.
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
        for (lending, run) in [
            (
                quote!(carrier, idempotent, cancel = token),
                "run_idempotent_cancellable",
            ),
            (
                quote!(carrier, cancel = token, fallback = stale),
                "run_cancellable",
            ),
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
            assert!(lent.contains(&format!("{run} :: < Answer")), "{lent}");
        }
        for (keys, run) in [
            (quote!(carrier), "run"),
            (quote!(carrier, cancel = token), "run_cancellable"),
            (quote!(carrier, idempotent), "run_idempotent"),
            (
                quote!(carrier, idempotent, cancel = token),
                "run_idempotent_cancellable",
            ),
        ] {
            let expanded = resilient(keys.clone(), item.clone()).to_string();
            assert!(
                expanded.contains(&format!(
                    ":: __private :: {run} :: < Answer , _ , _ , _ , _ >"
                )),
                "#[resilient({keys})]: {expanded}"
            );
        }
    }

    /// Each `impl Trait` an answer holds is `_` wherever the expansion names
    /// the answer, however deep it sits.
    #[test]
    fn an_impl_trait_inside_the_answer_is_inferred_wherever_the_expansion_names_it() {
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
            let expanded = resilient(
                quote!(carrier, idempotent, cancel = stop),
                quote! {
                    async fn label(&self) -> #answer {
                        send(stop).await
                    }
                },
            )
            .to_string();
            for written in [
                format!("run_idempotent_cancellable :: < {inferred} , _ , _ , _ , _ >"),
                format!("PhantomData :: < {inferred} >"),
                format!("attempt :: < {inferred} > ()"),
            ] {
                assert!(expanded.contains(&written), "{written} in {expanded}");
            }
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
            (quote!(carrier), "move || async move {".to_owned()),
            (quote!(carrier, idempotent), "|| async {".to_owned()),
            (quote!(carrier, fallback = stale), "|| async {".to_owned()),
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
    }
}
