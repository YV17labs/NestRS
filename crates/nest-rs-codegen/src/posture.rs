//! The access posture an operation declares, and the one place its mandatory-ness
//! is worded.
//!
//! `#[authorize(Action, Entity)]` / `#[public]` beside an operation is the only
//! greppable declaration of what a caller must be allowed to do. A transport
//! supplies only how its operations are spelled, and what `#[public]` buys on it:
//!
//! ```
//! # use nest_rs_codegen::{Posture, PostureRules};
//! # fn main() -> syn::Result<()> {
//! const POSTURE: PostureRules = PostureRules {
//!     operation: "#[subscribe_message]",
//!     transport: "WebSockets",
//!     public_means: "no gate and no mask — the guards bound beside it still run",
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
//! assert!(POSTURE.take(&mut undeclared).is_err());
//! # Ok(())
//! # }
//! ```
//!
//! GraphQL keeps its own parser for `#[authorize(Update, bind = ArtworksService)]`,
//! which no other transport can express.

use syn::punctuated::Punctuated;
use syn::{ImplItemFn, Meta, Path, Token};

use crate::attrs::take_flag_attr;

/// The sentence every site prints for a second `#[authorize(...)]`.
pub fn at_most_one_authorize(site: &str) -> String {
    format!("at most one `#[authorize(...)]` per {site}")
}

/// Why `id_arg` cannot be expressed anywhere but GraphQL: it renames the
/// argument GraphQL's `bind` synthesises.
pub const ID_ARG_UNSUPPORTED_BECAUSE: &str = "it renames the argument GraphQL's `bind = Service` synthesises, and no other \
     transport synthesises one";

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
    /// `#[public]` — deliberately ungated and unmasked. Guards bound beside the
    /// operation still run; this says the *posture* was decided, not skipped.
    Public,
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
}

/// One transport's vocabulary for the shared posture grammar.
pub struct PostureRules {
    /// The operation attribute as written — `"#[tool]"`, `"#[subscribe_message]"`.
    pub operation: &'static str,
    /// The transport as an operator reads it — `"MCP"`, `"WebSockets"`.
    pub transport: &'static str,
    /// What `#[public]` means on this transport, for the mandatory-posture
    /// refusal.
    pub public_means: &'static str,
    /// Why `bind = Service` cannot be expressed here — the half after the dash;
    /// [`posture_key_unsupported`] words the rest.
    pub bind_unsupported_because: &'static str,
}

impl PostureRules {
    /// Take the operation's declared posture off the method.
    ///
    /// Mandatory and fail-secure: an operation with no posture does not compile.
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
            (None, false) => Err(syn::Error::new_spanned(
                &method.sig.ident,
                posture_required(self.operation, self.public_means),
            )),
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
        operation: "#[subscribe_message]",
        transport: "WebSockets",
        public_means: "no gate and no mask",
        bind_unsupported_because: "a message takes one payload value",
    };

    fn method(tokens: proc_macro2::TokenStream) -> ImplItemFn {
        syn::parse2(tokens).expect("a method")
    }

    // The security-load-bearing case: silence is not a posture.
    #[test]
    fn an_operation_with_no_posture_does_not_compile() {
        let err = RULES
            .take(&mut method(quote!(
                async fn list(&self) {}
            )))
            .err()
            .expect("no posture must be refused");
        let msg = err.to_string();
        assert!(msg.contains("posture"), "{msg}");
        assert!(
            msg.contains("#[subscribe_message]"),
            "the refusal names the transport's own operation attribute: {msg}"
        );
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
