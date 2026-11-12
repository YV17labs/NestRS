//! [`Refusals`] — every refusal a decorator finds on one item, reported at
//! once.

/// Every refusal a decorator finds while it reads a whole item, reported
/// together: a developer fixes every mistake of a build, not one per build.
///
/// [`finish`](Self::finish) is the one way out — a `Refusals` dropped before
/// it reports nothing.
///
/// ```
/// use nest_rs_codegen::{Grammar, Refusals};
///
/// const STEP: Grammar = Grammar::new("step", &["name"]);
///
/// let block: syn::ItemImpl = syn::parse_quote! {
///     impl Pipeline {
///         #[step(nmae = "fetch")] fn fetch(&self) {}
///         #[step(name = "parse")] fn parse(&self) {}
///         #[step(name)] fn store(&self) {}
///     }
/// };
/// let mut refusals = Refusals::default();
/// for item in &block.items {
///     let syn::ImplItem::Fn(method) = item else { continue };
///     for attr in method.attrs.iter().filter(|attr| attr.path().is_ident("step")) {
///         let name = refusals.handle(STEP.parse_attr(attr, |arg| arg.str_lit("fetch").map(drop)));
///     }
/// }
/// let refused = refusals.finish().unwrap_err();
/// assert_eq!(refused.into_iter().count(), 2, "the misspelled key and the bare one");
/// ```
#[derive(Debug, Default)]
#[must_use = "a refusal reaches the developer only through `finish`"]
pub struct Refusals {
    refused: Option<syn::Error>,
}

impl Refusals {
    /// Keep `refusal` beside every one kept before.
    pub fn push(&mut self, refusal: syn::Error) {
        match &mut self.refused {
            Some(refused) => refused.combine(refusal),
            None => self.refused = Some(refusal),
        }
    }

    /// The value `result` holds, or `None` with its refusal kept.
    pub fn handle<T>(&mut self, result: syn::Result<T>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(refusal) => {
                self.push(refusal);
                None
            }
        }
    }

    /// Every refusal kept, as one error rustc reports at each one's span;
    /// `Ok` when none was.
    pub fn finish(self) -> syn::Result<()> {
        match self.refused {
            Some(refused) => Err(refused),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use proc_macro2::Span;

    use super::*;

    fn refusal(sentence: &str) -> syn::Error {
        syn::Error::new(Span::call_site(), sentence)
    }

    #[test]
    fn nothing_kept_finishes_clean() {
        assert!(Refusals::default().finish().is_ok());
    }

    #[test]
    fn every_refusal_kept_is_reported_in_the_order_found() {
        let mut refusals = Refusals::default();
        refusals.push(refusal("first"));
        assert_eq!(refusals.handle(Ok::<_, syn::Error>(7)), Some(7));
        assert_eq!(refusals.handle::<u8>(Err(refusal("second"))), None);
        let reported: Vec<String> = refusals
            .finish()
            .expect_err("two refusals were kept")
            .into_iter()
            .map(|refusal| refusal.to_string())
            .collect();
        assert_eq!(reported, ["first", "second"]);
    }
}
