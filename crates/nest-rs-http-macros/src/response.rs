//! Per-handler response shapers: `#[http_code]`, `#[response_header]`, and
//! `#[redirect]`, markers consumed by `#[routes]`.

use nest_rs_codegen::{mixed_site_ident, site, takes_value, ungrouped_expr};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, quote};
use syn::punctuated::Punctuated;
use syn::{Attribute, Block, Expr, ExprLit, Lit, LitInt, LitStr, Token};

/// Header names that may repeat in one response (RFC 7230 §3.2.2), appended
/// rather than inserted.
fn is_multi_value_header(name: &str) -> bool {
    matches!(name, "set-cookie")
}

/// Whether `attr` is the shaper `name`, written bare (`#[http_code(201)]`) or
/// path-qualified (`#[nest_rs::http::http_code(201)]`).
pub(crate) fn is_shaper(attr: &Attribute, name: &str) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == name)
}

/// The entry point of every shaper, reached only when `#[routes]` did not
/// consume the marker, so it refuses. The item is kept beside the error so
/// nothing else about it is reported as missing.
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

    /// The success status this handler emits, for the OpenAPI document: a
    /// `#[redirect]`'s code (default `307`), else `#[http_code(N)]`'s, else `200`.
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

/// Drain and validate `#[http_code]`, `#[response_header]`, and `#[redirect]`
/// from the method attributes, so `HeaderName::from_static` cannot panic at boot.
/// `body` is the method's block, which `#[redirect]` requires empty.
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

    // RFC 7231 §7.1.2: `Location` is single-valued, and `#[redirect]` sets it.
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

    // `#[redirect]` never calls the method, so a body would silently vanish.
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

/// `#[http_code(201)]`'s one argument: a status code from 100 to 999.
///
/// Re-emitted unsuffixed: `201u8` is the right number and the wrong type for
/// `StatusCode::from_u16`.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
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

#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
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

/// RFC 7230 §3.2.6 token grammar, restricted to the lowercase subset
/// `HeaderName::from_static` accepts without panicking.
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

/// Redirect URL bytes: printable ASCII only, no whitespace; anything else
/// injects a header line or panics `HeaderValue::from_static` at boot.
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

/// `#[redirect(url[, status])]`, each position refused at what was written there.
#[expect(
    clippy::map_err_ignore,
    reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
)]
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

/// `#[redirect]`'s optional status: a `3xx` code, re-emitted unsuffixed as in
/// [`http_code_value`].
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

/// Expand a handler's response transformation into a
/// `poem::Result<Response>`. `call_expr` evaluates the user method;
/// `wrapper_args` lists every wrapper-fn parameter, silenced when `#[redirect]`
/// skips the call.
///
/// A failure keeps its own status: the overrides touch the success path only,
/// and which answers are failures is read by type through `nest_rs_core::Answer`,
/// since a spelling-based check misses an aliased `Result`.
pub(crate) fn apply_response_shapers(
    shapers: &ResponseShapers,
    call_expr: TokenStream2,
    wrapper_args: &[syn::Ident],
) -> TokenStream2 {
    // Mixed-site: these share a scope with the developer's extractor bindings.
    let out = mixed_site_ident("__out");
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

    quote! {
        {
            let #out = #call_expr;
            let mut #response: ::nest_rs_http::poem::Response = {
                use ::nest_rs_core::AnswerFallback as _;
                ::nest_rs_core::Answer(&#out).map::<::nest_rs_http::poem::Error, _, _>()(
                    #out,
                    ::nest_rs_http::poem::IntoResponse::into_response,
                )?
            };
            #status_apply
            #header_writes
            ::nest_rs_http::poem::Result::<::nest_rs_http::poem::Response>::Ok(#response)
        }
    }
}

/// Emit one header write per `#[response_header]`: `.insert()` overrides what
/// the handler set, `.append()` for the headers [`is_multi_value_header`] names.
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
