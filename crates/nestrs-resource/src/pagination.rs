//! Page-based pagination primitives shared by every `#[expose(paginate)]`
//! entity. [`PageArgs`] is the request side (a GraphQL `InputObject`, an OpenAPI
//! schema, and `validator`-checked — so one type binds a `#[query]` argument and
//! a `Valid<Json<…>>` / query extractor identically), while the per-entity
//! `<Name>Page` envelope (emitted by the macro) is the response side. The macro
//! builds the envelope with `<Name>Page::new(items, total, &args)`, which derives
//! the page-count / has-more flags from these args, so the math lives in one
//! place rather than being recomputed at every list endpoint.

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

/// Page-based list arguments: a 1-based `page` and a `per_page` size. Defaults
/// (page 1, 20 per page) apply on both surfaces — GraphQL via `#[graphql(default
/// = …)]`, REST via `#[serde(default = …)]` — so a caller may omit either. The
/// `validator` bounds are enforced wherever the value crosses a boundary
/// (`Valid<…>` for REST; a resolver calls [`PageArgs::validate`] for GraphQL).
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
    /// SQL `OFFSET` for this page (`(page - 1) * per_page`), saturating so an
    /// out-of-range `page` of `0` cannot underflow.
    pub fn offset(&self) -> u64 {
        self.page.saturating_sub(1) * self.per_page
    }

    /// SQL `LIMIT` for this page — the page size.
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
        assert!(PageArgs {
            page: 0,
            per_page: 20
        }
        .validate()
        .is_err());
        assert!(PageArgs {
            page: 1,
            per_page: 1000
        }
        .validate()
        .is_err());
        assert!(PageArgs {
            page: 1,
            per_page: 20
        }
        .validate()
        .is_ok());
    }
}
