//! The access posture an operation declares, and the sentences that word it.
//!
//! `#[authorize(Action, Subject)]`, `#[public]` and `#[public(Action, Subject)]`
//! beside an operation are the only greppable declaration of what a caller must
//! be allowed to do. A transport supplies only its name and why an operation of
//! its binds no subject by id:
//!
//! ```
//! # use nest_rs_codegen::__private::{Posture, PostureRules};
//! # fn main() -> syn::Result<()> {
//! const POSTURE: PostureRules = PostureRules {
//!     transport: "WebSockets",
//!     bind_unsupported_because: Some("a message takes one payload value, not the …"),
//! };
//! # let mut public: syn::ImplItemFn = syn::parse_quote! {
//! #     #[public]
//! #     async fn ping(&self) {}
//! # };
//! # let method = &mut public;
//!
//! let posture = POSTURE.take(method)?;
//! assert!(matches!(posture, Posture::Public(None)));
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
//! argument, a route reads its path's. A `#[resource]` reads its one template
//! variable through this one, and a message or a tool, taking one payload value,
//! has no id to bind.

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, FnArg, Ident, ImplItemFn, Meta, Path, Signature, Token, Type};

use crate::args::{duplicate_argument, takes_value};
use crate::attrs::take_single_attr;
use crate::meta::SitePosture;
use crate::ty::nth_generic_type;

/// The sentence every site prints for a second `#[authorize(...)]`.
fn at_most_one_authorize(site: &str) -> String {
    format!("at most one `#[authorize(...)]` per {site}")
}

/// Why `id_arg` cannot be expressed anywhere but GraphQL: it renames the
/// argument GraphQL's `bind` synthesises — a route reads its subject's id off
/// its path instead.
pub const ID_ARG_UNSUPPORTED_BECAUSE: &str = "it renames the argument GraphQL's `bind = Service` synthesises, and no other \
     transport synthesises one";

/// What `bind` takes, for the sentence an edge prints for any other value.
pub const BIND_TAKES: &str =
    "the path of the service that loads the subject, e.g. `bind = FilesService`";

/// The sentence an edge prints for `bind = Service` with no parameter to
/// receive the subject it loads.
pub const BIND_NEEDS_AUTHORIZED: &str = "`bind = Service` needs a parameter of type \
     `Authorized<Action, E>` to receive the bound subject — the action in the type must match the \
     one in the attribute (e.g. `#[authorize(Update, bind = FilesService)]` ⇒ \
     `Authorized<Update, FileEntity>`)";

/// The sentence an edge prints for an `Authorized<A, E>` parameter no
/// `bind = Service` fills: the type carries a loaded row, never the posture.
pub const AUTHORIZED_NEEDS_BIND: &str = "an `Authorized<Action, E>` parameter receives the subject \
     `bind = Service` loads, and this operation binds none — declare `bind = Service` in its \
     posture, or drop the parameter: the type carries the row, the attribute the posture";

/// The sentence an edge prints when an operation declares both postures.
fn posture_contradiction() -> &'static str {
    "`#[authorize(...)]` and `#[public]` contradict — an operation is gated or public, not both"
}

/// The sentence a site prints for an `#[authorize(...)]` key it cannot express.
///
/// `key` is spelled **as the grammar spells it** — `unmasked`, `bind = Service`,
/// `id_arg = argument` — so one sentence serves a bare flag and a `key = value`
/// alike.
pub fn posture_key_unsupported(key: &str, site: &str, because: &str) -> String {
    format!("`{key}` is not available on {site} — {because}")
}

/// What a posture naming a subject gates on: the class gate, and the response
/// mask an entity arms unless `unmasked` opted the return shape out of it.
pub struct PostureGate {
    /// The action marker the gate demands (`Read`, `Update`, …).
    pub action: Path,
    /// The subject the gate and the mask act on — under `bind` alone, the one
    /// the `Authorized<Action, E>` parameter names.
    pub entity: Path,
    /// `unmasked`: keep the gate, leave masking to the body. For a shape the
    /// value-level round-trip cannot see through (a cursor connection).
    pub unmasked: bool,
    /// `bind = Service`: the service that loads the subject into the
    /// operation's `Authorized<Action, E>` parameter.
    pub bind: Option<Path>,
}

/// What an operation declared about who may call it.
#[expect(
    clippy::exhaustive_enums,
    reason = "tier 2: the `#[messages]` and `#[tools]` emitters match it, and a new posture \
              must reach each"
)]
pub enum Posture {
    /// `#[authorize(Action, Subject)]` — a caller a principal backs, gated.
    Authorize(PostureGate),
    /// `#[public]` — a caller with no credential is admitted, and guards bound
    /// beside the operation still run. Naming a subject (`#[public(Action,
    /// Subject)]`) gates and masks as `#[authorize]` does, on the ability that
    /// caller has; bare, it reads nothing a grant guards.
    Public(Option<PostureGate>),
    /// Neither: the guards covering the operation decide, and the boot refuses
    /// one no guard covers.
    Undeclared,
}

impl Posture {
    /// What the posture gates on, where it names a subject.
    pub fn gate(&self) -> Option<&PostureGate> {
        match self {
            Self::Authorize(gate) | Self::Public(Some(gate)) => Some(gate),
            Self::Public(None) | Self::Undeclared => None,
        }
    }

    /// The attribute the posture was written with, for a refusal to name;
    /// `authorize` for one that states none, whose refusal offers it.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Public(_) => "public",
            Self::Authorize(_) | Self::Undeclared => "authorize",
        }
    }

    /// Whether this posture arms automatic response masking.
    pub fn masks(&self) -> bool {
        self.gate().is_some_and(|gate| !gate.unmasked)
    }

    /// What a site's declaration says of this posture: `#[public]` is public,
    /// anything else gated.
    pub fn site_posture(&self) -> SitePosture {
        match self {
            Self::Public(_) => SitePosture::Public,
            Self::Authorize(_) | Self::Undeclared => SitePosture::Gated,
        }
    }

    /// Whether the operation states a posture of its own.
    pub fn is_declared(&self) -> bool {
        !matches!(self, Self::Undeclared)
    }

    /// Whether what the unit's reads run under is withheld from them, as a
    /// `bool` expression: true where no mask shapes its reply and the posture
    /// leaves that to no body. An entity arms the mask, `unmasked` leaves it to
    /// the body, and a `#[subject]` has no rows to mask, which only its type
    /// says.
    pub fn withheld(&self) -> TokenStream {
        withheld(self.gate().map(|gate| (&gate.entity, gate.unmasked)))
    }
}

/// [`Posture::withheld`], for an edge parsing its own posture: `named` is the
/// subject it names and whether it declares `unmasked`.
pub fn withheld(named: Option<(impl quote::ToTokens, bool)>) -> TokenStream {
    match named {
        None => quote!(true),
        Some((_, true)) => quote!(false),
        Some((subject, false)) => quote!(!<#subject as ::nest_rs_authz::Subject>::__ROWS),
    }
}

/// Take a method's posture attributes off `attrs`, every edge's one reading: what
/// it gates on, which `gate_of` reads off an attribute naming a subject and the
/// word that attribute is written with, and whether it is `#[public]`, named or
/// bare. `site` is the noun a repeated `#[authorize]` is refused with, and `method`
/// is where a contradiction is spanned.
pub fn take_posture<G>(
    attrs: &mut Vec<Attribute>,
    method: &Ident,
    site: &str,
    gate_of: impl Fn(&Attribute, &'static str) -> syn::Result<G>,
) -> syn::Result<(Option<G>, bool)> {
    let is_authorize = |attr: &Attribute| attr.path().is_ident("authorize");
    let authorize = match attrs.iter().position(is_authorize) {
        Some(at) => {
            let attr = attrs.remove(at);
            if attrs.iter().any(is_authorize) {
                return Err(syn::Error::new_spanned(&attr, at_most_one_authorize(site)));
            }
            Some(gate_of(&attr, "authorize")?)
        }
        None => None,
    };
    match (authorize, take_single_attr(attrs, "public")?) {
        (Some(_), Some(_)) => Err(syn::Error::new_spanned(method, posture_contradiction())),
        (Some(gate), None) => Ok((Some(gate), false)),
        (None, Some(attr)) if matches!(attr.meta, Meta::Path(_)) => Ok((None, true)),
        (None, Some(attr)) => Ok((Some(gate_of(&attr, "public")?), true)),
        (None, None) => Ok((None, false)),
    }
}

/// One transport's vocabulary for the shared posture grammar.
pub struct PostureRules {
    /// The transport as an operator reads it — `"MCP"`, `"WebSockets"`.
    pub transport: &'static str,
    /// Why `bind = Service` cannot be expressed here — the half after the dash,
    /// [`posture_key_unsupported`] wording the rest — or `None` where the
    /// operation reads the one id the binding needs.
    pub bind_unsupported_because: Option<&'static str>,
}

impl PostureRules {
    /// Take the operation's declared posture off the method;
    /// [`Posture::Undeclared`] when it states none.
    pub fn take(&self, method: &mut ImplItemFn) -> syn::Result<Posture> {
        let sig = &method.sig;
        let (gate, public) =
            take_posture(&mut method.attrs, &sig.ident, "operation", |attr, word| {
                self.gate_of(attr, word, sig)
            })?;
        Ok(match (gate, public) {
            (gate, true) => Posture::Public(gate),
            (Some(gate), false) => Posture::Authorize(gate),
            (None, false) => Posture::Undeclared,
        })
    }

    /// Read `#[word(Action, Subject)]` / `#[word(Action, Subject, unmasked)]`,
    /// and `#[word(Action, bind = Service)]` where the operation reads an id.
    #[expect(
        clippy::map_err_ignore,
        reason = "the refusal names the grammar the decorator accepts; syn's own message would name a token"
    )]
    fn gate_of(&self, attr: &Attribute, word: &str, sig: &Signature) -> syn::Result<PostureGate> {
        let malformed = || {
            let expected = match self.bind_unsupported_because {
                Some(_) => format!(
                    "expected `#[{word}(Action, Subject)]` — e.g. `#[{word}(Read, \
                     users::Entity)]`, optionally followed by `unmasked`"
                ),
                None => format!(
                    "expected `#[{word}(Action, Subject)]` — e.g. `#[{word}(Read, \
                     users::Entity)]`, or `#[{word}(Action, bind = Service)]`, which loads the \
                     row its one variable names and lets the entity be omitted — optionally \
                     followed by `unmasked`"
                ),
            };
            syn::Error::new_spanned(attr, expected)
        };
        let args = attr
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|_| malformed())?;

        let mut positional: Vec<Path> = Vec::new();
        let mut unmasked = false;
        let mut bind: Option<Path> = None;
        for meta in &args {
            match meta {
                Meta::Path(path) if path.is_ident("unmasked") => unmasked = true,
                Meta::Path(path) => positional.push(path.clone()),
                Meta::NameValue(value) if value.path.is_ident("bind") => {
                    if let Some(because) = self.bind_unsupported_because {
                        return Err(syn::Error::new_spanned(
                            value,
                            posture_key_unsupported("bind = Service", self.transport, because),
                        ));
                    }
                    if bind.is_some() {
                        return Err(syn::Error::new_spanned(
                            value,
                            duplicate_argument(word, "bind"),
                        ));
                    }
                    let Expr::Path(service) = &value.value else {
                        return Err(syn::Error::new_spanned(
                            &value.value,
                            takes_value(word, Some("bind"), BIND_TAKES),
                        ));
                    };
                    bind = Some(service.path.clone());
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

        // Under `bind` the written entity is optional: the `Authorized<Action, E>`
        // parameter names it, and the expansion asserts it the service's.
        let (action, entity) = match (positional.as_slice(), &bind) {
            ([action, entity], _) => (action.clone(), entity.clone()),
            ([action], Some(_)) => {
                let Some(Type::Path(entity)) = authorized_entity(&sig.inputs) else {
                    return Err(syn::Error::new_spanned(&sig.ident, BIND_NEEDS_AUTHORIZED));
                };
                (action.clone(), entity.path.clone())
            }
            _ => return Err(malformed()),
        };
        Ok(PostureGate {
            action,
            entity,
            unmasked,
            bind,
        })
    }
}

/// The `E` of the `Authorized<Action, E>` parameter among `inputs`, the entity a
/// posture naming none under `bind` reads.
pub fn authorized_entity<'a>(inputs: impl IntoIterator<Item = &'a FnArg>) -> Option<&'a Type> {
    inputs.into_iter().find_map(|arg| match arg {
        FnArg::Typed(pt) => nth_generic_type(&pt.ty, "Authorized", 1),
        FnArg::Receiver(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    const RULES: PostureRules = PostureRules {
        transport: "WebSockets",
        bind_unsupported_because: Some("a message takes one payload value"),
    };

    const BINDING: PostureRules = PostureRules {
        bind_unsupported_because: None,
        ..RULES
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
        let Posture::Authorize(PostureGate { unmasked, .. }) = posture else {
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
        assert!(matches!(posture, Posture::Public(None)));
        assert!(!posture.masks());
        assert!(matches!(posture.site_posture(), SitePosture::Public));
    }

    #[test]
    fn public_naming_a_subject_gates_and_masks_as_authorize_does() {
        let posture = RULES
            .take(&mut method(quote!(
                #[public(Read, posts::Entity)]
                async fn feed(&self) {}
            )))
            .expect("a well-formed opening");
        assert!(posture.masks());
        assert!(posture.gate().is_some());
        assert!(matches!(posture.site_posture(), SitePosture::Public));
    }

    #[test]
    fn public_keeps_the_grammar_authorize_takes() {
        let posture = BINDING
            .take(&mut method(quote!(
                #[public(Read, bind = PostsService)]
                async fn post(&self, post: Authorized<Read, posts::Entity>) {}
            )))
            .expect("a well-formed binding");
        let gate = posture.gate().expect("it names a subject");
        let entity = &gate.entity;
        assert_eq!(quote!(#entity).to_string(), "posts :: Entity");

        let unmasked = RULES
            .take(&mut method(quote!(
                #[public(Read, posts::Entity, unmasked)]
                async fn feed(&self) {}
            )))
            .expect("the body masks");
        assert!(!unmasked.masks());
    }

    // Spelled through the shared expression, so a posture and an edge parsing its
    // own say one thing.
    #[test]
    fn what_a_posture_withholds_is_read_off_what_it_names() {
        let withheld_by = |tokens| {
            RULES
                .take(&mut method(tokens))
                .expect("a well-formed posture")
                .withheld()
                .to_string()
        };
        for silent in [
            quote!(
                #[public]
                async fn ping(&self) {}
            ),
            quote!(
                async fn ping(&self) {}
            ),
        ] {
            assert_eq!(withheld_by(silent), "true");
        }
        assert_eq!(
            withheld_by(quote!(
                #[authorize(Read, posts::Entity, unmasked)]
                async fn feed(&self) {}
            )),
            "false",
            "`unmasked` leaves the mask to the body, which reads under the ability"
        );
        assert!(
            withheld_by(quote!(
                #[public(Read, posts::Entity)]
                async fn feed(&self) {}
            ))
            .contains("__ROWS"),
            "whether a named subject masks is its type's to say"
        );
    }

    #[test]
    fn a_public_it_cannot_read_names_public_and_what_it_takes() {
        let err = RULES
            .take(&mut method(quote!(
                #[public(Read)]
                async fn feed(&self) {}
            )))
            .err()
            .expect("it names no subject");
        assert!(
            err.to_string().contains("#[public(Action, Subject)]"),
            "{err}"
        );
    }

    #[test]
    fn a_public_written_twice_is_refused() {
        let err = RULES
            .take(&mut method(quote!(
                #[public]
                #[public]
                async fn ping(&self) {}
            )))
            .err()
            .expect("one posture per operation");
        assert_eq!(err.to_string(), crate::repeated_attribute("public"));
    }

    #[test]
    fn declaring_both_is_a_contradiction() {
        for public in [quote!(#[public]), quote!(#[public(Read, users::Entity)])] {
            let err = RULES
                .take(&mut method(quote!(
                    #[authorize(Read, users::Entity)]
                    #public
                    async fn list(&self) {}
                )))
                .err()
                .expect("gated and public at once must be refused");
            assert!(err.to_string().contains("contradict"), "{err}");
        }
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

    #[test]
    fn a_bind_reads_its_entity_off_the_authorized_parameter() {
        let posture = BINDING
            .take(&mut method(quote!(
                #[authorize(Read, bind = PostsService)]
                async fn post(&self, post: Authorized<Read, posts::Entity>) {}
            )))
            .expect("a well-formed binding");
        let Posture::Authorize(PostureGate { entity, bind, .. }) = posture else {
            panic!("expected the authorize posture");
        };
        assert_eq!(quote!(#entity).to_string(), "posts :: Entity");
        assert_eq!(quote!(#bind).to_string(), "PostsService");
    }

    #[test]
    fn a_bind_with_no_parameter_to_receive_its_subject_is_refused() {
        let err = BINDING
            .take(&mut method(quote!(
                #[authorize(Read, bind = PostsService)]
                async fn post(&self) {}
            )))
            .err()
            .expect("nothing receives the subject");
        assert!(err.to_string().contains("Authorized<Action, E>"), "{err}");
    }

    #[test]
    fn a_bind_written_twice_is_refused() {
        let err = BINDING
            .take(&mut method(quote!(
                #[authorize(Read, bind = PostsService, bind = OtherService)]
                async fn post(&self, post: Authorized<Read, posts::Entity>) {}
            )))
            .err()
            .expect("one service loads the subject");
        assert_eq!(err.to_string(), duplicate_argument("authorize", "bind"));
    }

    #[test]
    fn a_posture_it_cannot_read_names_bind_where_the_site_takes_it() {
        let err = BINDING
            .take(&mut method(quote!(
                #[authorize(bind = PostsService)]
                async fn post(&self, post: Authorized<Read, posts::Entity>) {}
            )))
            .err()
            .expect("it names no action");
        assert!(err.to_string().contains("bind = Service"), "{err}");
    }

    #[test]
    fn a_posture_it_cannot_read_names_no_bind_where_the_site_refuses_it() {
        let err = RULES
            .take(&mut method(quote!(
                #[authorize(Read)]
                async fn ping(&self) {}
            )))
            .err()
            .expect("it names no subject");
        assert!(!err.to_string().contains("bind"), "{err}");
    }
}
