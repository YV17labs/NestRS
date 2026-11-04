//! The access posture an operation declares, and the sentences that word it.
//!
//! `#[authorize(Action, Entity)]` / `#[public]` beside an operation is the only
//! greppable declaration of what a caller must be allowed to do. A transport
//! supplies only its name and why an operation of its binds no subject by id:
//!
//! ```
//! # use nest_rs_codegen::__private::{Posture, PostureRules};
//! # fn main() -> syn::Result<()> {
//! const POSTURE: PostureRules = PostureRules {
//!     transport: "WebSockets",
//!     bind_unsupported_because: "a message takes one payload value, not the …",
//! };
//! # let mut public: syn::ImplItemFn = syn::parse_quote! {
//! #     #[public]
//! #     async fn ping(&self) {}
//! # };
//! # let method = &mut public;
//!
//! let posture = POSTURE.take(method)?;
//! assert!(matches!(posture, Posture::Public));
//!
//! let mut undeclared: syn::ImplItemFn = syn::parse_quote! { async fn ping(&self) {} };
//! assert!(matches!(POSTURE.take(&mut undeclared)?, Posture::Undeclared));
//! # Ok(())
//! # }
//! ```
//!
//! An operation declaring neither leaves it to its guards, which an impl half
//! cannot see — the app's pool, its endpoint's — so the boot, not this grammar,
//! refuses one no guard covers, as it refuses such a route. GraphQL's posture
//! stays mandatory: `/graphql` admits a caller with no credential, so the
//! posture is what refuses a visitor there.
//!
//! GraphQL and HTTP keep their own parsers for
//! `#[authorize(Update, bind = ArtworksService)]`: a field synthesises its id
//! argument, a route reads its path's, and a message or a tool, taking one
//! payload value, has no id to bind.

use syn::punctuated::Punctuated;
use syn::{ImplItemFn, Meta, Path, Token};

use crate::attrs::take_flag_attr;
use crate::meta::SitePosture;

/// The sentence every site prints for a second `#[authorize(...)]`.
pub fn at_most_one_authorize(site: &str) -> String {
    format!("at most one `#[authorize(...)]` per {site}")
}

/// Why `id_arg` cannot be expressed anywhere but GraphQL: it renames the
/// argument GraphQL's `bind` synthesises — a route reads its subject's id off
/// its path instead.
pub const ID_ARG_UNSUPPORTED_BECAUSE: &str = "it renames the argument GraphQL's `bind = Service` synthesises, and no other \
     transport synthesises one";

/// The sentence an edge prints for `bind = Service` with no parameter to
/// receive the subject it loads.
pub const BIND_NEEDS_AUTHORIZED: &str = "`#[authorize(Action, bind = Service)]` needs a parameter of type \
     `Authorized<Action, E>` to receive the bound subject — the action in the type must match the \
     one in the attribute (e.g. `#[authorize(Update, bind = FilesService)]` ⇒ \
     `Authorized<Update, FileEntity>`)";

/// The sentence an edge prints for an `Authorized<A, E>` parameter no
/// `bind = Service` fills: the type carries a loaded row, never the posture.
pub const AUTHORIZED_NEEDS_BIND: &str = "an `Authorized<Action, E>` parameter receives the subject \
     `#[authorize(Action, bind = Service)]` loads, and this operation binds none — declare \
     `bind = Service` in its `#[authorize]`, or drop the parameter: the type carries the row, \
     the attribute the posture";

/// The sentence an edge prints when an operation declares both postures.
pub fn posture_contradiction() -> &'static str {
    "`#[authorize(...)]` and `#[public]` contradict — an operation is gated or public, not both"
}

/// The sentence an edge prints for an operation with no posture at all.
///
/// `operation` names what the edge calls one and `public_means` says what
/// `#[public]` costs there.
pub fn posture_required(operation: &str, public_means: &str) -> String {
    format!(
        "every {operation} declares its access posture: `#[authorize(Action, Entity)]` \
         (class-level gate + automatic response masking — e.g. \
         `#[authorize(Read, users::Entity)]`) or `#[public]` ({public_means})"
    )
}

/// The sentence a site prints for an `#[authorize(...)]` key it cannot express.
///
/// `key` is spelled **as the grammar spells it** — `unmasked`, `bind = Service`,
/// `id_arg = argument` — so one sentence serves a bare flag and a `key = value`
/// alike.
pub fn posture_key_unsupported(key: &str, site: &str, because: &str) -> String {
    format!("`{key}` is not available on {site} — {because}")
}

/// What an operation declared about who may call it.
#[expect(
    clippy::exhaustive_enums,
    reason = "tier 2: the `#[messages]` and `#[tools]` emitters match it, and a new posture \
              must reach each"
)]
pub enum Posture {
    /// `#[authorize(Action, Entity)]` — emit the class gate, and the response mask
    /// unless `unmasked` opted the return shape out of it.
    Authorize {
        /// The action marker the gate demands (`Read`, `Update`, …).
        action: Path,
        /// The entity the gate and the mask act on.
        entity: Path,
        /// `unmasked`: keep the gate, leave masking to the body. For a shape the
        /// value-level round-trip cannot see through (a cursor connection).
        unmasked: bool,
    },
    /// `#[public]` — a caller with no credential is admitted, ungated and
    /// unmasked. Guards bound beside the operation still run.
    Public,
    /// Neither: the guards covering the operation decide, and the boot refuses
    /// one no guard covers.
    Undeclared,
}

impl Posture {
    /// Whether this posture arms automatic response masking.
    pub fn masks(&self) -> bool {
        matches!(
            self,
            Self::Authorize {
                unmasked: false,
                ..
            }
        )
    }

    /// What a site's declaration says of this posture: `#[public]` is public,
    /// anything else gated.
    pub fn site_posture(&self) -> SitePosture {
        match self {
            Self::Public => SitePosture::Public,
            Self::Authorize { .. } | Self::Undeclared => SitePosture::Gated,
        }
    }

    /// Whether the operation states a posture of its own.
    pub fn is_declared(&self) -> bool {
        !matches!(self, Self::Undeclared)
    }
}

/// One transport's vocabulary for the shared posture grammar.
pub struct PostureRules {
    /// The transport as an operator reads it — `"MCP"`, `"WebSockets"`.
    pub transport: &'static str,
    /// Why `bind = Service` cannot be expressed here — the half after the dash;
    /// [`posture_key_unsupported`] words the rest.
    pub bind_unsupported_because: &'static str,
}

impl PostureRules {
    /// Take the operation's declared posture off the method;
    /// [`Posture::Undeclared`] when it states none.
    pub fn take(&self, method: &mut ImplItemFn) -> syn::Result<Posture> {
        let spec = self.take_authorize(method)?;
        let public = take_flag_attr(&mut method.attrs, "public")?;
        match (spec, public) {
            (Some(_), true) => Err(syn::Error::new_spanned(
                &method.sig.ident,
                posture_contradiction(),
            )),
            (Some(posture), false) => Ok(posture),
            (None, true) => Ok(Posture::Public),
            (None, false) => Ok(Posture::Undeclared),
        }
    }

    /// Parse and remove `#[authorize(Action, Entity)]` / `#[authorize(Action,
    /// Entity, unmasked)]`.
    #[expect(
        clippy::map_err_ignore,
        reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
    )]
    fn take_authorize(&self, method: &mut ImplItemFn) -> syn::Result<Option<Posture>> {
        let Some(pos) = method
            .attrs
            .iter()
            .position(|a| a.path().is_ident("authorize"))
        else {
            return Ok(None);
        };
        let attr = method.attrs.remove(pos);
        if method.attrs.iter().any(|a| a.path().is_ident("authorize")) {
            return Err(syn::Error::new_spanned(
                &attr,
                at_most_one_authorize("operation"),
            ));
        }

        let malformed = || {
            syn::Error::new_spanned(
                &attr,
                "expected `#[authorize(Action, Entity)]` — e.g. `#[authorize(Read, \
                 users::Entity)]`, optionally followed by `unmasked`",
            )
        };
        let args = attr
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|_| malformed())?;

        let mut positional: Vec<Path> = Vec::new();
        let mut unmasked = false;
        for meta in &args {
            match meta {
                Meta::Path(path) if path.is_ident("unmasked") => unmasked = true,
                Meta::Path(path) => positional.push(path.clone()),
                Meta::NameValue(value) if value.path.is_ident("bind") => {
                    return Err(syn::Error::new_spanned(
                        value,
                        posture_key_unsupported(
                            "bind = Service",
                            self.transport,
                            self.bind_unsupported_because,
                        ),
                    ));
                }
                Meta::NameValue(value) if value.path.is_ident("id_arg") => {
                    return Err(syn::Error::new_spanned(
                        value,
                        posture_key_unsupported(
                            "id_arg = argument",
                            self.transport,
                            ID_ARG_UNSUPPORTED_BECAUSE,
                        ),
                    ));
                }
                other => return Err(syn::Error::new_spanned(other, malformed())),
            }
        }

        let [action, entity] = positional.as_slice() else {
            return Err(malformed());
        };
        Ok(Some(Posture::Authorize {
            action: action.clone(),
            entity: entity.clone(),
            unmasked,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    const RULES: PostureRules = PostureRules {
        transport: "WebSockets",
        bind_unsupported_because: "a message takes one payload value",
    };

    fn method(tokens: proc_macro2::TokenStream) -> ImplItemFn {
        syn::parse2(tokens).expect("a method")
    }

    // Silence opens nothing: the guards covering it decide, and the boot refuses
    // an operation none covers.
    #[test]
    fn an_operation_with_no_posture_leaves_it_to_its_guards() {
        let posture = RULES
            .take(&mut method(quote!(
                async fn list(&self) {}
            )))
            .expect("no posture is the guards' to decide");
        assert!(matches!(posture, Posture::Undeclared));
        assert!(!posture.is_declared());
        assert!(!posture.masks());
        assert!(matches!(posture.site_posture(), SitePosture::Gated));
    }

    #[test]
    fn authorize_carries_action_entity_and_arms_the_mask() {
        let posture = RULES
            .take(&mut method(quote!(
                #[authorize(Read, users::Entity)]
                async fn list(&self) {}
            )))
            .expect("a well-formed posture");
        assert!(posture.masks());
        let Posture::Authorize { unmasked, .. } = posture else {
            panic!("expected the authorize posture");
        };
        assert!(!unmasked);
    }

    #[test]
    fn unmasked_keeps_the_gate_and_drops_the_mask() {
        let posture = RULES
            .take(&mut method(quote!(
                #[authorize(Read, users::Entity, unmasked)]
                async fn list(&self) {}
            )))
            .expect("a well-formed posture");
        assert!(
            !posture.masks(),
            "`unmasked` is the opt-out for a shape the round-trip cannot see through"
        );
    }

    #[test]
    fn public_is_a_posture_not_an_absence() {
        let posture = RULES
            .take(&mut method(quote!(
                #[public]
                async fn ping(&self) {}
            )))
            .expect("`#[public]` is a declared posture");
        assert!(matches!(posture, Posture::Public));
        assert!(!posture.masks());
    }

    #[test]
    fn declaring_both_is_a_contradiction() {
        let err = RULES
            .take(&mut method(quote!(
                #[authorize(Read, users::Entity)]
                #[public]
                async fn list(&self) {}
            )))
            .err()
            .expect("gated and public at once must be refused");
        assert!(err.to_string().contains("contradict"), "{err}");
    }

    // An option the transport cannot express is *said*, never parsed and dropped.
    #[test]
    fn an_unsupported_bind_names_itself() {
        let err = RULES
            .take(&mut method(quote!(
                #[authorize(Update, bind = UsersService)]
                async fn update(&self) {}
            )))
            .err()
            .expect("`bind` is not available here");
        assert!(err.to_string().contains("bind = Service"), "{err}");
    }
}
