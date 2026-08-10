//! The access posture an operation declares, and the one place its mandatory-ness
//! is worded.
//!
//! `#[authorize(Action, Entity)]` / `#[public]` beside an operation is the only
//! greppable declaration of what a caller must be allowed to do — `CLAUDE.md`'s
//! *no authn/authz decision outside a guard*. Three transports emit a class gate
//! and a response mask from it (`#[tools]`, `#[messages]`, `#[operations]`), and
//! two of them parse exactly the same grammar, so that grammar and the refusal
//! that makes it mandatory live here rather than in each macro crate.
//!
//! A transport supplies only what is genuinely its own — how its operations are
//! spelled, and what `#[public]` buys on it:
//!
//! ```ignore
//! const POSTURE: PostureRules = PostureRules {
//!     operation: "#[subscribe_message]",
//!     public_means: "no gate and no mask — the guards bound beside it still run",
//!     bind_unsupported: "`bind = Service` is not available on WebSockets — …",
//! };
//!
//! let posture = POSTURE.take(method)?;
//! ```
//!
//! GraphQL keeps its own parser: `#[authorize(Update, bind = ArtworksService)]`
//! synthesises an id argument and an `Authorized<A, E>` proof, which no other
//! transport can express. Carrying that option here for two transports that
//! reject it would be the abstraction paying for a case it does not have.

use syn::punctuated::Punctuated;
use syn::{ImplItemFn, Meta, Path, Token};

use crate::attrs::take_flag_attr;

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
    /// What `#[public]` means on this transport, spliced into the
    /// mandatory-posture refusal so the developer reads the actual alternative
    /// rather than a generic one.
    pub public_means: &'static str,
    /// What to say when `bind = Service` is written on a transport that cannot
    /// express it. Stated rather than silently parsed and ignored.
    pub bind_unsupported: &'static str,
}

impl PostureRules {
    /// Take the operation's declared posture off the method.
    ///
    /// Mandatory and fail-secure: an operation the developer forgot to think
    /// about does not compile, instead of shipping ungated and unmasked. That is
    /// the whole reason this returns `Posture` rather than `Option<Posture>`.
    pub fn take(&self, method: &mut ImplItemFn) -> syn::Result<Posture> {
        let spec = self.take_authorize(method)?;
        let public = take_flag_attr(&mut method.attrs, "public");
        match (spec, public) {
            (Some(_), true) => Err(syn::Error::new_spanned(
                &method.sig.ident,
                "`#[authorize(...)]` and `#[public]` contradict — an operation is gated \
                 or public, not both",
            )),
            (Some(posture), false) => Ok(posture),
            (None, true) => Ok(Posture::Public),
            (None, false) => Err(syn::Error::new_spanned(
                &method.sig.ident,
                format!(
                    "every {operation} declares its access posture: \
                     `#[authorize(Action, Entity)]` (class-level gate + automatic response \
                     masking — e.g. `#[authorize(Read, users::Entity)]`) or `#[public]` \
                     ({public_means})",
                    operation = self.operation,
                    public_means = self.public_means,
                ),
            )),
        }
    }

    /// Parse and remove `#[authorize(Action, Entity)]` / `#[authorize(Action,
    /// Entity, unmasked)]`.
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
                "at most one `#[authorize(...)]` per operation",
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
                    return Err(syn::Error::new_spanned(value, self.bind_unsupported));
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
        public_means: "no gate and no mask",
        bind_unsupported: "`bind = Service` is not available here",
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
