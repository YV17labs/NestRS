//! Per-handler response shapers: `#[http_code]`, `#[response_header]`, and
//! `#[redirect]`. These are **passthrough markers** consumed by `#[routes]` —
//! the proc-macro entries below expand to nothing, they exist only so rustc
//! recognizes the attribute name and so they have a documentation home.
//!
//! The actual response transformation is emitted by `#[routes]` around the
//! generated handler wrapper (see [`take_response_decorators`] and
//! [`apply_response_decorators`]).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Block, Expr, ExprLit, Lit, LitInt, LitStr};

/// Header names that legitimately appear multiple times in a single response
/// (per RFC 7230 §3.2.2). Decorators emit `.append()` for these so an
/// explicit `#[response_header("set-cookie", …)]` is additive, not
/// overriding. Everything else is single-valued and overrides via `.insert()`
/// — avoiding the duplicate-header footgun when the handler already set the
/// same name.
fn is_multi_value_header(name: &str) -> bool {
    matches!(name, "set-cookie")
}

/// Empty passthrough shared by every response-decorator entry point.
/// `#[routes]` consumes the attribute; if one survives to rustc (the
/// attribute is on something that is not a `#[routes]` method), this
/// expands to the original item unchanged so the error message blames
/// the missing `#[routes]`, not an unknown attribute.
pub(crate) fn passthrough(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// Parsed response decorators for one handler. All three are composable
/// except `http_code` and `redirect` (the latter sets the status itself).
#[derive(Default)]
pub(crate) struct ResponseDecorators {
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

impl ResponseDecorators {
    pub fn is_empty(&self) -> bool {
        self.http_code.is_none() && self.headers.is_empty() && self.redirect.is_none()
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
pub(crate) fn take_response_decorators(
    attrs: &mut Vec<Attribute>,
    body: &Block,
) -> syn::Result<ResponseDecorators> {
    let mut out = ResponseDecorators::default();

    while let Some(idx) = attrs.iter().position(|a| a.path().is_ident("http_code")) {
        if out.http_code.is_some() {
            return Err(syn::Error::new_spanned(
                &attrs[idx],
                "`#[http_code]` is allowed at most once per handler",
            ));
        }
        let attr = attrs.remove(idx);
        let lit = attr.parse_args::<LitInt>()?;
        let n: u16 = lit.base10_parse().map_err(|e| {
            syn::Error::new_spanned(&lit, format!("`#[http_code]` expects a u16: {e}"))
        })?;
        if !(100..=999).contains(&n) {
            return Err(syn::Error::new_spanned(
                &lit,
                "`#[http_code]` expects a status in 100..=999",
            ));
        }
        out.http_code = Some(lit);
    }

    while let Some(idx) = attrs
        .iter()
        .position(|a| a.path().is_ident("response_header"))
    {
        let attr = attrs.remove(idx);
        let (name, value) = parse_header_args(&attr)?;
        validate_header_name(&name)?;
        validate_header_value(&value)?;
        out.headers.push((name, value));
    }

    while let Some(idx) = attrs.iter().position(|a| a.path().is_ident("redirect")) {
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
    // the operator knows which decorator stole the call.
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

fn parse_header_args(attr: &Attribute) -> syn::Result<(LitStr, LitStr)> {
    use syn::Token;
    use syn::punctuated::Punctuated;
    let list: Punctuated<LitStr, Token![,]> = attr.parse_args_with(Punctuated::parse_terminated)?;
    let mut iter = list.into_iter();
    let name = iter.next().ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "`#[response_header]` expects two string literals: `name, value`",
        )
    })?;
    let value = iter.next().ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "`#[response_header]` expects two string literals: `name, value`",
        )
    })?;
    if iter.next().is_some() {
        return Err(syn::Error::new_spanned(
            attr,
            "`#[response_header]` accepts exactly two arguments: `name, value`",
        ));
    }
    Ok((name, value))
}

/// HTTP/1.1 header-name token grammar (RFC 7230 §3.2.6) restricted to the
/// lowercase subset accepted by `HeaderName::from_static` so `from_static`
/// cannot panic at boot. Empty names rejected.
fn validate_header_name(lit: &LitStr) -> syn::Result<()> {
    let s = lit.value();
    if s.is_empty() {
        return Err(syn::Error::new_spanned(
            lit,
            "`#[response_header]` header name cannot be empty",
        ));
    }
    for c in s.bytes() {
        let ok = matches!(c,
            b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'
        );
        if !ok {
            return Err(syn::Error::new_spanned(
                lit,
                "`#[response_header]` header name must be lowercase ASCII \
                 (a-z, 0-9, `-`, `_`)",
            ));
        }
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
                    "`#[redirect]` URL contains a non-printable-ASCII byte \
                     0x{b:02x}; percent-encode it or use ASCII (RFC 3986)"
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
                "`#[response_header]` header value must be printable ASCII \
                 (no CR/LF, no control bytes)",
            ));
        }
    }
    Ok(())
}

fn parse_redirect_args(attr: &Attribute) -> syn::Result<RedirectSpec> {
    use syn::Token;
    use syn::punctuated::Punctuated;
    let list: Punctuated<Expr, Token![,]> = attr.parse_args_with(Punctuated::parse_terminated)?;
    let mut iter = list.into_iter();
    let url_expr = iter.next().ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "`#[redirect]` expects a URL literal: `#[redirect(\"…\")]` \
             or `#[redirect(\"…\", 301)]`",
        )
    })?;
    let url = match url_expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => s,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "`#[redirect]` URL must be a string literal",
            ));
        }
    };
    // The URL ends up in the `Location` header; `HeaderValue::from_static`
    // will panic on any non-printable-ASCII byte. Validate at compile time so
    // boot cannot fail. RFC 3986 already requires URIs to be ASCII —
    // internationalized URLs must be percent-encoded by the caller.
    validate_redirect_url(&url)?;

    let code = match iter.next() {
        None => None,
        Some(expr) => {
            let lit = match expr {
                Expr::Lit(ExprLit {
                    lit: Lit::Int(i), ..
                }) => i,
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "`#[redirect]` status code must be an integer literal",
                    ));
                }
            };
            let n: u16 = lit.base10_parse().map_err(|e| {
                syn::Error::new_spanned(&lit, format!("`#[redirect]` status not a u16: {e}"))
            })?;
            if !(300..=399).contains(&n) {
                return Err(syn::Error::new_spanned(
                    &lit,
                    "`#[redirect]` status must be in 300..=399",
                ));
            }
            Some(lit)
        }
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

/// Expand a handler's response transformation. `call_expr` is the tokens that
/// evaluate the user method (e.g. `__ctrl.foo(a, b).await`); `wrapper_args`
/// lists every wrapper-fn parameter name including `__ctrl`, so a
/// `#[redirect]` body that skips the user call can still silence any
/// unused-variable warnings on its extractors. `returns_result` is `true`
/// when the user method's return type is a `Result<_, _>` — in that case
/// the emitted code short-circuits on `Err` so the original error status
/// (set by the error's `ResponseError`) survives and the `#[http_code]` /
/// `#[response_header]` overrides only touch the success path. The returned
/// tokens produce a `::poem::Result<::poem::Response>`.
pub(crate) fn apply_response_decorators(
    decorators: &ResponseDecorators,
    call_expr: TokenStream2,
    wrapper_args: &[syn::Ident],
    returns_result: bool,
) -> TokenStream2 {
    if let Some(redirect) = &decorators.redirect {
        let url = &redirect.url;
        let status_lit = match &redirect.code {
            Some(lit) => quote! { #lit },
            None => quote! { 307u16 },
        };
        let header_writes = headers_tokens(&decorators.headers);
        return quote! {
            {
                // The user method is not called — `#[redirect]` produces the
                // response itself; extractor arguments still resolve via
                // poem's normal pipeline (they are wrapper-fn parameters).
                // One tuple discard makes the "read but unused" intent explicit
                // at `cargo expand` time without N repetitive lines.
                let _ = (#(&#wrapper_args,)*);
                let mut __response: ::poem::Response =
                    ::poem::Response::builder()
                        .status(
                            ::poem::http::StatusCode::from_u16(#status_lit)
                                .expect("redirect status validated at compile time"),
                        )
                        .header(::poem::http::header::LOCATION, #url)
                        .finish();
                #header_writes
                ::poem::Result::<::poem::Response>::Ok(__response)
            }
        };
    }

    let status_apply = match &decorators.http_code {
        Some(lit) => quote! {
            __response.set_status(
                ::poem::http::StatusCode::from_u16(#lit)
                    .expect("status validated at compile time"),
            );
        },
        None => quote! {},
    };
    let header_writes = headers_tokens(&decorators.headers);

    // Bug 1 / Bug 5: matching the Result inside the wrapper keeps the
    // handler's error status (e.g. 403 via `ResponseError`) instead of
    // letting `#[http_code]` rewrite every response status — and avoids the
    // `Result<T, E>: IntoResponse` trait-bound when only `E: ResponseError`
    // (i.e. `From<E> for poem::Error`) is available.
    let unwrap_ok = if returns_result {
        quote! {
            let __ok = match __out {
                ::core::result::Result::Ok(v) => v,
                ::core::result::Result::Err(e) => {
                    return ::core::result::Result::Err(::core::convert::From::from(e));
                }
            };
        }
    } else {
        quote! { let __ok = __out; }
    };

    quote! {
        {
            let __out = #call_expr;
            #unwrap_ok
            let mut __response: ::poem::Response =
                ::poem::IntoResponse::into_response(__ok);
            #status_apply
            #header_writes
            ::poem::Result::<::poem::Response>::Ok(__response)
        }
    }
}

/// Emit one header write per `#[response_header]`. Single-valued headers
/// (the overwhelming majority — `Content-Type`, `Cache-Control`, `Location`,
/// …) use `.insert()` so the decorator overrides whatever the handler or an
/// `IntoResponse` impl already set, dodging the duplicate-header footgun.
/// Multi-value headers in `is_multi_value_header` (today: `Set-Cookie`) use
/// `.append()` so the decorator stacks instead of clobbering prior cookies.
fn headers_tokens(headers: &[(LitStr, LitStr)]) -> TokenStream2 {
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
            __response.headers_mut().#method(
                ::poem::http::HeaderName::from_static(#name),
                ::poem::http::HeaderValue::from_static(#value),
            );
        }
    });
    quote! { #(#writes)* }
}
