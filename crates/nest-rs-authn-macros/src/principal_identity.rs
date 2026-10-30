//! `#[derive(PrincipalIdentity)]` — a claims type's subject, tenant and scopes,
//! read off the fields `#[principal(…)]` marks.

use nest_rs_codegen::__private::takes_value;
use nest_rs_codegen::{Grammar, take_single_attr};
use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Fields, Ident, Member, Token};

/// The roles a field plays, each played by one field.
const PRINCIPAL: Grammar =
    Grammar::new("principal", &["actor", "tenant", "scopes"]).with_remedies(&[
        ("actor", "a principal names one subject"),
        ("tenant", "a principal acts within one tenant"),
        ("scopes", "a credential carries one scope list"),
    ]);

/// What a misplaced or empty `#[principal]` is told.
const NAMES_A_ROLE: &str = "#[principal] names the role its field plays — `#[principal(actor)]`, \
                            `#[principal(tenant)]` or `#[principal(scopes)]`";

pub(crate) fn principal_identity(input: TokenStream) -> TokenStream {
    expand(input).unwrap_or_else(syn::Error::into_compile_error)
}

/// A field a role reads: where it is, and its type's span, where a type the
/// role cannot read is refused.
///
/// Only the refusal takes the field's span: an expression spanned in the
/// developer's source is linted as their code, and clippy's `needless_borrow`
/// would refuse the probe's borrow of a reference field under `-D warnings`.
struct Marked {
    member: Member,
    span: Span,
}

/// The field each role reads, when a field plays it.
#[derive(Default)]
struct Roles {
    actor: Option<Marked>,
    tenant: Option<Marked>,
    scopes: Option<Marked>,
}

fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let item: DeriveInput = syn::parse2(input)?;
    if let Some(attr) = item
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("principal"))
    {
        return Err(syn::Error::new_spanned(
            attr,
            format!("{NAMES_A_ROLE}, written on that field rather than on the struct"),
        ));
    }
    let Data::Struct(data) = &item.data else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "#[derive(PrincipalIdentity)] goes on a struct — the claims a strategy authenticates — \
             whose fields `#[principal(…)]` marks",
        ));
    };
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[derive(PrincipalIdentity)] takes no generics: a strategy authenticates one concrete \
             principal, and an ambient value owns what it holds — write `PrincipalIdentity` and \
             `AmbientValue` by hand for a generic one",
        ));
    }
    let roles = roles(&data.fields)?;
    let Some(actor) = roles.actor else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "#[derive(PrincipalIdentity)] requires a field marked `#[principal(actor)]`: the \
             subject the verified credential names, an access token's `sub`; \
             `#[principal(tenant)]` and `#[principal(scopes)]` are optional",
        ));
    };

    let ident = &item.ident;
    let actor = identifier(&actor);
    let tenant = roles.tenant.map(|tenant| {
        let read = identifier(&tenant);
        quote! {
            fn tenant_id(&self) -> ::std::option::Option<::std::string::String> {
                #read
            }
        }
    });
    let scopes = roles.scopes.map(|Marked { member, span }| {
        // A list, never an `Option`, so a credential sent without its scope
        // claim stays scope-aware; the annotation puts a refusal at the field.
        let list = quote_spanned!(span=> &[::std::string::String]);
        quote! {
            fn scopes(&self) -> ::std::option::Option<&[::std::string::String]> {
                let scopes: #list = &self.#member;
                ::std::option::Option::Some(scopes)
            }
        }
    });
    Ok(quote! {
        impl ::nest_rs_authn::PrincipalIdentity for #ident {
            fn actor_id(&self) -> ::std::option::Option<::std::string::String> {
                #actor
            }
            #tenant
            #scopes
        }

        impl ::nest_rs_core::AmbientValue for #ident {}
    })
}

/// The field each role reads, every role played once.
fn roles(fields: &Fields) -> syn::Result<Roles> {
    let mut marked: Vec<(Ident, Marked)> = Vec::new();
    for (index, field) in fields.iter().enumerate() {
        let mut attrs = field.attrs.clone();
        let Some(attr) = take_single_attr(&mut attrs, "principal")? else {
            continue;
        };
        let member = field.ident.clone().map_or_else(
            || Member::from(index),
            |mut ident| {
                ident.set_span(Span::call_site());
                Member::from(ident)
            },
        );
        let span = field.ty.span();
        for key in keys(&attr)? {
            let member = member.clone();
            marked.push((key, Marked { member, span }));
        }
    }
    PRINCIPAL.take_all(marked.iter().map(|(key, _)| key))?;

    let mut roles = Roles::default();
    for (key, field) in marked {
        let role = match key.unraw().to_string().as_str() {
            "actor" => &mut roles.actor,
            "tenant" => &mut roles.tenant,
            _ => &mut roles.scopes,
        };
        *role = Some(field);
    }
    Ok(roles)
}

/// The roles one `#[principal(…)]` names, each taking no value.
fn keys(attr: &Attribute) -> syn::Result<Vec<Ident>> {
    if !matches!(attr.meta, syn::Meta::List(_)) {
        return Err(syn::Error::new_spanned(attr, NAMES_A_ROLE));
    }
    let mut keys = Vec::new();
    PRINCIPAL.parse_attr(attr, |arg| {
        let rest = arg.input();
        if !(rest.is_empty() || rest.peek(Token![,])) {
            return Err(syn::Error::new(
                arg.ident().span(),
                takes_value(
                    "principal",
                    Some(arg.key()),
                    &format!(
                        "no value — the role is the field it is written on: \
                         `#[principal({})]`",
                        arg.key(),
                    ),
                ),
            ));
        }
        keys.push(arg.ident().clone());
        Ok(())
    })?;
    if keys.is_empty() {
        return Err(syn::Error::new_spanned(attr, NAMES_A_ROLE));
    }
    Ok(keys)
}

/// What an `actor` or `tenant` field answers, read by its type: the text its
/// value prints, or `None` for an optional field holding none.
fn identifier(field: &Marked) -> TokenStream {
    let Marked { member, span } = field;
    let answer = Ident::new("identifier", *span);
    quote! {
        {
            use ::nest_rs_authn::__private::IdentifierAnswer as _;
            (&&::nest_rs_authn::__private::IdentifierProbe(&self.#member)).#answer()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(item: TokenStream) -> String {
        expand(item).expect("the claims derive").to_string()
    }

    fn refused(item: TokenStream) -> String {
        match expand(item) {
            Ok(expanded) => panic!("expanded where it should refuse: {expanded}"),
            Err(refusal) => refusal.to_string(),
        }
    }

    #[test]
    fn each_marked_role_reads_its_field_and_both_impls_are_written() {
        let expanded = expanded(quote! {
            pub struct Claims {
                #[principal(actor)]
                pub sub: Subject,
                #[principal(tenant)]
                pub org_id: Uuid,
                #[principal(scopes)]
                pub scope: Vec<String>,
                pub roles: Vec<Role>,
            }
        });
        assert!(
            expanded.contains("impl :: nest_rs_authn :: PrincipalIdentity for Claims"),
            "{expanded}"
        );
        assert!(
            expanded.contains("impl :: nest_rs_core :: AmbientValue for Claims { }"),
            "{expanded}"
        );
        assert!(
            expanded.contains("IdentifierProbe (& self . sub)"),
            "{expanded}"
        );
        assert!(
            expanded.contains("fn tenant_id")
                && expanded.contains("IdentifierProbe (& self . org_id)"),
            "{expanded}"
        );
        assert!(
            expanded.contains("fn scopes")
                && expanded
                    .contains("let scopes : & [:: std :: string :: String] = & self . scope"),
            "{expanded}"
        );
        assert!(!expanded.contains("roles"), "{expanded}");
    }

    #[test]
    fn a_role_left_unmarked_keeps_the_traits_default() {
        let expanded = expanded(quote! {
            pub struct Member {
                #[principal(actor)]
                pub id: String,
            }
        });
        assert!(expanded.contains("fn actor_id"), "{expanded}");
        assert!(!expanded.contains("fn tenant_id"), "{expanded}");
        assert!(!expanded.contains("fn scopes"), "{expanded}");
    }

    #[test]
    fn a_tuple_field_is_read_by_its_position_and_may_play_two_roles() {
        let expanded = expanded(quote! {
            pub struct ApiKey(u32, #[principal(actor, tenant)] String);
        });
        assert_eq!(
            expanded.matches("IdentifierProbe (& self . 1)").count(),
            2,
            "{expanded}"
        );
    }

    #[test]
    fn a_missing_actor_is_refused_naming_every_role() {
        let refusal = refused(quote! {
            pub struct Claims {
                #[principal(tenant)]
                pub org_id: String,
            }
        });
        assert!(
            refusal.contains("requires a field marked `#[principal(actor)]`")
                && refusal
                    .contains("`#[principal(tenant)]` and `#[principal(scopes)]` are optional"),
            "{refusal}"
        );
        assert!(
            refused(quote!(
                pub struct Claims;
            ))
            .contains("requires a field marked"),
            "a struct with no field names no subject"
        );
    }

    #[test]
    fn an_unknown_role_is_refused_listing_the_three() {
        assert_eq!(
            refused(quote! {
                pub struct Claims {
                    #[principal(subject)]
                    pub sub: String,
                }
            }),
            "unknown #[principal] argument `subject`; expected `actor`, `tenant` or `scopes`",
        );
    }

    #[test]
    fn a_role_played_twice_is_refused_at_the_second() {
        let across = refused(quote! {
            pub struct Claims {
                #[principal(actor)]
                pub sub: String,
                #[principal(actor)]
                pub client_id: String,
            }
        });
        assert_eq!(
            across,
            "#[principal] takes at most one `actor` — a principal names one subject"
        );
        let within = refused(quote! {
            pub struct Claims {
                #[principal(actor, scopes, scopes)]
                pub sub: String,
            }
        });
        assert_eq!(
            within,
            "#[principal] takes at most one `scopes` — a credential carries one scope list"
        );
        let attribute = refused(quote! {
            pub struct Claims {
                #[principal(actor)]
                #[principal(tenant)]
                pub sub: String,
            }
        });
        assert!(
            attribute.starts_with("`#[principal]` is written twice here"),
            "{attribute}"
        );
    }

    #[test]
    fn a_role_takes_no_value() {
        for attribute in [
            quote!(#[principal(actor = sub)]),
            quote!(#[principal(actor(sub))]),
        ] {
            assert_eq!(
                refused(quote! {
                    pub struct Claims {
                        #attribute
                        pub sub: String,
                    }
                }),
                "#[principal] `actor` takes no value — the role is the field it is written on: \
                 `#[principal(actor)]`",
            );
        }
    }

    #[test]
    fn a_principal_naming_no_role_is_refused() {
        for attribute in [
            quote!(#[principal]),
            quote!(#[principal()]),
            quote!(#[principal = "actor"]),
        ] {
            let refusal = refused(quote! {
                pub struct Claims {
                    #attribute
                    pub sub: String,
                }
            });
            assert!(
                refusal.starts_with("#[principal] names the role its field plays"),
                "{refusal}"
            );
        }
        let on_the_struct = refused(quote! {
            #[principal(actor)]
            pub struct Claims {
                pub sub: String,
            }
        });
        assert!(
            on_the_struct.ends_with("written on that field rather than on the struct"),
            "{on_the_struct}"
        );
    }

    #[test]
    fn every_shape_but_a_concrete_struct_is_refused() {
        for (item, says) in [
            (
                quote!(
                    pub enum Caller {
                        User(String),
                    }
                ),
                "goes on a struct",
            ),
            (
                quote!(
                    pub union Caller {
                        id: u64,
                    }
                ),
                "goes on a struct",
            ),
            (
                quote!(
                    pub struct Claims<T> {
                        #[principal(actor)]
                        sub: T,
                    }
                ),
                "takes no generics",
            ),
            (
                quote!(
                    pub struct Claims<'a> {
                        #[principal(actor)]
                        sub: &'a str,
                    }
                ),
                "takes no generics",
            ),
        ] {
            let refusal = refused(item);
            assert!(refusal.contains(says), "{refusal}");
        }
    }
}
