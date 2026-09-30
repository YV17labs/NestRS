//! Per-handler response shapers: `#[http_code]`, `#[response_header]`, and
//! `#[redirect]`. These are **markers** consumed by `#[routes]` — the
//! proc-macro entries exist so rustc recognizes the attribute name, so they
//! have a documentation home, and so a marker nothing consumed is refused
//! ([`unread`]) rather than expanding to the item in silence.
//!
//! The actual response transformation is emitted by `#[routes]` around the
//! generated handler wrapper (see [`take_response_shapers`] and
//! [`apply_response_shapers`]).

use nest_rs_codegen::{mixed_site_ident, site, takes_value, ungrouped_expr};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::punctuated::Punctuated;
use syn::{Attribute, Block, Expr, ExprLit, Lit, LitInt, LitStr, Token};

/// Header names that legitimately appear multiple times in a single response
/// (per RFC 7230 §3.2.2). The shaper emits `.append()` for these so an
/// explicit `#[response_header("set-cookie", …)]` is additive, not
/// overriding. Everything else is single-valued and overrides via `.insert()`
/// — avoiding the duplicate-header footgun when the handler already set the
/// same name.
fn is_multi_value_header(name: &str) -> bool {
    matches!(name, "set-cookie")
}

/// Whether `attr` is the shaper `name`, written bare (`#[http_code(201)]`) or
/// path-qualified (`#[nest_rs::http::http_code(201)]`).
///
/// The last segment, because the three are exported attribute macros and a
/// path is a legitimate way to write one. Matching the bare ident alone let a
/// qualified shaper survive `#[routes]`, expand to nothing, and leave the route
/// answering `200` with no header and no `Location` — while OpenAPI documented
/// the same `200`.
pub(crate) fn is_shaper(attr: &Attribute, name: &str) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == name)
}

/// The entry point of every shaper — which runs **only** when `#[routes]` did
/// not consume the marker, since `#[routes]` expands first and removes each one
/// it reads. So reaching here is the refusal: the marker sits outside a
/// `#[routes]` impl, or under a name `#[routes]` cannot recognise (an import
/// alias), and in either case it would shape nothing. It used to return the
/// item unchanged, accepting any argument list and answering the route with
/// its default status. The item is kept beside the error so nothing else about
/// it is reported as missing.
pub(crate) fn unread(shaper: &str, input: TokenStream) -> TokenStream {
    let error = syn::Error::new(
        proc_macro2::Span::call_site(),
        format!(
            "`#[{shaper}]` shapes a handler's response and is read by `#[routes]`, which did not \
             read this one — it sits outside a `#[routes]` impl, or under a name `#[routes]` \
             cannot recognise (an import alias). Write it on a verb-tagged method of a \
             `#[routes]` impl as `#[{shaper}(…)]`, bare or path-qualified"
        ),
    )
    .to_compile_error();
    let input = TokenStream2::from(input);
    quote!(#error #input).into()
}

/// Parsed response shapers for one handler. All three are composable
/// except `http_code` and `redirect` (the latter sets the status itself).
#[derive(Default)]
pub(crate) struct ResponseShapers {
    pub http_code: Option<LitInt>,
    pub headers: Vec<(LitStr, LitStr)>,
    pub redirect: Option<RedirectSpec>,
}

pub(crate) struct RedirectSpec {
    pub url: LitStr,
    pub code: Option<LitInt>,
    /// The redirect attribute itself, kept for error spans.
    pub attr: Attribute,
}

impl ResponseShapers {
    pub(crate) fn is_empty(&self) -> bool {
        self.http_code.is_none() && self.headers.is_empty() && self.redirect.is_none()
    }

    /// The effective **success** status this handler emits, for the OpenAPI
    /// document (OAPI-O3): a `#[redirect]`'s code (default `307`), else a
    /// `#[http_code(N)]`'s `N`, else `200`. The literals are already validated
    /// by [`take_response_shapers`], so a parse fallback is unreachable but kept
    /// total.
    pub(crate) fn success_status(&self) -> u16 {
        if let Some(redirect) = &self.redirect {
            redirect
                .code
                .as_ref()
                .and_then(|c| c.base10_parse().ok())
                .unwrap_or(307)
        } else {
            self.http_code
                .as_ref()
                .and_then(|c| c.base10_parse().ok())
                .unwrap_or(200)
        }
    }
}

/// Drain `#[http_code]`, `#[response_header]`, and `#[redirect]` from the
/// method attributes, validating each. Compile-time validation: status codes
/// fall in `100..=999`, redirect codes in `300..=399`, header name characters
/// fit the HTTP token grammar (lowercase ASCII, digits, `-`), header value is
/// printable ASCII. Strict static checks fail the build before
/// `HeaderName::from_static` would panic at boot.
///
/// `body` is the decorated method's block — required for `#[redirect]`'s
/// empty-body check (the macro never calls the user method, so any
/// statements in the body are silently dropped — that is a footgun and
/// must fail the build).
pub(crate) fn take_response_shapers(
    attrs: &mut Vec<Attribute>,
    body: &Block,
) -> syn::Result<ResponseShapers> {
    let mut out = ResponseShapers::default();

    while let Some(idx) = attrs.iter().position(|a| is_shaper(a, "http_code")) {
        if out.http_code.is_some() {
            return Err(syn::Error::new_spanned(
                &attrs[idx],
                "`#[http_code]` is allowed at most once per handler",
            ));
        }
        let attr = attrs.remove(idx);
        out.http_code = Some(http_code_value(&attr)?);
    }

    while let Some(idx) = attrs.iter().position(|a| is_shaper(a, "response_header")) {
        let attr = attrs.remove(idx);
        let (name, value) = parse_header_args(&attr)?;
        validate_header_name(&name)?;
        validate_header_value(&value)?;
        out.headers.push((name, value));
    }

    while let Some(idx) = attrs.iter().position(|a| is_shaper(a, "redirect")) {
        if out.redirect.is_some() {
            return Err(syn::Error::new_spanned(
                &attrs[idx],
                "`#[redirect]` is allowed at most once per handler",
            ));
        }
        let attr = attrs.remove(idx);
        let spec = parse_redirect_args(&attr)?;
        out.redirect = Some(spec);
    }

    if let (Some(_), Some(r)) = (&out.http_code, &out.redirect) {
        return Err(syn::Error::new_spanned(
            &r.attr,
            "`#[redirect]` and `#[http_code]` are mutually exclusive — \
             `#[redirect]` sets the status itself",
        ));
    }

    // RFC 7231 §7.1.2: `Location` is single-valued. `#[redirect]` always sets
    // it; a `#[response_header("location", …)]` next to it would either
    // duplicate (the pre-`insert()` bug) or silently override (the new
    // default). Both are surprising — fail at compile time, with the span on
    // the redundant header.
    if out.redirect.is_some()
        && let Some((name_lit, _)) = out
            .headers
            .iter()
            .find(|(n, _)| n.value().eq_ignore_ascii_case("location"))
    {
        return Err(syn::Error::new_spanned(
            name_lit,
            "`#[response_header(\"location\", …)]` cannot be combined with \
             `#[redirect]` — the redirect URL already sets the Location header",
        ));
    }

    // `#[redirect]` produces the response itself — the user method is never
    // called, so any side-effect work inside the body silently disappears.
    // Reject a non-empty body at compile time, naming the redirect URL so
    // the operator knows which redirect attribute stole the call.
    if let Some(spec) = &out.redirect
        && !body.stmts.is_empty()
    {
        let url = spec.url.value();
        return Err(syn::Error::new_spanned(
            body,
            format!(
                "`#[redirect({url:?})]` handlers must have an empty body — \
                 the method is not called, only the redirect URL is sent. \
                 Move side-effect work into a service the user is redirected to, \
                 or drop the body to opt in."
            ),
        ));
    }

    Ok(out)
}

/// `#[http_code(201)]`'s one argument: a status code, refused in one sentence
/// whatever was written instead — a string, a number outside the three-digit
/// range, a literal too large for a `u16`, or nothing at all.
///
/// Re-emitted unsuffixed: `201u8` is the right number and the wrong type for
/// `StatusCode::from_u16`, and the value checked is the value sent.
fn http_code_value(attr: &Attribute) -> syn::Result<LitInt> {
    let refused = |at: &dyn ToTokens| {
        syn::Error::new_spanned(
            at,
            takes_value(
                "http_code",
                None,
                "a status code from 100 to 999, e.g. `#[http_code(201)]`",
            ),
        )
    };
    let written: Expr = attr.parse_args().map_err(|_| refused(attr))?;
    let Expr::Lit(ExprLit {
        lit: Lit::Int(lit), ..
    }) = ungrouped_expr(&written)
    else {
        return Err(refused(ungrouped_expr(&written)));
    };
    match lit.base10_parse::<u16>() {
        Ok(code) if (100..=999).contains(&code) => Ok(LitInt::new(&code.to_string(), lit.span())),
        _ => Err(refused(lit)),
    }
}

/// What `#[response_header]`'s `name` takes — the lowercase subset of the
/// header-name token grammar that `HeaderName::from_static` accepts.
const HEADER_NAME_TAKES: &str = "a lowercase header name of `a`-`z`, `0`-`9`, `-` and `_`, e.g. \
     \"cache-control\" — the form `HeaderName::from_static` accepts";

/// What `#[response_header]`'s `value` takes.
const HEADER_VALUE_TAKES: &str = "printable ASCII or a tab, e.g. \"no-store\" — a CR, an LF or \
     another control byte would split or corrupt the header";

fn parse_header_args(attr: &Attribute) -> syn::Result<(LitStr, LitStr)> {
    let two = || {
        syn::Error::new_spanned(
            attr,
            "`#[response_header]` expects two string literals: `name, value`",
        )
    };
    let list: Punctuated<Expr, Token![,]> = attr
        .parse_args_with(Punctuated::parse_terminated)
        .map_err(|_| two())?;
    let mut iter = list.iter();
    let (Some(name), Some(value)) = (iter.next(), iter.next()) else {
        return Err(two());
    };
    if iter.next().is_some() {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[response_header]` accepts exactly two arguments: `name, value`",
        ));
    }
    Ok((
        header_literal(name, "name", HEADER_NAME_TAKES)?,
        header_literal(value, "value", HEADER_VALUE_TAKES)?,
    ))
}

/// One of `#[response_header]`'s two positions, read as a string literal.
fn header_literal(written: &Expr, position: &str, what: &str) -> syn::Result<LitStr> {
    match ungrouped_expr(written) {
        Expr::Lit(ExprLit {
            lit: Lit::Str(literal),
            ..
        }) => Ok(literal.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            takes_value("response_header", Some(position), what),
        )),
    }
}

/// HTTP/1.1 header-name token grammar (RFC 7230 §3.2.6) restricted to the
/// lowercase subset accepted by `HeaderName::from_static` so `from_static`
/// cannot panic at boot. Empty names rejected.
fn validate_header_name(lit: &LitStr) -> syn::Result<()> {
    let s = lit.value();
    let accepted = |c: u8| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_');
    if s.is_empty() || !s.bytes().all(accepted) {
        return Err(syn::Error::new_spanned(
            lit,
            takes_value("response_header", Some("name"), HEADER_NAME_TAKES),
        ));
    }
    Ok(())
}

/// Redirect URL bytes: printable ASCII only (0x21-0x7E), no whitespace. Any
/// non-printable byte (CR/LF/NUL, control char, or ≥0x80) would either inject
/// a header line or panic `HeaderValue::from_static` at boot. Internationalized
/// URLs must be percent-encoded by the caller (RFC 3986).
fn validate_redirect_url(lit: &LitStr) -> syn::Result<()> {
    for b in lit.value().bytes() {
        if !(0x21..=0x7e).contains(&b) {
            return Err(syn::Error::new_spanned(
                lit,
                format!(
                    "{}: byte 0x{b:02x} is not printable ASCII — the URL is sent as the \
                     `Location` header, so percent-encode it (RFC 3986)",
                    site("redirect", Some("url")),
                ),
            ));
        }
    }
    Ok(())
}

/// Header values: printable ASCII plus tab; reject CR/LF (header injection).
fn validate_header_value(lit: &LitStr) -> syn::Result<()> {
    let s = lit.value();
    for c in s.bytes() {
        let ok = c == b'\t' || (0x20..=0x7e).contains(&c);
        if !ok {
            return Err(syn::Error::new_spanned(
                lit,
                takes_value("response_header", Some("value"), HEADER_VALUE_TAKES),
            ));
        }
    }
    Ok(())
}

/// `#[redirect(url[, status])]`: each position refused in its own sentence, at
/// what was written there — the URL when it is missing or not a string, the
/// status when it is not a redirect status.
fn parse_redirect_args(attr: &Attribute) -> syn::Result<RedirectSpec> {
    let url_refused = |at: &dyn ToTokens| {
        syn::Error::new_spanned(
            at,
            takes_value(
                "redirect",
                Some("url"),
                "the target as a string literal, e.g. `#[redirect(\"/login\")]` or \
                 `#[redirect(\"/login\", 301)]`",
            ),
        )
    };
    let list: Punctuated<Expr, Token![,]> = attr
        .parse_args_with(Punctuated::parse_terminated)
        .map_err(|_| url_refused(attr))?;
    let mut iter = list.iter();
    let url = match iter.next().map(ungrouped_expr) {
        Some(Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        })) => s.clone(),
        Some(other) => return Err(url_refused(other)),
        None => return Err(url_refused(attr)),
    };
    // The URL ends up in the `Location` header; `HeaderValue::from_static`
    // will panic on any non-printable-ASCII byte. Validate at compile time so
    // boot cannot fail. RFC 3986 already requires URIs to be ASCII —
    // internationalized URLs must be percent-encoded by the caller.
    validate_redirect_url(&url)?;

    let code = match iter.next() {
        None => None,
        Some(written) => Some(redirect_status(written)?),
    };

    if iter.next().is_some() {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[redirect]` accepts at most two arguments: `url[, status]`",
        ));
    }

    Ok(RedirectSpec {
        url,
        code,
        attr: attr.clone(),
    })
}

/// `#[redirect]`'s optional status: a `3xx` code, one sentence for anything
/// else, re-emitted unsuffixed for the reason [`http_code_value`] gives.
fn redirect_status(written: &Expr) -> syn::Result<LitInt> {
    let written = ungrouped_expr(written);
    let refused = || {
        syn::Error::new_spanned(
            written,
            takes_value(
                "redirect",
                Some("status"),
                "a redirect status from 300 to 399, e.g. `#[redirect(\"/login\", 301)]`",
            ),
        )
    };
    let Expr::Lit(ExprLit {
        lit: Lit::Int(lit), ..
    }) = written
    else {
        return Err(refused());
    };
    match lit.base10_parse::<u16>() {
        Ok(code) if (300..=399).contains(&code) => Ok(LitInt::new(&code.to_string(), lit.span())),
        _ => Err(refused()),
    }
}

/// Expand a handler's response transformation. `call_expr` is the tokens that
/// evaluate the user method (e.g. `__ctrl.foo(a, b).await`); `wrapper_args`
/// lists every wrapper-fn parameter name including `__ctrl`, so a
/// `#[redirect]` body that skips the user call can still silence any
/// unused-variable warnings on its extractors. `returns_result` is `true`
/// when the user method's return type is a `Result<_, _>` — in that case
/// the emitted code short-circuits on `Err` so the original error status
/// (set by the error's `ResponseError`) survives and the `#[http_code]` /
/// `#[response_header]` overrides only touch the success path. The returned
/// tokens produce a `::nest_rs_http::poem::Result<::nest_rs_http::poem::Response>`.
pub(crate) fn apply_response_shapers(
    shapers: &ResponseShapers,
    call_expr: TokenStream2,
    wrapper_args: &[syn::Ident],
    returns_result: bool,
) -> TokenStream2 {
    // Bound in the same scope as the developer's extractor bindings, so they
    // take the same definition-site hygiene the wrapper's own locals do — see
    // the `mixed_site_ident` note in `routes.rs`. Safe by span, not by the
    // order these statements happen to be emitted in.
    let out = mixed_site_ident("__out");
    let ok = mixed_site_ident("__ok");
    let response = mixed_site_ident("__response");

    if let Some(redirect) = &shapers.redirect {
        let url = &redirect.url;
        let status_lit = match &redirect.code {
            Some(lit) => quote! { #lit },
            None => quote! { 307u16 },
        };
        let header_writes = headers_tokens(&shapers.headers, &response);
        return quote! {
            {
                // The user method is not called — `#[redirect]` produces the
                // response itself; extractor arguments still resolve via
                // poem's normal pipeline (they are wrapper-fn parameters).
                // One tuple discard makes the "read but unused" intent explicit
                // at `cargo expand` time without N repetitive lines.
                let _ = (#(&#wrapper_args,)*);
                let mut #response: ::nest_rs_http::poem::Response =
                    ::nest_rs_http::poem::Response::builder()
                        .status(
                            ::nest_rs_http::poem::http::StatusCode::from_u16(#status_lit)
                                .expect("redirect status validated at compile time"),
                        )
                        .header(::nest_rs_http::poem::http::header::LOCATION, #url)
                        .finish();
                #header_writes
                ::nest_rs_http::poem::Result::<::nest_rs_http::poem::Response>::Ok(#response)
            }
        };
    }

    let status_apply = match &shapers.http_code {
        Some(lit) => quote! {
            #response.set_status(
                ::nest_rs_http::poem::http::StatusCode::from_u16(#lit)
                    .expect("status validated at compile time"),
            );
        },
        None => quote! {},
    };
    let header_writes = headers_tokens(&shapers.headers, &response);

    // Bug 1 / Bug 5: matching the Result inside the wrapper keeps the
    // handler's error status (e.g. 403 via `ResponseError`) instead of
    // letting `#[http_code]` rewrite every response status — and avoids the
    // `Result<T, E>: IntoResponse` trait-bound when only `E: ResponseError`
    // (i.e. `From<E> for poem::Error`) is available.
    let unwrap_ok = if returns_result {
        quote! {
            let #ok = match #out {
                ::core::result::Result::Ok(v) => v,
                ::core::result::Result::Err(e) => {
                    return ::core::result::Result::Err(::core::convert::From::from(e));
                }
            };
        }
    } else {
        quote! { let #ok = #out; }
    };

    quote! {
        {
            let #out = #call_expr;
            #unwrap_ok
            let mut #response: ::nest_rs_http::poem::Response =
                ::nest_rs_http::poem::IntoResponse::into_response(#ok);
            #status_apply
            #header_writes
            ::nest_rs_http::poem::Result::<::nest_rs_http::poem::Response>::Ok(#response)
        }
    }
}

/// Emit one header write per `#[response_header]`. Single-valued headers
/// (the overwhelming majority — `Content-Type`, `Cache-Control`, `Location`,
/// …) use `.insert()` so the shaper overrides whatever the handler or an
/// `IntoResponse` impl already set, dodging the duplicate-header footgun.
/// Multi-value headers in `is_multi_value_header` (today: `Set-Cookie`) use
/// `.append()` so the shaper stacks instead of clobbering prior cookies.
fn headers_tokens(headers: &[(LitStr, LitStr)], response: &syn::Ident) -> TokenStream2 {
    if headers.is_empty() {
        return quote! {};
    }
    let writes = headers.iter().map(|(name, value)| {
        let method = if is_multi_value_header(&name.value()) {
            quote! { append }
        } else {
            quote! { insert }
        };
        quote! {
            #response.headers_mut().#method(
                ::nest_rs_http::poem::http::HeaderName::from_static(#name),
                ::nest_rs_http::poem::http::HeaderValue::from_static(#value),
            );
        }
    });
    quote! { #(#writes)* }
}
