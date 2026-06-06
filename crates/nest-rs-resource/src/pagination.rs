//! Page-based pagination primitives shared by every `#[expose(paginate)]`
//! entity. [`PageArgs`] is the request side (one type binds a GraphQL
//! `#[query]` argument and a REST `Valid<Json<…>>` / query extractor); the
//! per-entity `<Name>Page` envelope (emitted by the macro) is the response
//! side.

use async_graphql::InputObject;
use schemars::JsonSchema;
use serde::Deserialize;
use validator::Validate;

fn default_page() -> u64 {
    1
}

fn default_per_page() -> u64 {
    20
}

/// 1-based `page` and `per_page` size. Defaults (1, 20) apply on both surfaces.
/// `validator` bounds enforce at the boundary (`Valid<…>` for REST; resolvers
/// call [`PageArgs::validate`]).
#[derive(Debug, Clone, Deserialize, InputObject, JsonSchema, Validate)]
pub struct PageArgs {
    #[graphql(default = 1)]
    #[serde(default = "default_page")]
    #[validate(range(min = 1))]
    pub page: u64,
    #[graphql(default = 20)]
    #[serde(default = "default_per_page")]
    #[validate(range(min = 1, max = 100))]
    pub per_page: u64,
}

impl Default for PageArgs {
    fn default() -> Self {
        Self {
            page: default_page(),
            per_page: default_per_page(),
        }
    }
}

impl PageArgs {
    /// `(page - 1) * per_page`, saturating so `page = 0` does not underflow.
    pub fn offset(&self) -> u64 {
        self.page.saturating_sub(1) * self.per_page
    }

    pub fn limit(&self) -> u64 {
        self.per_page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_first_page_of_twenty() {
        let args = PageArgs::default();
        assert_eq!(args.page, 1);
        assert_eq!(args.per_page, 20);
        assert_eq!(args.offset(), 0);
        assert_eq!(args.limit(), 20);
    }

    #[test]
    fn offset_is_zero_based_from_one_based_page() {
        let args = PageArgs {
            page: 3,
            per_page: 25,
        };
        assert_eq!(args.offset(), 50);
        assert_eq!(args.limit(), 25);
    }

    #[test]
    fn offset_saturates_on_page_zero() {
        let args = PageArgs {
            page: 0,
            per_page: 10,
        };
        assert_eq!(args.offset(), 0);
    }

    #[test]
    fn validation_rejects_out_of_range() {
        use validator::Validate;
        assert!(
            PageArgs {
                page: 0,
                per_page: 20
            }
            .validate()
            .is_err()
        );
        assert!(
            PageArgs {
                page: 1,
                per_page: 1000
            }
            .validate()
            .is_err()
        );
        assert!(
            PageArgs {
                page: 1,
                per_page: 20
            }
            .validate()
            .is_ok()
        );
    }
}
